/// Authenticated proxy tunnel for Kerberos (Negotiate/SPNEGO) and NTLM.
///
/// Implements the challenge-response HTTP CONNECT handshake required by
/// corporate proxies:
///
///   Client                Pre-processor          Upstream Proxy
///     │── CONNECT host ──▶│                              │
///     │                   │── CONNECT host ─────────────▶│
///     │                   │◀─ 407 Negotiate/NTLM ────────│
///     │                   │  session.step(challenge)     │
///     │                   │── CONNECT + Proxy-Auth ──────▶│
///     │                   │  (NTLM: one more round)      │
///     │                   │◀─ 200 Connection Established─│
///     │◀─ 200 ────────────│                              │
///     │◀══════ splice ════════════════════════════════════│
///
/// For NTLM the loop runs up to three times (Negotiate → Challenge → Response).
/// For Kerberos it typically resolves in one authenticated round.
use std::sync::Arc;

use base64::Engine as _;
use log::{debug, error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::auth::UpstreamAuthenticator;

/// Maximum bytes to read when collecting HTTP headers.
const MAX_HEADER_BYTES: usize = 64 * 1024;

// ─── low-level helpers ───────────────────────────────────────────────────────

/// Read HTTP headers from `stream` until `\r\n\r\n`, returning the raw string
/// (including the terminator).  Does **not** read any body bytes.
///
/// Reads in chunks (up to 4 KiB) instead of byte-at-a-time to minimise
/// syscall overhead.  In the CONNECT handshake context, each HTTP message
/// arrives as a single write from the peer, so `read()` will return the
/// complete headers without over-reading into tunnel payload.
pub async fn read_http_headers(
    stream: &mut TcpStream,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];

    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err("connection closed before end of headers".into());
        }
        buf.extend_from_slice(&chunk[..n]);

        if memchr::memmem::find(&buf, b"\r\n\r\n").is_some() {
            break;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err("HTTP headers exceeded maximum size".into());
        }
    }

    // `from_utf8` transfers ownership of `buf` into a `String` without any
    // heap allocation when the bytes are valid UTF-8 (which HTTP headers
    // always are).  The previous `from_utf8_lossy(&buf).into_owned()` was
    // borrowing `buf` and then cloning, wasting one full heap copy per call.
    Ok(String::from_utf8(buf).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Extract the target `host:port` from a CONNECT request line, e.g.
/// `"CONNECT example.com:443 HTTP/1.1"` → `"example.com:443"`.
pub fn parse_connect_target(request_line: &str) -> Option<String> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?;
    if !method.eq_ignore_ascii_case("CONNECT") {
        return None;
    }
    Some(parts.next()?.to_string())
}

/// Extract the HTTP method from the first line of a request.
pub fn http_method(request_line: &str) -> &str {
    request_line.split_whitespace().next().unwrap_or("")
}

/// Return the value of the `Proxy-Authenticate` header (first occurrence),
/// e.g. `"NTLM"`, `"NTLM <base64>"`, `"Negotiate"`, `"Negotiate <base64>"`.
///
/// Returns a `&str` borrowed directly from `headers` — no heap allocation.
fn find_proxy_authenticate(headers: &str) -> Option<&str> {
    for line in headers.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.len() > 20 && trimmed[..20].eq_ignore_ascii_case("Proxy-Authenticate: ") {
            return Some(trimmed[20..].trim());
        }
        // Handle "Proxy-Authenticate:" without trailing space
        if let Some(rest) = trimmed
            .strip_prefix("Proxy-Authenticate:")
            .or_else(|| trimmed.strip_prefix("proxy-authenticate:"))
        {
            return Some(rest.trim());
        }
    }
    None
}

