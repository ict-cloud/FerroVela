#![allow(dead_code)]
use log::{debug, error, info, warn};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::Sender;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod http_utils;

pub const MAGIC_SHOW_PATH: &str = "/__ferrovela/show";
pub const MAGIC_SHOW_REQUEST: &str =
    "GET /__ferrovela/show HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

#[derive(Debug, Clone)]
pub enum ProxySignal {
    Show,
}

pub struct Proxy {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
    signal_sender: Option<Sender<ProxySignal>>,
}

impl Proxy {
    pub fn new(
        config: Arc<Config>,
        pac: Option<PacEngine>,
        signal_sender: Option<Sender<ProxySignal>>,
    ) -> Self {
        let authenticator = if let Some(upstream_conf) = &config.upstream {
            create_authenticator(upstream_conf).map(Arc::new)
        } else {
            None
        };

        Proxy {
            config,
            pac: Arc::new(pac),
            authenticator,
            signal_sender,
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = format!("127.0.0.1:{}", self.config.proxy.port);
        let listener = TcpListener::bind(&addr).await?;
        info!("Proxy listening on http://{}", addr);

        self.accept_loop(listener).await
    }

    pub async fn run_with_listener(
        &self,
        listener: TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let local_addr = listener.local_addr()?;
        info!("Proxy listening on http://{}", local_addr);

        self.accept_loop(listener).await
    }

    async fn accept_loop(
        &self,
        listener: TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            debug!("Accepted connection from {}", peer_addr);

            let config = self.config.clone();
            let pac = self.pac.clone();
            let authenticator = self.authenticator.clone();
            let signal_sender = self.signal_sender.clone();

            tokio::spawn(async move {
                if let Err(e) =
                    handle_connection(stream, config, pac, authenticator, signal_sender).await
                {
                    debug!("Connection error from {}: {}", peer_addr, e);
                }
            });
        }
    }
}

/// Parse the first line + headers from a raw HTTP request.
/// Returns (method, uri, version_line, full_header_block_including_trailing_crlf_crlf).
/// The header block does NOT include the final \r\n\r\n separator.
fn parse_request_head(buf: &[u8]) -> Option<(String, String, String, String, usize)> {
    // Find end of headers
    let header_end = http_utils::find_subsequence(buf, b"\r\n\r\n")?;
    let head_bytes = &buf[..header_end];
    let head_str = std::str::from_utf8(head_bytes).ok()?;

    let mut lines = head_str.lines();
    let request_line = lines.next()?;
    let parts: Vec<&str> = request_line.splitn(3, ' ').collect();
    if parts.len() < 3 {
        return None;
    }

    let method = parts[0].to_string();
    let uri = parts[1].to_string();
    let version = parts[2].to_string();

    // Collect remaining headers (skip request line)
    let headers: String = head_str.lines().skip(1).collect::<Vec<&str>>().join("\r\n");

    // total consumed = header_end + 4 bytes for \r\n\r\n
    Some((method, uri, version, headers, header_end + 4))
}

