use log::{debug, error, info, warn};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Sender;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod http_utils;

pub const MAGIC_SHOW_PATH: &str = "/__ferrovela/show";
pub const MAGIC_SHOW_REQUEST: &str =
    "GET /__ferrovela/show HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

const MAGIC_SHOW_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

#[derive(Debug, Clone)]
pub enum ProxySignal {
    Show,
}

pub struct Proxy {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    #[allow(dead_code)]
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

        // Reserve an available port for g3proxy's internal listener before writing config.
        // There is a small race window between drop and g3proxy binding, but on loopback
        // this is negligible for a user application.
        let internal_port = {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let port = probe.local_addr()?.port();
            drop(probe);
            port
        };

        // Build g3proxy YAML config and write it to a temp file.
        let yaml = self.build_g3proxy_yaml(internal_port);
        let config_path = std::env::temp_dir().join("ferrovela_g3proxy.yaml");
        std::fs::write(&config_path, &yaml)?;
        debug!(
            "g3proxy config written to {} (internal port {})",
            config_path.display(),
            internal_port
        );

        // Point g3-daemon at our generated config file. This is a one-time global init.
        g3_daemon::opts::validate_and_set_config_file(&config_path, "g3proxy")
            .map_err(|e| format!("g3proxy config file init failed: {e}"))?;

        // Parse the YAML config into g3proxy's global registries.
        g3proxy::config::load().map_err(|e| format!("g3proxy config load failed: {e}"))?;

        // Spawn all sub-systems in dependency order.
        g3proxy::resolve::spawn_all()
            .await
            .map_err(|e| format!("g3proxy resolver spawn failed: {e}"))?;
        g3proxy::escape::load_all()
            .await
            .map_err(|e| format!("g3proxy escaper load failed: {e}"))?;
        g3proxy::auth::load_all()
            .await
            .map_err(|e| format!("g3proxy auth load failed: {e}"))?;
        g3proxy::audit::load_all()
            .await
            .map_err(|e| format!("g3proxy auditor load failed: {e}"))?;
        g3proxy::serve::spawn_offline_clean();
        g3proxy::serve::spawn_all()
            .await
            .map_err(|e| format!("g3proxy server spawn failed: {e}"))?;

        info!("g3proxy engine running on internal port {}", internal_port);

        // Our pre-processor listener sits on the configured port and handles two cases:
        //   1. Magic show request  → signal the UI and return 200 OK locally.
        //   2. Everything else     → pipe raw bytes to/from the g3proxy internal port.
        let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
        info!("Listening on http://{}", listen_addr);

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    debug!("accepted connection from {}", peer);
                    let signal_sender = self.signal_sender.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, internal_port, signal_sender).await;
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
        _listener: tokio::net::TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    /// Generates a minimal g3proxy YAML configuration.
    fn build_g3proxy_yaml(&self, internal_port: u16) -> String {
        let escaper_yaml = self.build_escaper_yaml();

        format!(
            r#"resolver:
  - name: default
    type: c-ares

escaper:
{escaper_yaml}
server:
  - name: proxy
    type: HttpProxy
    escaper: default
    listen: "127.0.0.1:{internal_port}"
"#,
        )
    }

    fn build_escaper_yaml(&self) -> String {
        let Some(upstream) = &self.config.upstream else {
            return Self::direct_fixed_yaml();
        };

        let Some(proxy_url) = &upstream.proxy_url else {
            return Self::direct_fixed_yaml();
        };

        // Parse proxy_url (expected: "host:port" or "http://host:port")
        let addr = proxy_url
            .trim_start_matches("http://")
            .trim_start_matches("https://");

        match upstream.auth_type.as_str() {
            "basic" => {
                let user = upstream.username.as_deref().unwrap_or("");
                let pass = upstream.password.as_deref().unwrap_or("");
                format!(
                    r#"  - name: default
    type: ProxyHttp
    proxy_addr: "{addr}"
    proxy_username: "{user}"
    proxy_password: "{pass}"
    resolver: default
"#,
                )
            }
            "ntlm" | "kerberos" | "mock_kerberos" => {
                // g3proxy's ProxyHttp escaper only supports Basic auth natively.
                // For NTLM/Kerberos we forward without pre-auth and log the limitation.
                warn!(
                    "auth_type '{}' is not natively supported by the g3proxy escaper; \
                     connections to authenticated upstreams may be rejected",
                    upstream.auth_type
                );
                format!(
                    r#"  - name: default
    type: ProxyHttp
    proxy_addr: "{addr}"
    resolver: default
"#,
                )
            }
            _ => format!(
                r#"  - name: default
    type: ProxyHttp
    proxy_addr: "{addr}"
    resolver: default
"#,
            ),
        }
    }

    fn direct_fixed_yaml() -> String {
        r#"  - name: default
    type: DirectFixed
    resolver: default
"#
        .to_string()
    }
}

/// Handles a single inbound TCP connection.
///
/// Peeks at the first bytes:
/// - Magic show request → respond 200 OK + forward ProxySignal::Show.
/// - Anything else      → splice bidirectionally with g3proxy's internal port.
async fn handle_connection(
    mut client: tokio::net::TcpStream,
    internal_port: u16,
    signal_sender: Option<Sender<ProxySignal>>,
) {
    let magic = MAGIC_SHOW_REQUEST.as_bytes();

    // Peek without consuming.
    let mut peek_buf = vec![0u8; magic.len()];
    let n = match client.peek(&mut peek_buf).await {
        Ok(n) => n,
        Err(e) => {
            debug!("peek error: {}", e);
            return;
        }
    };

    if n == magic.len() && peek_buf == magic {
        // Consume the magic request from the socket.
        let mut discard = vec![0u8; magic.len()];
        let _ = client.read_exact(&mut discard).await;

        if let Some(sender) = signal_sender {
            if let Err(e) = sender.send(ProxySignal::Show).await {
                debug!("signal send error: {}", e);
            }
        }

        let _ = client.write_all(MAGIC_SHOW_RESPONSE).await;
        return;
    }

    // Forward to g3proxy.
    let upstream_addr = format!("127.0.0.1:{}", internal_port);
    let mut upstream = match tokio::net::TcpStream::connect(&upstream_addr).await {
        Ok(s) => s,
        Err(e) => {
            error!("failed to connect to g3proxy at {}: {}", upstream_addr, e);
            return;
        }
    };

    match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
        Ok((down, up)) => {
            debug!("connection closed: {} bytes down, {} bytes up", down, up);
        }
        Err(e) => {
            debug!("splice error: {}", e);
        }
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
