use std::sync::Arc;

use base64::prelude::*;
use bytes::{Bytes, BytesMut};
use http_body_util::combinators::BoxBody;
use hyper::upgrade::Upgraded;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::config::Config;
use crate::pac::PacEngine;
use crate::proxy::{empty, resolve_proxy};

pub async fn handle(
    req: Request<hyper::body::Incoming>,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    if let Some(addr) = req.uri().authority().map(|a| a.to_string()) {
        tokio::task::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    if let Err(e) = tunnel(upgraded, addr, config, pac).await {
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
) -> std::io::Result<()> {
    let mut upgraded = TokioIo::new(upgraded);

    // Resolve Proxy
    let upstream_proxy = resolve_proxy(&target, &config, &pac).await;

    if let Some(proxy_addr) = upstream_proxy {
        debug!("Connecting via upstream: {}", proxy_addr);
        connect_via_upstream(&mut upgraded, &target, &proxy_addr, &config).await
    } else {
        debug!("Connecting direct: {}", target);
        connect_direct(&mut upgraded, &target).await
    }
}

async fn connect_direct(upgraded: &mut TokioIo<Upgraded>, target: &str) -> std::io::Result<()> {
    let mut server = TcpStream::connect(target)
        .await
        .expect("Failed to connect to target");
    let _ = tokio::io::copy_bidirectional(upgraded, &mut server)
        .await
        .expect("Failed to copy bidirectional");
    Ok(())
}

async fn connect_via_upstream(
    upgraded: &mut TokioIo<Upgraded>,
    target: &str,
    proxy_addr: &str,
    config: &Arc<Config>,
) -> std::io::Result<()> {
    // Connect to upstream proxy
    // proxy_addr might be host:port or scheme://host:port
    // simple heuristic: remove scheme
    let addr = proxy_addr
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    let mut server = TcpStream::connect(addr)
        .await
        .expect("Failed to connect to upstream");

    // Send CONNECT request to upstream
    let mut connect_req = format!("CONNECT {} HTTP/1.1\r\nHost: {}\r\n", target, target);

    // Auth Logic
    if let Some(upstream_conf) = &config.upstream {
        match upstream_conf.auth_type.as_str() {
            "basic" => {
                if let (Some(u), Some(p)) = (&upstream_conf.username, &upstream_conf.password) {
                    let creds = format!("{}:{}", u, p);
                    let encoded = BASE64_STANDARD.encode(creds);
                    connect_req.push_str(&format!("Proxy-Authorization: Basic {}\r\n", encoded));
                }
            }
            _ => {}
        }
    }

    connect_req.push_str("\r\n"); // End of headers
    server.write_all(connect_req.as_bytes()).await?;

    // Read response headers efficiently using BytesMut
    let mut header_buf = BytesMut::with_capacity(4096);
    loop {
        let n = server.read_buf(&mut header_buf).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Upstream closed connection",
            ));
        }
        if let Some(pos) = find_subsequence(&header_buf, b"\r\n\r\n") {
            let body_start = pos + 4;
            let headers_str = String::from_utf8_lossy(&header_buf[..pos]);
            if !headers_str.contains(" 200 ") {
                error!(
                    "Upstream proxy returned error: {}",
                    headers_str.lines().next().unwrap_or("")
                );
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Upstream refused connection",
                ));
            }

            if body_start < header_buf.len() {
                upgraded.write_all(&header_buf[body_start..]).await?;
            }
            break;
        }
    }

    let _ = tokio::io::copy_bidirectional(upgraded, &mut server).await?;
    Ok(())
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