/// Parse the HTTP status code from a response's first line.
fn parse_status(raw_response: &str) -> Option<u16> {
    raw_response
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Read the HTTP status line + headers and optionally drain the body
/// (identified via `Content-Length`).  Returns `(status_code, Proxy-Authenticate value)`.
async fn read_proxy_response(
    stream: &mut TcpStream,
) -> Result<(u16, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
    let raw = read_http_headers(stream).await?;

    let status = parse_status(&raw).ok_or("could not parse HTTP status")?;
    // `find_proxy_authenticate` returns a `&str` into `raw`; convert to
    // owned only once here, at the boundary where ownership is required.
    let challenge = find_proxy_authenticate(&raw).map(str::to_owned);

    // Drain body so the connection stays usable for the next request.
    let content_length = crate::proxy::http_utils::parse_content_length(&raw);
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        stream.read_exact(&mut body).await?;
    }

    Ok((status, challenge))
}

/// Send a `CONNECT target HTTP/1.1` request and return only the status code.
/// Used by the rama CONNECT handler for the unauthenticated upstream path.
pub(crate) async fn send_connect_request(
    stream: &mut TcpStream,
    target: &str,
    proxy_authorization: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_connect(stream, target, proxy_authorization).await
}

/// Read a proxy CONNECT response and return only the status code.
/// Used by the rama CONNECT handler for the unauthenticated upstream path.
pub(crate) async fn read_connect_response(
    stream: &mut TcpStream,
) -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
    read_proxy_response(stream).await.map(|(status, _)| status)
}

