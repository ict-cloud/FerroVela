use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use log::{debug, error, info};
use tokio::net::TcpListener;
use tokio::sync::mpsc::Sender;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod connect;
pub mod nonconnect;

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
            let authenticator = self.authenticator.clone();
            let signal_sender = self.signal_sender.clone();

            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .preserve_header_case(true)
                    .title_case_headers(true)
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let config = config.clone();
                            let pac = pac.clone();
                            let authenticator = authenticator.clone();
                            let signal_sender = signal_sender.clone();
                            async move {
                                if req.uri().path() == "/__ferrovela/show" {
                                    if let Some(sender) = signal_sender {
                                        let _ = sender.send(ProxySignal::Show).await;
                                    }
                                    return Ok(Response::builder()
                                        .status(StatusCode::OK)
                                        .body(full("OK"))
                                        .unwrap());
                                }
                                proxy(req, config, pac, authenticator).await
                            }
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
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    if Method::CONNECT == req.method() {
        connect::handle(req, config, pac, authenticator).await
    } else {
        nonconnect::handle(req, config, pac, authenticator).await
    }
}

pub async fn resolve_proxy(
    target: &str,
    config: &Arc<Config>,
    pac: &Arc<Option<PacEngine>>,
) -> Option<String> {
    let host = target.split(':').next().unwrap_or(target);

    // Check Exceptions
    if let Some(exceptions) = &config.exceptions {
        if exceptions.exact.contains(host) {
            debug!("Exception matched exact host: {}, direct", host);
            return None;
        }
        for suffix in &exceptions.wildcards {
            if host.ends_with(suffix) {
                debug!("Exception matched glob: {}, direct", host);
                return None;
            }
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

pub fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::new().map_err(|never| match never {}).boxed()
}

pub fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into())
        .map_err(|never| match never {})
        .boxed()
}
