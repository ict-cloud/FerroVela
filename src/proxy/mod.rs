use bytes::Bytes;
use log::{debug, error, info};
use pingora::server::Server;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod http_utils;
pub mod shutdown;

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
    shutdown_rx: Option<tokio::sync::watch::Receiver<bool>>,
}

impl Proxy {
    pub fn new(
        config: Arc<Config>,
        pac: Option<PacEngine>,
        signal_sender: Option<Sender<ProxySignal>>,
        shutdown_rx: Option<tokio::sync::watch::Receiver<bool>>,
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
            shutdown_rx,
        }
    }

    pub fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = format!("127.0.0.1:{}", self.config.proxy.port);
        info!("Listening on http://{}", addr);

        // Start Pingora server
        let mut my_server = Server::new(None)?;
        my_server.bootstrap();

        let mut proxy_service = pingora::proxy::http_proxy_service(
            &my_server.configuration,
            FerroVelaProxy {
                config: self.config.clone(),
                pac: self.pac.clone(),
                authenticator: self.authenticator.clone(),
                signal_sender: self.signal_sender.clone(),
            },
        );

        proxy_service.add_tcp(&addr);
        my_server.add_service(proxy_service);
        #[cfg(unix)]
        let run_args = pingora::server::RunArgs {
            shutdown_signal: if let Some(rx) = &self.shutdown_rx {
                Box::new(shutdown::WatchShutdownSignal {
                    receiver: rx.clone(),
                })
            } else {
                Box::new(shutdown::WatchShutdownSignal {
                    receiver: tokio::sync::watch::channel(false).1,
                })
            },
        };

        #[cfg(windows)]
        let run_args = pingora::server::RunArgs::default();

        my_server.run(run_args);
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn run_with_listener(
        &self,
        _listener: tokio::net::TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
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

use async_trait::async_trait;
use pingora::proxy::{ProxyHttp, Session};
use pingora::upstreams::peer::HttpPeer;
use pingora::Result;

pub struct FerroVelaProxy {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<Box<dyn UpstreamAuthenticator>>>,
    signal_sender: Option<Sender<ProxySignal>>,
}

#[async_trait]
impl ProxyHttp for FerroVelaProxy {
    type CTX = ();

    fn new_ctx(&self) -> Self::CTX {}

    async fn upstream_peer(
        &self,
        session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let req = session.req_header();

        let target = if req.method == pingora::http::Method::CONNECT {
            req.uri.to_string()
        } else {
            let host = req.uri.host().unwrap_or("");
            let port = req.uri.port_u16().unwrap_or(80);
            format!("{}:{}", host, port)
        };

        if target.is_empty() || target == ":" {
            return Err(pingora::Error::explain(
                pingora::ErrorType::HTTPStatus(400),
                "Invalid target",
            ));
        }

        let upstream_proxy = resolve_proxy(&target, &self.config, &self.pac).await;

        if let Some(proxy_addr) = upstream_proxy {
            let proxy_addr = proxy_addr
                .trim_start_matches("http://")
                .trim_start_matches("https://");

            // Connect to upstream proxy
            let peer = HttpPeer::new(proxy_addr, false, target.clone());
            Ok(Box::new(peer))
        } else {
            // Direct connection
            let parts: Vec<&str> = target.split(':').collect();
            let host = parts[0];
            let port = parts
                .get(1)
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(80);
            let sni = host.to_string();

            let peer = HttpPeer::new(target.clone(), port == 443, sni);
            Ok(Box::new(peer))
        }
    }

    async fn request_filter(&self, session: &mut Session, _ctx: &mut Self::CTX) -> Result<bool> {
        if session.req_header().uri.path() == MAGIC_SHOW_PATH {
            if let Some(sender) = &self.signal_sender {
                let _ = sender.send(ProxySignal::Show).await;
            }
            let response = pingora::http::ResponseHeader::build(200, None).unwrap();
            session
                .write_response_header(Box::new(response), true)
                .await?;
            session
                .write_response_body(Some(Bytes::from("OK")), true)
                .await?;
            return Ok(true);
        }

        Ok(false)
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut pingora::http::RequestHeader,
        _ctx: &mut Self::CTX,
    ) -> Result<()> {
        if let Some(authenticator) = &self.authenticator {
            let mut auth_session = authenticator.create_session();
            if let Ok(Some(header)) = auth_session.step(None) {
                let _ = upstream_request.insert_header("Proxy-Authorization", header);
            }
        }
        Ok(())
    }
}