async fn handle_connection(
    mut client: TcpStream,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    _authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
    signal_sender: Option<Sender<ProxySignal>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Read the initial request from the client
    let mut buf = vec![0u8; 8192];
    let mut total_read = 0;

    // Read until we have the full header block
    loop {
        let n = client.read(&mut buf[total_read..]).await?;
        if n == 0 {
            return Ok(()); // Client disconnected
        }
        total_read += n;

        if http_utils::find_subsequence(&buf[..total_read], b"\r\n\r\n").is_some() {
            break;
        }

        // Grow buffer if needed
        if total_read >= buf.len() {
            buf.resize(buf.len() * 2, 0);
        }
    }

    let (method, uri, _version, _headers, head_len) = match parse_request_head(&buf[..total_read]) {
        Some(parsed) => parsed,
        None => {
            let response = "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n";
            client.write_all(response.as_bytes()).await?;
            return Ok(());
        }
    };

    // Check for magic IPC show request
    if method == "GET" && uri == MAGIC_SHOW_PATH {
        info!("Received magic show request via IPC");
        if let Some(sender) = &signal_sender {
            let _ = sender.send(ProxySignal::Show).await;
        }
        let response = "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nOK";
        client.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    if method == "CONNECT" {
        handle_connect(client, &uri, &config, &pac).await
    } else {
        handle_http(
            client,
            &method,
            &uri,
            &buf[..total_read],
            head_len,
            &config,
            &pac,
        )
        .await
    }
}

/// Handle HTTP CONNECT tunneling (for HTTPS).
async fn handle_connect(
    mut client: TcpStream,
    target: &str,
    config: &Arc<Config>,
    pac: &Arc<Option<PacEngine>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("CONNECT request to {}", target);

    // Ensure target has a port
    let target_addr = if target.contains(':') {
        target.to_string()
    } else {
        format!("{}:443", target)
    };

    let upstream = resolve_proxy(&target_addr, config, pac).await;

    match upstream {
        Some(proxy_addr) => {
            // Connect through upstream proxy
            debug!(
                "Tunneling {} via upstream proxy {}",
                target_addr, proxy_addr
            );
            let upstream_addr = normalize_proxy_addr(&proxy_addr);

            match TcpStream::connect(&upstream_addr).await {
                Ok(mut upstream_stream) => {
                    // Send CONNECT to upstream proxy
                    let connect_req = format!(
                        "CONNECT {} HTTP/1.1\r\nHost: {}\r\n\r\n",
                        target_addr, target_addr
                    );
                    upstream_stream.write_all(connect_req.as_bytes()).await?;

                    // Read upstream proxy response
                    let mut resp_buf = vec![0u8; 4096];
                    let n = upstream_stream.read(&mut resp_buf).await?;
                    let resp_str = String::from_utf8_lossy(&resp_buf[..n]);

                    if resp_str.contains("200") {
                        // Upstream tunnel established, tell client
                        client
                            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                            .await?;
                        // Splice the two streams
                        tunnel(client, upstream_stream).await;
                    } else {
                        warn!(
                            "Upstream proxy rejected CONNECT to {}: {}",
                            target_addr,
                            resp_str.lines().next().unwrap_or("")
                        );
                        let response = "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n";
                        client.write_all(response.as_bytes()).await?;
                    }
                }
                Err(e) => {
                    error!(
                        "Failed to connect to upstream proxy {}: {}",
                        upstream_addr, e
                    );
                    let response = "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n";
                    client.write_all(response.as_bytes()).await?;
                }
            }
        }
        None => {
            // DIRECT connection
            debug!("CONNECT direct to {}", target_addr);
            match TcpStream::connect(&target_addr).await {
                Ok(target_stream) => {
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await?;
                    tunnel(client, target_stream).await;
                }
                Err(e) => {
                    error!("Failed to connect directly to {}: {}", target_addr, e);
                    let response = "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n";
                    client.write_all(response.as_bytes()).await?;
                }
            }
        }
    }

    Ok(())
}

/// Handle standard HTTP proxy requests (GET, POST, etc.)
async fn handle_http(
    mut client: TcpStream,
    method: &str,
    uri: &str,
    raw_request: &[u8],
    _head_len: usize,
    config: &Arc<Config>,
    pac: &Arc<Option<PacEngine>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("HTTP {} request to {}", method, uri);

    // Parse the target host:port from the URI
    // For proxy requests, the URI is typically absolute: http://example.com/path
    let (host, port, path) = parse_proxy_uri(uri);

    if host.is_empty() {
        let response = "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 16\r\n\r\nBad request URI.";
        client.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    let target = format!("{}:{}", host, port);
    let upstream = resolve_proxy(&target, config, pac).await;

    match upstream {
        Some(proxy_addr) => {
            // Forward via upstream proxy - send the full absolute URI
            debug!("Forwarding HTTP {} to {} via {}", method, uri, proxy_addr);
            let upstream_addr = normalize_proxy_addr(&proxy_addr);

            match TcpStream::connect(&upstream_addr).await {
                Ok(mut upstream_stream) => {
                    // Forward the raw request as-is (it has the absolute URI the upstream expects)
                    upstream_stream.write_all(raw_request).await?;

                    // Relay response back to client
                    relay_response(&mut upstream_stream, &mut client).await?;
                }
                Err(e) => {
                    error!(
                        "Failed to connect to upstream proxy {}: {}",
                        upstream_addr, e
                    );
                    let response = "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n";
                    client.write_all(response.as_bytes()).await?;
                }
            }
        }
        None => {
            // DIRECT connection - rewrite request to use relative path
            debug!("HTTP {} direct to {}{}", method, target, path);
            match TcpStream::connect(&target).await {
                Ok(mut target_stream) => {
                    // Rewrite the request line to use relative URI
                    let rewritten = rewrite_request_relative(raw_request, &path);
                    target_stream.write_all(&rewritten).await?;

                    // Relay response back to client
                    relay_response(&mut target_stream, &mut client).await?;
                }
                Err(e) => {
                    error!("Failed to connect directly to {}: {}", target, e);
                    let response = "HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n";
                    client.write_all(response.as_bytes()).await?;
                }
            }
        }
    }

    Ok(())
}

/// Bidirectional TCP tunnel between two streams.
async fn tunnel(mut client: TcpStream, mut target: TcpStream) {
    let (mut cr, mut cw) = client.split();
    let (mut tr, mut tw) = target.split();

    let client_to_target = tokio::io::copy(&mut cr, &mut tw);
    let target_to_client = tokio::io::copy(&mut tr, &mut cw);

    let _ = tokio::select! {
        r = client_to_target => r,
        r = target_to_client => r,
    };
}

/// Relay response bytes from origin/upstream back to client.
async fn relay_response(
    source: &mut TcpStream,
    client: &mut TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; 8192];
    loop {
        let n = source.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        client.write_all(&buf[..n]).await?;
    }
    Ok(())
}

/// Parse an absolute proxy URI like "http://host:port/path" into (host, port, path).
fn parse_proxy_uri(uri: &str) -> (String, u16, String) {
    // Handle absolute URIs: http://host:port/path
    if let Some(rest) = uri.strip_prefix("http://") {
        let (authority, path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, "/"),
        };
        let (host, port) = parse_authority(authority, 80);
        return (host, port, path.to_string());
    }

    if let Some(rest) = uri.strip_prefix("https://") {
        let (authority, path) = match rest.find('/') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, "/"),
        };
        let (host, port) = parse_authority(authority, 443);
        return (host, port, path.to_string());
    }

    // Relative URI (shouldn't happen for proxy requests, but handle gracefully)
    (String::new(), 80, uri.to_string())
}