/// Write a `CONNECT target HTTP/1.1` request, optionally adding a
/// `Proxy-Authorization` header.
async fn send_connect(
    stream: &mut TcpStream,
    target: &str,
    proxy_authorization: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(auth) = proxy_authorization {
        request.push_str("Proxy-Authorization: ");
        request.push_str(auth);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    Ok(())
}

// ─── public API ──────────────────────────────────────────────────────────────

/// Establish an authenticated CONNECT tunnel through `upstream_proxy` to
/// `target` (format `host:port`).
///
/// Implements up to four round-trips so that it handles:
/// - Kerberos (Negotiate): typically resolves in 1 authenticated round.
/// - NTLM:                 requires 2 authenticated rounds (Type1 → Type3).
pub async fn perform_authenticated_connect(
    upstream_proxy: &str,
    target: &str,
    authenticator: &Arc<dyn UpstreamAuthenticator>,
) -> Result<TcpStream, Box<dyn std::error::Error + Send + Sync>> {
    let mut session = authenticator.create_session();
    let mut upstream = TcpStream::connect(upstream_proxy).await?;
    let _ = upstream.set_nodelay(true);

    // ── initial attempt without auth ─────────────────────────────────────
    send_connect(&mut upstream, target, None).await?;
    let (mut status, mut challenge) = read_proxy_response(&mut upstream).await?;

    if status == 200 {
        debug!("upstream accepted CONNECT without auth");
        return Ok(upstream);
    }

    // ── challenge-response loop ───────────────────────────────────────────
    // Max 4 iterations covers Kerberos (1) + NTLM (2) with headroom.
    for round in 0..4 {
        if status != 407 {
            return Err(format!("upstream proxy returned {status} (round {round})").into());
        }

        // The challenge value fed into step() is the raw Proxy-Authenticate
        // header, e.g. "Negotiate", "NTLM <base64>", "Negotiate <base64>".
        let auth_header = match session.step(challenge.as_deref())? {
            Some(h) => h,
            None => {
                return Err(format!(
                    "auth session produced no token on round {round}; status was {status}"
                )
                .into())
            }
        };

        debug!(
            "round {}: sending Proxy-Authorization scheme: {}",
            round,
            auth_header.split_whitespace().next().unwrap_or("(unknown)")
        );

        send_connect(&mut upstream, target, Some(&auth_header)).await?;
        let (s, c) = read_proxy_response(&mut upstream).await?;
        status = s;
        challenge = c;

        if status == 200 {
            debug!(
                "authenticated CONNECT established after {} round(s)",
                round + 1
            );
            return Ok(upstream);
        }
    }

    Err(format!("authentication exhausted all rounds; final status: {status}").into())
}

/// Top-level handler for a single client connection.
///
/// Routing:
/// - `CONNECT` + upstream + authenticator → [`perform_authenticated_connect`] + splice.
/// - `CONNECT` + upstream + no auth       → plain CONNECT tunnel (no credentials).
/// - `CONNECT` + direct                   → TCP connect to `target` + SSRF guard + splice.
/// - anything else                        → [`handle_plain_http_request`].
pub async fn handle_authenticated_tunnel(
    mut client: TcpStream,
    authenticator: Option<Arc<dyn UpstreamAuthenticator>>,
    config: Arc<crate::config::Config>,
    pac: Arc<Option<crate::pac::PacEngine>>,
) {
    let headers = match read_http_headers(&mut client).await {
        Ok(h) => h,
        Err(e) => {
            debug!("failed to read client headers: {}", e);
            return;
        }
    };

    let first_line = headers.lines().next().unwrap_or("");

    if http_method(first_line).eq_ignore_ascii_case("CONNECT") {
        let Some(target) = parse_connect_target(first_line) else {
            debug!("malformed CONNECT line: {}", first_line);
            return;
        };

        // Resolve which upstream to use (PAC or static config).
        let resolved = crate::proxy::resolve_proxy(&target, &config, &pac).await;

        match resolved {
            Some(proxy_addr) => {
                if let Some(auth) = authenticator {
                    // ── authenticated CONNECT (Kerberos, NTLM, Basic) ──────
                    match perform_authenticated_connect(&proxy_addr, &target, &auth).await {
                        Ok(mut upstream) => {
                            let _ = client
                                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                                .await;
                            if let Err(e) =
                                tokio::io::copy_bidirectional(&mut client, &mut upstream).await
                            {
                                debug!("splice error for {}: {}", target, e);
                            }
                        }
                        Err(e) => {
                            error!("auth tunnel to {} via {}: {}", target, proxy_addr, e);
                            let _ = client
                                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                                .await;
                        }
                    }
                } else {
                    // ── unauthenticated CONNECT through upstream proxy ─────
                    let addr = normalize_proxy_addr(&proxy_addr);
                    match TcpStream::connect(&addr).await {
                        Ok(mut upstream) => {
                            let _ = upstream.set_nodelay(true);
                            if send_connect(&mut upstream, &target, None).await.is_err() {
                                return;
                            }
                            match read_proxy_response(&mut upstream).await {
                                Ok((200, _)) => {
                                    let _ = client
                                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                                        .await;
                                    let _ =
                                        tokio::io::copy_bidirectional(&mut client, &mut upstream)
                                            .await;
                                }
                                Ok((status, _)) => {
                                    error!(
                                        "upstream proxy returned {} for CONNECT {}",
                                        status, target
                                    );
                                    let _ = client
                                        .write_all(
                                            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n",
                                        )
                                        .await;
                                }
                                Err(e) => {
                                    error!("upstream proxy response for {}: {}", target, e);
                                    let _ = client
                                        .write_all(
                                            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n",
                                        )
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            error!("connect to upstream {}: {}", addr, e);
                            let _ = client
                                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                                .await;
                        }
                    }
                }
            }
            None => {
                // ── direct CONNECT (exception or no upstream) ────────────
                if !config.proxy.allow_private_ips && crate::proxy::ssrf::is_private_target(&target)
                {
                    log::warn!("SSRF blocked: direct CONNECT to private address {}", target);
                    let _ = client
                        .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                }
                match TcpStream::connect(&target).await {
                    Ok(mut upstream) => {
                        let _ = upstream.set_nodelay(true);
                        let _ = client
                            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                            .await;
                        if let Err(e) =
                            tokio::io::copy_bidirectional(&mut client, &mut upstream).await
                        {
                            debug!("direct splice error for {}: {}", target, e);
                        }
                    }
                    Err(e) => {
                        error!("direct connect to {}: {}", target, e);
                        let _ = client
                            .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                            .await;
                    }
                }
            }
        }
    } else {
        // Plain HTTP request: handle natively.
        handle_plain_http_request(&mut client, &headers, &config, &pac).await;
    }
}

// ─── plain HTTP forwarding ────────────────────────────────────────────────────

/// Handle a plain HTTP (non-CONNECT) request from the client.
///
/// - Upstream proxy configured: connect to it and forward the request as-is,
///   injecting a `Proxy-Authorization` header for Basic auth.
/// - No upstream (direct): parse the URL, apply SSRF guard, rewrite the
///   request line to origin-form, connect to the target, and relay.
async fn handle_plain_http_request(
    client: &mut TcpStream,
    headers: &str,
    config: &Arc<crate::config::Config>,
    pac: &Arc<Option<crate::pac::PacEngine>>,
) {
    let first_line = headers.lines().next().unwrap_or("");
    let url_str = first_line.split_whitespace().nth(1).unwrap_or("");

    let target = match target_from_http_url(url_str) {
        Some(t) => t,
        None => {
            debug!("plain HTTP: cannot parse target from URL: {}", url_str);
            let _ = client
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await;
            return;
        }
    };

    let resolved = crate::proxy::resolve_proxy(&target, config, pac).await;

    match resolved {
        Some(proxy_url) => {
            // Forward to upstream proxy, optionally adding Basic auth.
            let upstream_addr = normalize_proxy_addr(&proxy_url);
            let mut upstream = match TcpStream::connect(&upstream_addr).await {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    s
                }
                Err(e) => {
                    error!("plain HTTP: connect to upstream {}: {}", upstream_addr, e);
                    let _ = client
                        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                }
            };
            let request = inject_basic_proxy_auth(headers, config);
            if upstream.write_all(request.as_bytes()).await.is_err() {
                return;
            }
            if let Err(e) = tokio::io::copy_bidirectional(client, &mut upstream).await {
                debug!("plain HTTP upstream relay error: {}", e);
            }
        }
        None => {
            // Direct connection: SSRF guard, rewrite request line, relay.
            if !config.proxy.allow_private_ips && crate::proxy::ssrf::is_private_target(&target) {
                log::warn!("SSRF blocked: plain HTTP to private address {}", target);
                let _ = client
                    .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    .await;
                return;
            }
            let mut upstream = match TcpStream::connect(&target).await {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    s
                }
                Err(e) => {
                    error!("plain HTTP: direct connect to {}: {}", target, e);
                    let _ = client
                        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                }
            };
            let request = rewrite_request_for_direct(headers, url_str);
            if upstream.write_all(request.as_bytes()).await.is_err() {
                return;
            }
            if let Err(e) = tokio::io::copy_bidirectional(client, &mut upstream).await {
                debug!("plain HTTP direct relay error: {}", e);
            }
        }
    }
}

/// Extract `host:port` from a plain HTTP proxy request URL.
///
/// `http://example.com:8080/path` → `"example.com:8080"`
/// `http://example.com/path`      → `"example.com:80"`
fn target_from_http_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let port = u.port_or_known_default()?;
    match u.host()? {
        url::Host::Ipv6(addr) => Some(format!("[{}]:{}", addr, port)),
        host => Some(format!("{}:{}", host, port)),
    }
}

