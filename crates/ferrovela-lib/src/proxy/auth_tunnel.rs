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
pub async fn read_http_headers(
    stream: &mut TcpStream,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let buf = read_head_bytes(stream).await?;
    // `from_utf8` transfers ownership of `buf` into a `String` without any
    // heap allocation when the bytes are valid UTF-8.
    Ok(String::from_utf8(buf)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Read from `stream` up to and including `\r\n\r\n`, and not a byte more.
///
/// Peeks before consuming so that whatever follows the headers — a 407 error
/// body, or tunnel payload sent right behind a `200` — stays in the socket.
async fn read_head_bytes(
    stream: &mut TcpStream,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];

    loop {
        let n = stream.peek(&mut chunk).await?;
        if n == 0 {
            return Err("connection closed before end of headers".into());
        }
        let old_len = buf.len();
        buf.extend_from_slice(&chunk[..n]);

        let end = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
        buf.truncate(end.unwrap_or(buf.len()));
        // Consume exactly the bytes kept from this peek.
        stream.read_exact(&mut chunk[..buf.len() - old_len]).await?;

        if end.is_some() {
            return Ok(buf);
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err("HTTP headers exceeded maximum size".into());
        }
    }
}

/// Find the value of the first header whose name matches `key`
/// (case-insensitive), returning a borrowed `&str` into `headers`.
pub fn find_header_value<'a>(headers: &'a str, key: &str) -> Option<&'a str> {
    let key_len = key.len();
    headers.lines().find_map(|line| {
        (line.len() > key_len
            && line.as_bytes()[key_len] == b':'
            && line[..key_len].eq_ignore_ascii_case(key))
        .then(|| line[key_len + 1..].trim())
    })
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
/// (identified via `Content-Length`).  Returns
/// `(status_code, Proxy-Authenticate value, proxy will close the connection)`.
pub(crate) async fn read_proxy_response(
    stream: &mut TcpStream,
) -> Result<(u16, Option<String>, bool), Box<dyn std::error::Error + Send + Sync>> {
    let raw = read_http_headers(stream).await?;

    let status = parse_status(&raw).ok_or("could not parse HTTP status")?;
    // e.g. "NTLM", "NTLM <base64>", "Negotiate", "Negotiate <base64>".
    let challenge = find_header_value(&raw, "Proxy-Authenticate").map(str::to_owned);

    // Drain body so the connection stays usable for the next request.
    let content_length = find_header_value(&raw, "Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        stream.read_exact(&mut body).await?;
    }

    let close = ["Connection", "Proxy-Connection"]
        .iter()
        .any(|h| find_header_value(&raw, h).is_some_and(|v| v.eq_ignore_ascii_case("close")));

    Ok((status, challenge, close))
}

/// Write a `CONNECT target HTTP/1.1` request, optionally adding a
/// `Proxy-Authorization` header.
pub(crate) async fn send_connect(
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
    let mut session = authenticator.create_session(proxy_host(upstream_proxy));
    let mut upstream = TcpStream::connect(upstream_proxy).await?;
    if let Err(e) = upstream.set_nodelay(true) {
        debug!("set_nodelay failed: {}", e);
    }

    // ── initial attempt without auth ─────────────────────────────────────
    send_connect(&mut upstream, target, None).await?;
    let (mut status, mut challenge, mut close) = read_proxy_response(&mut upstream).await?;
    debug!("CONNECT {target} via {upstream_proxy}: status {status}, close={close}");

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

        // The proxy announced it will drop this connection (common on the
        // first 407), so the retry needs a fresh one.  A mid-handshake NTLM
        // close cannot be recovered and simply fails on the next round.
        if close {
            debug!("round {round}: upstream closed connection, reconnecting");
            upstream = TcpStream::connect(upstream_proxy).await?;
            if let Err(e) = upstream.set_nodelay(true) {
                debug!("set_nodelay failed: {}", e);
            }
        }

        send_connect(&mut upstream, target, Some(&auth_header)).await?;
        (status, challenge, close) = read_proxy_response(&mut upstream).await?;
        debug!("round {round}: status {status}, close={close}");

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

/// Bare hostname of a `host:port` proxy address (IPv6 brackets stripped),
/// as required for the Kerberos SPN.
fn proxy_host(addr: &str) -> &str {
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    host.trim_start_matches('[').trim_end_matches(']')
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
    fn test_proxy_host() {
        assert_eq!(proxy_host("outproxy1.pnet.ch:3128"), "outproxy1.pnet.ch");
        assert_eq!(proxy_host("[::1]:3128"), "::1");
    }

    #[test]
    fn test_find_header_value() {
        let headers = "HTTP/1.1 407 Proxy Authentication Required\r\nproxy-authenticate: Negotiate YIIGhg==\r\nContent-Length:42\r\nConnection: close\r\n\r\n";
        assert_eq!(
            find_header_value(headers, "Proxy-Authenticate"),
            Some("Negotiate YIIGhg==")
        );
        assert_eq!(find_header_value(headers, "content-length"), Some("42"));
        assert_eq!(find_header_value(headers, "Connection"), Some("close"));
        assert_eq!(find_header_value(headers, "Proxy-Connection"), None);
        assert_eq!(
            find_header_value("Proxy-Authenticate: NTLM\r\n", "Proxy-Authenticate"),
            Some("NTLM")
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
