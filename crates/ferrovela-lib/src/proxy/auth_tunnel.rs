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

use log::debug;
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
    Ok(String::from_utf8(buf)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
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

// ─── public API: authenticated CONNECT ───────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;

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