/// Normalise a proxy address string to `host:port` suitable for `TcpStream::connect`.
///
/// `resolve_proxy` returns a full URL (from static config) or a bare `host:port`
/// (from PAC).  This function handles both forms.
pub(crate) fn normalize_proxy_addr(proxy: &str) -> String {
    if proxy.contains("://") {
        crate::proxy::proxy_addr_from_url(proxy).unwrap_or_else(|| proxy.to_string())
    } else {
        proxy.to_string()
    }
}

/// Inject a `Proxy-Authorization: Basic …` header when Basic auth is configured.
///
/// Inserts the header before the blank line that terminates the headers block
/// so that the `\r\n\r\n` terminator is preserved at the very end.
fn inject_basic_proxy_auth(headers: &str, config: &Arc<crate::config::Config>) -> String {
    let Some(upstream) = &config.upstream else {
        return headers.to_string();
    };
    if upstream.auth_type != "basic" {
        return headers.to_string();
    }
    let user = upstream.username.as_deref().unwrap_or("");
    let pass = upstream.password.as_deref().unwrap_or("");
    let creds = base64::prelude::BASE64_STANDARD.encode(format!("{user}:{pass}"));
    let auth_line = format!("Proxy-Authorization: Basic {creds}\r\n");

    // Headers string ends with \r\n\r\n; insert before the terminal \r\n.
    if let Some(pos) = memchr::memmem::find(headers.as_bytes(), b"\r\n\r\n") {
        format!("{}{}\r\n", &headers[..pos + 2], auth_line)
    } else {
        format!("{}{}\r\n", headers, auth_line)
    }
}