/// Parse "host:port" or "host" from an authority string.
fn parse_authority(authority: &str, default_port: u16) -> (String, u16) {
    if let Some(colon_pos) = authority.rfind(':') {
        let host = &authority[..colon_pos];
        let port = authority[colon_pos + 1..]
            .parse::<u16>()
            .unwrap_or(default_port);
        (host.to_string(), port)
    } else {
        (authority.to_string(), default_port)
    }
}

/// Normalize a proxy address to ensure it has host:port format.
fn normalize_proxy_addr(proxy: &str) -> String {
    let cleaned = proxy
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    if cleaned.contains(':') {
        cleaned.to_string()
    } else {
        format!("{}:3128", cleaned)
    }
}

/// Rewrite a raw HTTP request to use a relative path instead of an absolute URI.
fn rewrite_request_relative(raw: &[u8], relative_path: &str) -> Vec<u8> {
    let raw_str = match std::str::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => return raw.to_vec(),
    };

    // Find the first line
    if let Some(first_line_end) = raw_str.find("\r\n") {
        let first_line = &raw_str[..first_line_end];
        let parts: Vec<&str> = first_line.splitn(3, ' ').collect();
        if parts.len() == 3 {
            let new_first_line = format!("{} {} {}", parts[0], relative_path, parts[2]);
            let rest = &raw_str[first_line_end..];
            return format!("{}{}", new_first_line, rest).into_bytes();
        }
    }

    raw.to_vec()
}

