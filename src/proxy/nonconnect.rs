use std::sync::Arc;

use base64::prelude::*;
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use hyper::client::conn::http1;
use hyper::header::HeaderValue;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, error};
use tokio::net::TcpStream;

use crate::config::Config;
use crate::pac::PacEngine;
use crate::proxy::{full, resolve_proxy};

pub async fn handle(
    req: Request<hyper::body::Incoming>,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
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
        handle_upstream(req, proxy_addr, config).await
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
        Ok(s) => s,
        Err(e) => {
            error!("Failed to connect to {}: {}", addr, e);
            let mut resp = Response::new(full(format!("Failed to connect: {}", e)));
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
    let query = req.uri().query().map(|q| format!("?{}", q)).unwrap_or_default();
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
    mut req: Request<hyper::body::Incoming>,
    proxy_addr: String,
    config: Arc<Config>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    let addr = proxy_addr
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    let stream = match TcpStream::connect(addr).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to connect to upstream {}: {}", addr, e);
            let mut resp = Response::new(full(format!("Failed to connect to upstream: {}", e)));
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

    // Add Auth Headers
    if let Some(upstream_conf) = &config.upstream {
        if upstream_conf.auth_type == "basic" {
             if let (Some(u), Some(p)) = (&upstream_conf.username, &upstream_conf.password) {
                 let creds = format!("{}:{}", u, p);
                 let encoded = BASE64_STANDARD.encode(creds);
                 let val = format!("Basic {}", encoded);
                 if let Ok(header_val) = HeaderValue::from_str(&val) {
                    req.headers_mut().insert(hyper::header::PROXY_AUTHORIZATION, header_val);
                 }
             }
        }
    }

    let resp = sender.send_request(req).await?;
    Ok(resp.map(|b| b.map_err(|e| e).boxed()))
}
