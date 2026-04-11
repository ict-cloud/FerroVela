use log::{debug, error, info};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Sender;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod auth_tunnel;
pub mod http_utils;
pub mod ssrf;

pub const MAGIC_SHOW_REQUEST: &str =
    "GET /__ferrovela/show HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

const MAGIC_SHOW_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

#[derive(Debug, Clone)]
pub enum ProxySignal {
    Show,
}

/// Extracts `host:port` from a proxy URL.
///
/// Handles all URL forms correctly:
/// - Strips scheme and userinfo (`http://user:pass@host:port` → `host:port`)
/// - Re-adds brackets for IPv6 literals (`http://[::1]:8080` → `[::1]:8080`)
/// - Falls back to the scheme's default port when no port is explicit
///
/// Returns `None` if the URL cannot be parsed or has no host.
pub(crate) fn proxy_addr_from_url(proxy_url: &str) -> Option<String> {
    let u = url::Url::parse(proxy_url).ok()?;
    let port = u.port_or_known_default()?;
    match u.host()? {
        url::Host::Ipv6(addr) => Some(format!("[{}]:{}", addr, port)),
        host => Some(format!("{}:{}", host, port)),
    }
}

pub struct Proxy {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<dyn UpstreamAuthenticator>>,
    signal_sender: Option<Sender<ProxySignal>>,
}

impl Proxy {
    pub fn new(
        config: Arc<Config>,
        pac: Option<PacEngine>,
        signal_sender: Option<Sender<ProxySignal>>,
    ) -> Self {
        let authenticator = if let Some(upstream_conf) = &config.upstream {
            create_authenticator(upstream_conf)
                .map(|b| -> Arc<dyn UpstreamAuthenticator> { Arc::from(b) })
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
        let listen_addr = format!("127.0.0.1:{}", self.config.proxy.port);
        let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
        info!("Listening on http://{}", listen_addr);

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    debug!("accepted connection from {}", peer);
                    let _ = stream.set_nodelay(true);
                    let signal_sender = self.signal_sender.clone();
                    let authenticator = self.authenticator.clone();
                    let config = Arc::clone(&self.config);
                    let pac = Arc::clone(&self.pac);

                    tokio::spawn(async move {
                        handle_connection(stream, signal_sender, authenticator, config, pac).await;
                    });
                }
                Err(e) => {
                    error!("accept error: {}", e);
                }
            }
        }
    }

    #[cfg(test)]
    pub async fn run_with_listener(
        &self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let upstream_addr = self
            .config
            .upstream
            .as_ref()
            .and_then(|u| u.proxy_url.clone());

        loop {
            match listener.accept().await {
                Ok((mut client, _)) => {
                    let addr = upstream_addr.clone();
                    tokio::spawn(async move {
                        if let Some(addr) = addr {
                            if let Ok(mut upstream) = tokio::net::TcpStream::connect(&addr).await {
                                let _ =
                                    tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                            }
                        }
                    });
                }
                Err(_) => break,
            }
        }
        Ok(())
    }
}

// ─── connection dispatcher ────────────────────────────────────────────────────

/// Routes a single inbound client connection:
///
/// 1. Magic show request  → respond 200 OK + send `ProxySignal::Show`.
/// 2. Everything else     → [`auth_tunnel::handle_authenticated_tunnel`].
async fn handle_connection(
    mut client: tokio::net::TcpStream,
    signal_sender: Option<Sender<ProxySignal>>,
    authenticator: Option<Arc<dyn UpstreamAuthenticator>>,
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
) {
    const MAGIC_LEN: usize = MAGIC_SHOW_REQUEST.len();
    let magic = MAGIC_SHOW_REQUEST.as_bytes();

    // Peek without consuming — cheap way to detect the IPC magic request.
    let mut peek_buf = [0u8; MAGIC_LEN];
    let n = match client.peek(&mut peek_buf).await {
        Ok(n) => n,
        Err(e) => {
            debug!("peek error: {}", e);
            return;
        }
    };

    if n == MAGIC_LEN && peek_buf == magic {
        // Consume the magic bytes.
        let mut discard = [0u8; MAGIC_LEN];
        let _ = client.read_exact(&mut discard).await;

        if let Some(sender) = signal_sender {
            if let Err(e) = sender.send(ProxySignal::Show).await {
                debug!("signal send error: {}", e);
            }
        }
        let _ = client.write_all(MAGIC_SHOW_RESPONSE).await;
        return;
    }

    // Auth tunnel handler reads headers and dispatches all request types.
    auth_tunnel::handle_authenticated_tunnel(client, authenticator, config, pac).await;
}

// ─── proxy resolution (PAC / static config) ───────────────────────────────────

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
mod proxy_addr_tests {
    use super::proxy_addr_from_url;

    #[test]
    fn standard_http_url() {
        assert_eq!(
            proxy_addr_from_url("http://proxy.corp.com:8080"),
            Some("proxy.corp.com:8080".to_string())
        );
    }

    #[test]
    fn strips_userinfo() {
        assert_eq!(
            proxy_addr_from_url("http://user:secret@proxy.corp.com:8080"),
            Some("proxy.corp.com:8080".to_string())
        );
    }

    #[test]
    fn ipv6_gets_brackets() {
        assert_eq!(
            proxy_addr_from_url("http://[::1]:3128"),
            Some("[::1]:3128".to_string())
        );
    }

    #[test]
    fn default_port_for_https() {
        assert_eq!(
            proxy_addr_from_url("https://proxy.corp.com"),
            Some("proxy.corp.com:443".to_string())
        );
    }

    #[test]
    fn default_port_for_http() {
        assert_eq!(
            proxy_addr_from_url("http://proxy.corp.com"),
            Some("proxy.corp.com:80".to_string())
        );
    }

    #[test]
    fn invalid_url_returns_none() {
        assert_eq!(proxy_addr_from_url("not a url"), None);
        assert_eq!(proxy_addr_from_url(""), None);
    }
}