pub async fn resolve_proxy(
    target: &str,
    config: &Arc<Config>,
    pac: &Arc<Option<PacEngine>>,
) -> Option<String> {
    let host = target.split(':').next().unwrap_or(target);

    // Check Exceptions
    if let Some(exceptions) = &config.exceptions {
        if exceptions.matches(host) {
            debug!("Exception matched host: {}, direct", host);
            return None;
        }
    }

    // Determine Upstream
    if let Some(pac_engine) = &**pac {
        let url = format!("https://{}/", target); // Approximation for CONNECT
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
                config.upstream.as_ref().and_then(|u| u.proxy_url.clone())
            }
        }
    } else {
        config.upstream.as_ref().and_then(|u| u.proxy_url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_proxy_uri_http() {
        let (host, port, path) = parse_proxy_uri("http://example.com/foo/bar");
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/foo/bar");
    }

    #[test]
    fn test_parse_proxy_uri_http_with_port() {
        let (host, port, path) = parse_proxy_uri("http://example.com:8080/test");
        assert_eq!(host, "example.com");
        assert_eq!(port, 8080);
        assert_eq!(path, "/test");
    }

    #[test]
    fn test_parse_proxy_uri_no_path() {
        let (host, port, path) = parse_proxy_uri("http://example.com");
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_parse_proxy_uri_https() {
        let (host, port, path) = parse_proxy_uri("https://secure.example.com/path");
        assert_eq!(host, "secure.example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/path");
    }

    #[test]
    fn test_parse_proxy_uri_relative() {
        let (host, port, path) = parse_proxy_uri("/just/a/path");
        assert_eq!(host, "");
        assert_eq!(port, 80);
        assert_eq!(path, "/just/a/path");
    }

    #[test]
    fn test_normalize_proxy_addr() {
        assert_eq!(normalize_proxy_addr("10.0.0.1:3128"), "10.0.0.1:3128");
        assert_eq!(
            normalize_proxy_addr("http://10.0.0.1:3128"),
            "10.0.0.1:3128"
        );
        assert_eq!(normalize_proxy_addr("10.0.0.1"), "10.0.0.1:3128");
        assert_eq!(
            normalize_proxy_addr("http://proxy.example.com:8080"),
            "proxy.example.com:8080"
        );
    }

    #[test]
    fn test_rewrite_request_relative() {
        let raw = b"GET http://example.com/foo HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let rewritten = rewrite_request_relative(raw, "/foo");
        let rewritten_str = String::from_utf8(rewritten).unwrap();
        assert!(rewritten_str.starts_with("GET /foo HTTP/1.1\r\n"));
        assert!(rewritten_str.contains("Host: example.com"));
    }

    #[test]
    fn test_parse_request_head_valid() {
        let raw = b"GET /path HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(result.is_some());
        let (method, uri, version, headers, consumed) = result.unwrap();
        assert_eq!(method, "GET");
        assert_eq!(uri, "/path");
        assert_eq!(version, "HTTP/1.1");
        assert!(headers.contains("Host: example.com"));
        assert_eq!(consumed, raw.len());
    }

    #[test]
    fn test_parse_request_head_connect() {
        let raw = b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(result.is_some());
        let (method, uri, _, _, _) = result.unwrap();
        assert_eq!(method, "CONNECT");
        assert_eq!(uri, "example.com:443");
    }

    #[test]
    fn test_parse_request_head_incomplete() {
        let raw = b"GET /path HTTP/1.1\r\nHost: example.com\r\n";
        let result = parse_request_head(raw);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_request_head_malformed() {
        let raw = b"BADREQUEST\r\n\r\n";
        let result = parse_request_head(raw);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_authority() {
        let (host, port) = parse_authority("example.com:8080", 80);
        assert_eq!(host, "example.com");
        assert_eq!(port, 8080);

        let (host, port) = parse_authority("example.com", 80);
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);

        let (host, port) = parse_authority("example.com", 443);
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[tokio::test]
    async fn test_proxy_binds_to_port() {
        // Use port 0 to let the OS assign a free port
        let config = Arc::new(Config {
            proxy: crate::config::ProxyConfig {
                port: 0,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: None,
            exceptions: None,
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_port = listener.local_addr().unwrap().port();
        assert!(bound_port > 0);

        let proxy = Proxy::new(config, None, None);

        // Spawn proxy in background task
        let proxy_handle = tokio::spawn(async move {
            let _ = proxy.run_with_listener(listener).await;
        });

        // Give the proxy a moment to start accepting
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify the port is accepting connections
        let result = TcpStream::connect(format!("127.0.0.1:{}", bound_port)).await;
        assert!(
            result.is_ok(),
            "Proxy should be accepting connections on port {}",
            bound_port
        );

        proxy_handle.abort();
    }

    #[tokio::test]
    async fn test_proxy_magic_show_request() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);

        let config = Arc::new(Config {
            proxy: crate::config::ProxyConfig {
                port: 0,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: None,
            exceptions: None,
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_port = listener.local_addr().unwrap().port();

        let proxy = Proxy::new(config, None, Some(tx));

        let proxy_handle = tokio::spawn(async move {
            let _ = proxy.run_with_listener(listener).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send magic show request
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", bound_port))
            .await
            .unwrap();
        stream
            .write_all(MAGIC_SHOW_REQUEST.as_bytes())
            .await
            .unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("200 OK"),
            "Expected 200 OK response for magic show request, got: {}",
            response
        );

        // Verify signal was sent
        let signal = rx.try_recv();
        assert!(signal.is_ok(), "Expected ProxySignal::Show to be received");

        proxy_handle.abort();
    }

    #[tokio::test]
    async fn test_proxy_rejects_bad_request() {
        let config = Arc::new(Config {
            proxy: crate::config::ProxyConfig {
                port: 0,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: None,
            exceptions: None,
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_port = listener.local_addr().unwrap().port();

        let proxy = Proxy::new(config, None, None);

        let proxy_handle = tokio::spawn(async move {
            let _ = proxy.run_with_listener(listener).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send garbage
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", bound_port))
            .await
            .unwrap();
        stream.write_all(b"GARBAGE\r\n\r\n").await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("400 Bad Request"),
            "Expected 400 Bad Request, got: {}",
            response
        );

        proxy_handle.abort();
    }

    #[tokio::test]
    async fn test_proxy_http_forward_direct() {
        // Start a simple echo/response HTTP server
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo_listener.local_addr().unwrap().port();

        let echo_handle = tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = echo_listener.accept().await {
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        let _n = stream.read(&mut buf).await.unwrap_or(0);
                        let body = "Hello from echo server";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            }
        });

        // Start the proxy
        let config = Arc::new(Config {
            proxy: crate::config::ProxyConfig {
                port: 0,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: None,
            exceptions: None,
        });

        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy_listener.local_addr().unwrap().port();

        let proxy = Proxy::new(config, None, None);

        let proxy_handle = tokio::spawn(async move {
            let _ = proxy.run_with_listener(proxy_listener).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send an HTTP proxy request through the proxy
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
            .await
            .unwrap();

        let request = format!(
            "GET http://127.0.0.1:{}/test HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            echo_port, echo_port
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut response_buf = Vec::new();
        let _ = stream.read_to_end(&mut response_buf).await;
        let response = String::from_utf8_lossy(&response_buf);

        assert!(
            response.contains("200 OK"),
            "Expected proxied 200 OK, got: {}",
            response
        );
        assert!(
            response.contains("Hello from echo server"),
            "Expected echo server body in response, got: {}",
            response
        );

        proxy_handle.abort();
        echo_handle.abort();
    }

    #[tokio::test]
    async fn test_proxy_connect_tunnel() {
        // Start a simple TCP echo server (simulates the target server behind CONNECT)
        let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo_listener.local_addr().unwrap().port();

        let echo_handle = tokio::spawn(async move {
            if let Ok((mut stream, _)) = echo_listener.accept().await {
                let mut buf = vec![0u8; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    // Echo back what we received
                    let _ = stream.write_all(&buf[..n]).await;
                }
            }
        });

        // Start the proxy
        let config = Arc::new(Config {
            proxy: crate::config::ProxyConfig {
                port: 0,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: None,
            exceptions: None,
        });

        let proxy_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = proxy_listener.local_addr().unwrap().port();

        let proxy = Proxy::new(config, None, None);

        let proxy_handle = tokio::spawn(async move {
            let _ = proxy.run_with_listener(proxy_listener).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send a CONNECT request through the proxy
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
            .await
            .unwrap();

        let connect_req = format!(
            "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
            echo_port, echo_port
        );
        stream.write_all(connect_req.as_bytes()).await.unwrap();

        // Read the 200 Connection Established
        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf[..n]);
        assert!(
            response.contains("200 Connection Established"),
            "Expected 200 Connection Established, got: {}",
            response
        );

        // Now send data through the tunnel and expect it echoed back
        let test_data = b"Hello through tunnel!";
        stream.write_all(test_data).await.unwrap();

        let mut echo_buf = vec![0u8; 1024];
        let n = stream.read(&mut echo_buf).await.unwrap();
        assert_eq!(
            &echo_buf[..n],
            test_data,
            "Expected echoed data through CONNECT tunnel"
        );

        proxy_handle.abort();
        echo_handle.abort();
    }

    #[tokio::test]
    async fn test_proxy_default_port_binding() {
        // Test that Proxy::run() correctly binds to the configured port.
        // We use port 0 to avoid conflicts, but verify the mechanism works.
        let config = Arc::new(Config {
            proxy: crate::config::ProxyConfig {
                port: 0,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: None,
            exceptions: None,
        });

        // Bind manually to verify the mechanism used by run_with_listener
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert!(addr.port() > 0);

        let proxy = Proxy::new(config, None, None);
        let handle = tokio::spawn(async move {
            let _ = proxy.run_with_listener(listener).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // The port should be open and accepting connections
        let conn = TcpStream::connect(addr).await;
        assert!(conn.is_ok(), "Proxy must be listening on the bound address");

        handle.abort();
    }
}