/// Rewrite a proxy-style request line to origin-form for direct connections.
///
/// `GET http://example.com/path?q=1 HTTP/1.1` → `GET /path?q=1 HTTP/1.1`
fn rewrite_request_for_direct(headers: &str, url: &str) -> String {
    let path = url::Url::parse(url)
        .ok()
        .map(|u| {
            let mut p = u.path().to_string();
            if let Some(q) = u.query() {
                p.push('?');
                p.push_str(q);
            }
            if p.is_empty() {
                "/".to_string()
            } else {
                p
            }
        })
        .unwrap_or_else(|| "/".to_string());

    if let Some(eol) = headers.find("\r\n") {
        let first_line = &headers[..eol];
        let mut parts = first_line.splitn(3, ' ');
        if let (Some(method), Some(_url), Some(version)) =
            (parts.next(), parts.next(), parts.next())
        {
            return format!("{} {} {}{}", method, path, version, &headers[eol..]);
        }
    }
    headers.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_connect_target ──────────────────────────────────────────────────

    #[test]
    fn test_parse_connect_target_valid() {
        assert_eq!(
            parse_connect_target("CONNECT example.com:443 HTTP/1.1"),
            Some("example.com:443".to_string())
        );
    }

    #[test]
    fn test_parse_connect_target_case_insensitive() {
        assert_eq!(
            parse_connect_target("connect example.com:443 HTTP/1.1"),
            Some("example.com:443".to_string())
        );
    }

    #[test]
    fn test_parse_connect_target_not_connect_method() {
        assert_eq!(parse_connect_target("GET / HTTP/1.1"), None);
    }

    #[test]
    fn test_parse_connect_target_missing_target() {
        assert_eq!(parse_connect_target("CONNECT"), None);
    }

    #[test]
    fn test_parse_connect_target_empty() {
        assert_eq!(parse_connect_target(""), None);
    }

    // ── http_method ───────────────────────────────────────────────────────────

    #[test]
    fn test_http_method_connect() {
        assert_eq!(http_method("CONNECT example.com:443 HTTP/1.1"), "CONNECT");
    }

    #[test]
    fn test_http_method_get() {
        assert_eq!(http_method("GET / HTTP/1.1"), "GET");
    }

    #[test]
    fn test_http_method_post() {
        assert_eq!(http_method("POST /path HTTP/1.1"), "POST");
    }

    #[test]
    fn test_http_method_empty() {
        assert_eq!(http_method(""), "");
    }

    // ── find_proxy_authenticate ───────────────────────────────────────────────

    #[test]
    fn test_find_proxy_authenticate_ntlm() {
        let headers = "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: NTLM\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(find_proxy_authenticate(headers), Some("NTLM"));
    }

    #[test]
    fn test_find_proxy_authenticate_negotiate_with_token() {
        let headers = "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Negotiate YIIGhg==\r\n\r\n";
        assert_eq!(find_proxy_authenticate(headers), Some("Negotiate YIIGhg=="));
    }

    #[test]
    fn test_find_proxy_authenticate_missing() {
        let headers = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(find_proxy_authenticate(headers), None);
    }

    #[test]
    fn test_find_proxy_authenticate_lowercase_header() {
        let headers = "HTTP/1.1 407 Proxy Authentication Required\r\nproxy-authenticate: Basic realm=\"proxy\"\r\n\r\n";
        assert_eq!(
            find_proxy_authenticate(headers),
            Some("Basic realm=\"proxy\"")
        );
    }

    // ── parse_status ──────────────────────────────────────────────────────────

    #[test]
    fn test_parse_status_200() {
        assert_eq!(
            parse_status("HTTP/1.1 200 Connection established\r\n"),
            Some(200)
        );
    }

    #[test]
    fn test_parse_status_407() {
        assert_eq!(
            parse_status("HTTP/1.1 407 Proxy Authentication Required\r\n"),
            Some(407)
        );
    }

    #[test]
    fn test_parse_status_502() {
        assert_eq!(parse_status("HTTP/1.1 502 Bad Gateway\r\n"), Some(502));
    }

    #[test]
    fn test_parse_status_malformed() {
        assert_eq!(parse_status("not a response"), None);
        assert_eq!(parse_status("HTTP/1.1"), None);
        assert_eq!(parse_status(""), None);
    }

    // ── target_from_http_url ──────────────────────────────────────────────────

    #[test]
    fn test_target_from_http_url_with_port() {
        assert_eq!(
            target_from_http_url("http://example.com:8080/path"),
            Some("example.com:8080".to_string())
        );
    }

    #[test]
    fn test_target_from_http_url_default_port() {
        assert_eq!(
            target_from_http_url("http://example.com/path"),
            Some("example.com:80".to_string())
        );
    }

    #[test]
    fn test_target_from_http_url_invalid() {
        assert_eq!(target_from_http_url("not-a-url"), None);
        assert_eq!(target_from_http_url(""), None);
    }

    // ── rewrite_request_for_direct ────────────────────────────────────────────

    #[test]
    fn test_rewrite_request_for_direct_basic() {
        let headers = "GET http://example.com/path HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let result = rewrite_request_for_direct(headers, "http://example.com/path");
        assert!(result.starts_with("GET /path HTTP/1.1\r\n"));
    }

    #[test]
    fn test_rewrite_request_for_direct_with_query() {
        let headers = "GET http://example.com/search?q=1 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let result = rewrite_request_for_direct(headers, "http://example.com/search?q=1");
        assert!(result.starts_with("GET /search?q=1 HTTP/1.1\r\n"));
    }

    #[test]
    fn test_rewrite_request_for_direct_root() {
        let headers = "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let result = rewrite_request_for_direct(headers, "http://example.com/");
        assert!(result.starts_with("GET / HTTP/1.1\r\n"));
    }

    // ── inject_basic_proxy_auth ───────────────────────────────────────────────

    #[test]
    fn test_inject_basic_proxy_auth_adds_header() {
        use crate::config::{Config, ProxyConfig, UpstreamConfig};
        let config = Arc::new(Config {
            proxy: ProxyConfig::default(),
            upstream: Some(UpstreamConfig {
                auth_type: "basic".to_string(),
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
                ..Default::default()
            }),
            exceptions: None,
        });
        let headers = "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let result = inject_basic_proxy_auth(headers, &config);
        assert!(result.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"));
        assert!(result.ends_with("\r\n\r\n"));
    }

    #[test]
    fn test_inject_basic_proxy_auth_skips_non_basic() {
        use crate::config::{Config, ProxyConfig, UpstreamConfig};
        let config = Arc::new(Config {
            proxy: ProxyConfig::default(),
            upstream: Some(UpstreamConfig {
                auth_type: "ntlm".to_string(),
                ..Default::default()
            }),
            exceptions: None,
        });
        let headers = "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let result = inject_basic_proxy_auth(headers, &config);
        assert_eq!(result, headers);
    }
}
