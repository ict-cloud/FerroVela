use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use http_body_util::combinators::BoxBody;
use memchr::memmem;
use hyper::upgrade::Upgraded;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::auth::UpstreamAuthenticator;
use crate::config::Config;
use crate::pac::PacEngine;
pub use crate::proxy::http_utils::{find_header_value, find_subsequence, parse_content_length};
use crate::proxy::{empty, resolve_proxy};

pub async fn handle(
    req: Request<hyper::body::Incoming>,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    if let Some(addr) = req.uri().authority().map(|a| a.to_string()) {
        tokio::task::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    if let Err(e) = tunnel(upgraded, addr, config, pac, authenticator).await {
                        error!("Tunnel error: {}", e);
                    };
                }
                Err(e) => error!("Upgrade error: {}", e),
            }
        });
        Ok(Response::new(empty()))
    } else {
        error!("CONNECT host is missing");
        let mut resp = Response::new(empty());
        *resp.status_mut() = StatusCode::BAD_REQUEST;
        Ok(resp)
    }
}

async fn tunnel(
    upgraded: Upgraded,
    target: String,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
) -> std::io::Result<()> {
    let mut upgraded = TokioIo::new(upgraded);

    // Resolve Proxy
    let upstream_proxy = resolve_proxy(&target, &config, &pac).await;

    if let Some(proxy_addr) = upstream_proxy {
        debug!("Connecting via upstream: {}", proxy_addr);
        connect_via_upstream(&mut upgraded, &target, &proxy_addr, &config, authenticator).await
    } else {
        debug!("Connecting direct: {}", target);
        connect_direct(&mut upgraded, &target).await
    }
}

async fn connect_direct(upgraded: &mut TokioIo<Upgraded>, target: &str) -> std::io::Result<()> {
    let mut server = TcpStream::connect(target).await?;
    tokio::io::copy_bidirectional(upgraded, &mut server).await?;
    Ok(())
}

async fn connect_via_upstream(
    upgraded: &mut TokioIo<Upgraded>,
    target: &str,
    proxy_addr: &str,
    _config: &Arc<Config>,
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
) -> std::io::Result<()> {
    // Connect to upstream proxy
    let addr = proxy_addr
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    let mut server = TcpStream::connect(addr).await?;

    let mut auth_session = authenticator.as_ref().map(|a| a.create_session());
    let mut challenge: Option<String> = None;
    let mut header_buf = BytesMut::with_capacity(4096);

    // Handshake loop
    loop {
        // 1. Send CONNECT Request
        let mut connect_req = format!("CONNECT {} HTTP/1.1\r\nHost: {}\r\n", target, target);
        connect_req.push_str("Proxy-Connection: Keep-Alive\r\n");

        if let Some(session) = &mut auth_session {
            match session.step(challenge.as_deref()) {
                Ok(Some(h)) => {
                    connect_req.push_str(&format!("Proxy-Authorization: {}\r\n", h));
                }
                Ok(None) => {
                    // Session established or no header needed
                }
                Err(e) => {
                    error!("Auth session step error: {}", e);
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "Auth error"));
                }
            }
        }

        connect_req.push_str("\r\n");
        server.write_all(connect_req.as_bytes()).await?;

        // Reset state for response reading
        header_buf.clear();

        // 2. Read Response Loop
        loop {
            let n = server.read_buf(&mut header_buf).await?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Upstream closed connection",
                ));
            }

            if let Some(pos) = find_subsequence(&header_buf, b"\r\n\r\n") {
                let headers_bytes = &header_buf[..pos];
                let headers_str = String::from_utf8_lossy(headers_bytes).to_string();
                let body_start = pos + 4;

                if headers_str.contains(" 200 ") {
                    // Success!
                    // If we read more than headers (body start), write it to client
                    if body_start < header_buf.len() {
                        upgraded.write_all(&header_buf[body_start..]).await?;
                    }
                    // Start tunnel
                    tokio::io::copy_bidirectional(upgraded, &mut server).await?;
                    return Ok(());
                } else if headers_str.contains(" 407 ") {
                    // Auth Challenge
                    if auth_session.is_none() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "Upstream requires authentication",
                        ));
                    }

                    // Parse Content-Length to drain body
                    let cl = parse_content_length(&headers_str);
                    let total_len = body_start + cl;

                    // Ensure we read the full body
                    while header_buf.len() < total_len {
                         let n = server.read_buf(&mut header_buf).await?;
                         if n == 0 {
                            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Upstream closed connection during body read"));
                        }
                    }

                    // Extract challenge
                    if let Some(val) = find_header_value(&headers_str, "Proxy-Authenticate") {
                        debug!("Received Proxy-Authenticate: {}", val);
                        challenge = Some(val);
                        // Break inner reading loop to send next request
                        break;
                    } else {
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, "407 without Proxy-Authenticate"));
                    }
                } else {
                    error!("Upstream proxy returned error: {}", headers_str.lines().next().unwrap_or(""));
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "Upstream refused connection"));
                }
            }

            if header_buf.len() > 16384 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Header too large"));
            }
        }
    }
}

pub fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        panic!("needle is empty");
    }
    memmem::find(haystack, needle)
}

fn parse_content_length(headers: &str) -> usize {
    let key = "content-length:";
    for line in headers.lines() {
        if line.len() >= key.len() && line[..key.len()].eq_ignore_ascii_case(key) {
            return line[key.len()..].trim().parse().unwrap_or(0);
        }
    }
    0
}

fn find_header_value(headers: &str, key: &str) -> Option<String> {
    for line in headers.lines() {
        if line.len() > key.len()
            && line.as_bytes()[key.len()] == b':'
            && line[..key.len()].eq_ignore_ascii_case(key)
        {
            return Some(line[key.len() + 1..].trim().to_string());
        }
    }
    None
}
