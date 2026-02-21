use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, error, info, warn};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::Config;
use crate::pac::PacEngine;

pub struct Proxy {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
}

impl Proxy {
    pub fn new(config: Arc<Config>, pac: Option<PacEngine>) -> Self {
        Proxy {
            config,
            pac: Arc::new(pac),
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.config.proxy.port));
        let listener = TcpListener::bind(addr).await?;
        info!("Listening on http://{}", addr);
        self.run_with_listener(listener).await
    }

    pub async fn run_with_listener(
        &self,
        listener: TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let config = self.config.clone();
            let pac = self.pac.clone();

            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .preserve_header_case(true)
                    .title_case_headers(true)
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let config = config.clone();
                            let pac = pac.clone();
                            async move { proxy(req, config, pac).await }
                        }),
                    )
                    .with_upgrades()
                    .await
                {
                    error!("Failed to serve connection: {:?}", err);
                }
            });
        }
    }
}

async fn proxy(
    req: Request<hyper::body::Incoming>,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
) -> Result<Response<Empty<Bytes>>, hyper::Error> {
    if Method::CONNECT == req.method() {
        if let Some(addr) = req.uri().authority().map(|a| a.to_string()) {
            let config = config.clone();
            let pac = pac.clone();

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
            Ok(Response::new(Empty::new()))
        } else {
            error!("CONNECT host is missing");
            let mut resp = Response::new(Empty::new());
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            Ok(resp)
        }
    } else {
        // Handle HTTP Proxying (GET, POST, etc.)
        let mut resp = Response::new(Empty::new());
        *resp.status_mut() = StatusCode::NOT_IMPLEMENTED;
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

    // Check Exceptions
    let host = target.split(':').next().unwrap_or(&target);
    if let Some(exceptions) = &config.exceptions {
        for pattern in &exceptions.hosts {
            if pattern == host {
                debug!("Exception matched exact host: {}, direct", host);
                return connect_direct(&mut upgraded, &target).await;
            }
            if pattern.starts_with("*.") && host.ends_with(&pattern[2..]) {
                debug!("Exception matched glob: {}, direct", host);
                return connect_direct(&mut upgraded, &target).await;
            }
        }
    }

    // Determine Upstream
    let upstream_proxy: Option<String> = if let Some(pac_engine) = &*pac {
        let url = format!("https://{}/", target);
        match pac_engine.find_proxy(&url, host).await {
            Ok(proxy_str) => {
                debug!("PAC returned: {}", proxy_str);
                let parts: Vec<&str> = proxy_str.split(';').collect();
                let first = parts[0].trim();
                if first.starts_with("PROXY") {
                    Some(first[6..].trim().to_string())
                } else {
                    None
                }
            }
            Err(e) => {
                error!("PAC error: {}, falling back to config", e);
                // Fallback to config if PAC fails?
                config.upstream.as_ref().and_then(|u| u.proxy_url.clone())
            }
        }
    } else {
        config.upstream.as_ref().and_then(|u| u.proxy_url.clone())
    };

    if let Some(proxy_addr) = upstream_proxy {
        debug!("Connecting via upstream: {}", proxy_addr);
        connect_via_upstream(&mut upgraded, &target, &proxy_addr, &config).await
    } else {
        debug!("Connecting direct: {}", target);
        connect_direct(&mut upgraded, &target).await
    }
}

async fn connect_direct(upgraded: &mut TokioIo<Upgraded>, target: &str) -> std::io::Result<()> {
    let mut server = TcpStream::connect(target).await?;
    let _ = tokio::io::copy_bidirectional(upgraded, &mut server).await?;
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

    let mut server = TcpStream::connect(addr).await?;

    // Send CONNECT request to upstream
    let mut connect_req = format!("CONNECT {} HTTP/1.1\r\nHost: {}\r\n", target, target);

    // Auth Logic
    if let Some(upstream_conf) = &config.upstream {
        match upstream_conf.auth_type.as_str() {
            "basic" => {
                if let (Some(u), Some(p)) = (&upstream_conf.username, &upstream_conf.password) {
                    let creds = format!("{}:{}", u, p);
                    use base64::prelude::*;
                    let encoded = BASE64_STANDARD.encode(creds);
                    connect_req.push_str(&format!("Proxy-Authorization: Basic {}\r\n", encoded));
                }
            }
            _ => {}
        }
    }

    connect_req.push_str("\r\n"); // End of headers
    server.write_all(connect_req.as_bytes()).await?;

    // Read response line
    let mut buf = [0u8; 4096];
    let mut header_buf = Vec::new();
    loop {
        let n = server.read(&mut buf).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "Upstream closed connection",
            ));
        }
        header_buf.extend_from_slice(&buf[..n]);
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
