use std::sync::Arc;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use hyper::client::conn::http1;
use hyper::header::HeaderValue;
use hyper::body::Body;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, error};
use tokio::net::TcpStream;

use crate::auth::UpstreamAuthenticator;
use crate::config::Config;
use crate::pac::PacEngine;
use crate::proxy::{full, resolve_proxy};

pub async fn handle(
    req: Request<hyper::body::Incoming>,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    // Ensure we have a host to connect to
    let uri = req.uri().clone();
    let host = match uri.host() {
        Some(h) => h.to_string(),
        None => {
            let mut resp = Response::new(full("Bad Request: Missing Host"));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };
    let port = uri.port_u16().unwrap_or(80);
    let target_addr = format!("{}:{}", host, port);

    // Resolve Proxy
    let proxy_addr_opt = resolve_proxy(&target_addr, &config, &pac).await;

    if let Some(proxy_addr) = proxy_addr_opt {
        debug!("Proxying {} via upstream: {}", target_addr, proxy_addr);
        handle_upstream(req, proxy_addr, target_addr, authenticator).await
    } else {
        debug!("Proxying {} direct", target_addr);
        handle_direct(req, host, port).await
    }
}

async fn handle_direct(
    mut req: Request<hyper::body::Incoming>,
    host: String,
    port: u16,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let addr = format!("{}:{}", host, port);
    let stream = match TcpStream::connect(&addr).await {
        Ok(s) => {
            if let Err(e) = s.set_nodelay(true) {
                debug!(
                    "Failed to set nodelay on direct connection to {}: {}",
                    addr, e
                );
            }
            s
        }
        Err(e) => {
            error!("Failed to connect direct to target {}: {}", addr, e);
            let mut resp = Response::new(full(format!(
                "Failed to connect direct to target {}: {}",
                addr, e
            )));
            *resp.status_mut() = StatusCode::BAD_GATEWAY;
            return Ok(resp);
        }
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = http1::handshake(io).await?;

    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            error!("Connection failed: {:?}", err);
        }
    });

    // Rewrite URI to origin-form (relative path)
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let new_uri = format!("{}{}", path, query);

    if let Ok(new_uri) = new_uri.parse() {
        *req.uri_mut() = new_uri;
    } else {
        error!("Failed to parse new URI: {}", new_uri);
        let mut resp = Response::new(full("Internal Server Error: URI Parse Failed"));
        *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        return Ok(resp);
    }

    let resp = sender.send_request(req).await?;
    Ok(resp.map(|b| b.map_err(|e| e).boxed()))
}

async fn handle_upstream(
    req: Request<hyper::body::Incoming>,
    proxy_addr: String,
    target_addr: String,
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let addr = proxy_addr
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    // 1. Buffer Request Body
    let (parts, body) = req.into_parts();
    let body_bytes = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => {
            error!("Failed to read request body: {}", e);
            let mut resp = Response::new(full("Internal Server Error: Body Read Failed"));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(resp);
        }
    };

    let method = parts.method.clone();
    let uri = parts.uri.clone();
    let version = parts.version;
    let headers = parts.headers.clone();

    // 2. Connect
    let stream = match TcpStream::connect(addr).await {
        Ok(s) => {
            if let Err(e) = s.set_nodelay(true) {
                debug!(
                    "Failed to set nodelay on upstream connection to {}: {}",
                    addr, e
                );
            }
            s
        }
        Err(e) => {
            error!(
                "Failed to connect to upstream proxy {} for target {}: {}",
                addr, target_addr, e
            );
            let mut resp = Response::new(full(format!(
                "Failed to connect to upstream proxy {} for target {}: {}",
                addr, target_addr, e
            )));
            *resp.status_mut() = StatusCode::BAD_GATEWAY;
            return Ok(resp);
        }
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = http1::handshake(io).await?;

    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            error!("Upstream connection failed: {:?}", err);
        }
    });

    let mut auth_session = authenticator.as_ref().map(|a| a.create_session());
    let mut challenge: Option<String> = None;
    let mut retry_count = 0;

    // Pre-construct the base request outside the loop to avoid redundant allocations
    let mut builder = Request::builder().method(method).uri(uri).version(version);

    for (k, v) in headers.iter() {
        if k != "proxy-authorization" {
            builder = builder.header(k, v);
        }
    }

    // We use () for the base body since it allows the Request to be cloned easily
    let base_req = builder.body(()).unwrap();

    // Loop
    loop {
        retry_count += 1;
        if retry_count > 10 {
            error!("Too many authentication retries");
            let mut resp = Response::new(full("Upstream Authentication Loop"));
            *resp.status_mut() = StatusCode::BAD_GATEWAY;
            return Ok(resp);
        }

        // Reconstruct Request
        let mut req = base_req.clone();

        // Add Proxy-Authorization
        if let Some(session) = &mut auth_session {
            match session.step(challenge.as_deref()) {
                Ok(Some(h)) => {
                    if let Ok(val) = HeaderValue::from_str(&h) {
                        req.headers_mut()
                            .insert(hyper::header::PROXY_AUTHORIZATION, val);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    error!("Auth error: {}", e);
                    let mut resp = Response::new(full("Internal Server Error: Auth Failed"));
                    *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    return Ok(resp);
                }
            }
        }

        // Map the empty body () to the actual body bytes
        let req = req.map(|_| full(body_bytes.clone()));

        // Send Request
        let resp = match sender.send_request(req).await {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to send request to upstream: {}", e);
                // Can retry if connection closed? But for NTLM we must restart handshake.
                // Assuming fatal error.
                let mut resp = Response::new(full(format!("Upstream Error: {}", e)));
                *resp.status_mut() = StatusCode::BAD_GATEWAY;
                return Ok(resp);
            }
        };

        if resp.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
            // 407
            if auth_session.is_none() {
                return Ok(resp.map(|b| b.map_err(|e| e).boxed()));
            }

            // Extract Challenge
            if let Some(val) = resp.headers().get("proxy-authenticate") {
                if let Ok(s) = val.to_str() {
                    challenge = Some(s.to_string());
                } else {
                    challenge = None;
                }
            } else {
                challenge = None;
            }

            // Check if we should pass through 407 (e.g. auth failed after attempts)
            // If we got 407 and challenge is None, it's weird, but maybe pass through.
            if challenge.is_none() {
                return Ok(resp.map(|b| b.map_err(|e| e).boxed()));
            }

            // Drain body so we can reuse connection
            let mut body = resp.into_body();
            let mut drained_bytes = 0;
            let max_drain_bytes = 1024 * 1024 * 5; // 5 MB limit
            while let Some(Ok(frame)) =
                std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
            {
                if let Some(data) = frame.data_ref() {
                    drained_bytes += data.len();
                    if drained_bytes > max_drain_bytes {
                        break; // Stop draining to prevent unbounded memory/CPU usage
                    }
                }
            } // Ignore errors during drain

            // Continue loop
            continue;
        } else {
            // Success or other error
            return Ok(resp.map(|b| b.map_err(|e| e).boxed()));
        }
    }
}
