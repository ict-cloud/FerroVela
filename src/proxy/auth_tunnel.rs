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

    Ok(String::from_utf8_lossy(&buf).into_owned())
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
fn find_proxy_authenticate(headers: &str) -> Option<String> {
    for line in headers.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.len() > 20 && trimmed[..20].eq_ignore_ascii_case("Proxy-Authenticate: ") {
            return Some(trimmed[20..].trim().to_string());
        }
        // Handle "Proxy-Authenticate:" without trailing space
        if let Some(rest) = trimmed
            .strip_prefix("Proxy-Authenticate:")
            .or_else(|| trimmed.strip_prefix("proxy-authenticate:"))
        {
            return Some(rest.trim().to_string());
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
    let challenge = find_proxy_authenticate(&raw);

    // Drain body so the connection stays usable for the next request.
    let content_length = crate::proxy::http_utils::parse_content_length(&raw);
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        stream.read_exact(&mut body).await?;
    }

    Ok((status, challenge))
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
            "round {}: sending Proxy-Authorization: {}",
            round,
            &auth_header[..auth_header.find(' ').unwrap_or(auth_header.len()).min(20)]
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

/// Top-level handler for a single client connection when Kerberos or NTLM is
/// the configured auth type.
///
/// Routing:
/// - `CONNECT`  + proxy needed  → [`perform_authenticated_connect`] + splice.
/// - `CONNECT`  + direct        → plain TCP connect to `target` + splice.
/// - anything else              → forward buffered request to g3proxy.
pub async fn handle_authenticated_tunnel(
    mut client: TcpStream,
    internal_port: u16,
    authenticator: Arc<dyn UpstreamAuthenticator>,
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
                // ── auth tunnel ──────────────────────────────────────────
                match perform_authenticated_connect(&proxy_addr, &target, &authenticator).await {
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
            }
            None => {
                // ── direct CONNECT (exception or no upstream) ────────────
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
        // Plain HTTP request: re-inject buffered headers into a g3proxy connection.
        forward_buffered_to_g3proxy(&mut client, internal_port, headers.as_bytes()).await;
    }
}

/// Send already-read `buffered` bytes to g3proxy followed by the rest of the
/// client stream (used when the request headers were consumed for routing).
async fn forward_buffered_to_g3proxy(client: &mut TcpStream, internal_port: u16, buffered: &[u8]) {
    let addr = format!("127.0.0.1:{internal_port}");
    let mut upstream = match TcpStream::connect(&addr).await {
        Ok(s) => {
            let _ = s.set_nodelay(true);
            s
        }
        Err(e) => {
            error!("connect to g3proxy at {}: {}", addr, e);
            return;
        }
    };

    if upstream.write_all(buffered).await.is_err() {
        return;
    }

    if let Err(e) = tokio::io::copy_bidirectional(client, &mut upstream).await {
        debug!("g3proxy forward error: {}", e);
    }
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
        assert_eq!(find_proxy_authenticate(headers), Some("NTLM".to_string()));
    }

    #[test]
    fn test_find_proxy_authenticate_negotiate_with_token() {
        let headers = "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Negotiate YIIGhg==\r\n\r\n";
        assert_eq!(
            find_proxy_authenticate(headers),
            Some("Negotiate YIIGhg==".to_string())
        );
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
            Some("Basic realm=\"proxy\"".to_string())
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
}
