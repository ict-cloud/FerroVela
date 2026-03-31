use log::{debug, error, info, warn};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Sender;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod auth_tunnel;
pub mod http_utils;

pub const MAGIC_SHOW_REQUEST: &str =
    "GET /__ferrovela/show HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

const MAGIC_SHOW_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

#[derive(Debug, Clone)]
pub enum ProxySignal {
    Show,
}

/// Escapes a string for safe embedding inside a YAML double-quoted scalar.
///
/// YAML double-quoted scalars use backslash escape sequences (YAML 1.2 §7.3.1).
/// Without escaping, a value containing `"` or `\n` can break out of the scalar
/// and inject arbitrary YAML keys — a config-injection vulnerability.
fn yaml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
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

        // Reserve an available port for g3proxy's internal listener.
        // Brief race window between drop and g3proxy bind; negligible on loopback.
        let internal_port = {
            let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let port = probe.local_addr()?.port();
            drop(probe);
            port
        };

        // Build and write g3proxy YAML config.
        let yaml = self.build_g3proxy_yaml(internal_port);
        let config_path = std::env::temp_dir().join("ferrovela_g3proxy.yaml");
        std::fs::write(&config_path, &yaml)?;
        debug!(
            "g3proxy config written to {} (internal port {})",
            config_path.display(),
            internal_port
        );

        // Initialise g3-daemon global config path (one-time per process).
        g3_daemon::opts::validate_and_set_config_file(&config_path, "g3proxy")
            .map_err(|e| format!("g3proxy config file init failed: {e}"))?;

        // Parse YAML into g3proxy's global registries.
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

        // Decide whether to use the auth tunnel for this proxy configuration.
        // Kerberos and NTLM require a multi-step challenge-response that g3proxy's
        // ProxyHttp escaper does not support; we handle those connections ourselves.
        let use_auth_tunnel = self.needs_auth_tunnel();

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
                        handle_connection(
                            stream,
                            internal_port,
                            signal_sender,
                            if use_auth_tunnel { authenticator } else { None },
                            config,
                            pac,
                        )
                        .await;
                    });
                }
                Err(e) => {
                    error!("accept error: {}", e);
                }
            }
        }
    }

    /// Returns `true` when the configured auth type requires the pre-processor
    /// to drive the challenge-response handshake itself (Kerberos, NTLM).
    fn needs_auth_tunnel(&self) -> bool {
        self.config
            .upstream
            .as_ref()
            .map(|u| {
                matches!(u.auth_type.as_str(), "kerberos" | "mock_kerberos" | "ntlm")
                    && u.proxy_url.is_some()
                    && self.authenticator.is_some()
            })
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub async fn run_with_listener(
        &self,
        _listener: tokio::net::TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    /// Generates a minimal g3proxy YAML configuration from FerroVela's config.
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

        let addr = yaml_escape(
            proxy_url
                .trim_start_matches("http://")
                .trim_start_matches("https://"),
        );

        match upstream.auth_type.as_str() {
            "basic" => {
                let user = yaml_escape(upstream.username.as_deref().unwrap_or(""));
                let pass = yaml_escape(upstream.password.as_deref().unwrap_or(""));
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
            "kerberos" | "mock_kerberos" | "ntlm" => {
                // The auth tunnel in this process handles Kerberos/NTLM CONNECT traffic.
                // g3proxy is configured with DirectFixed so it only handles connections
                // that the pre-processor explicitly forwards to it (direct/exception paths
                // and plain-HTTP fallback).
                warn!(
                    "auth_type '{}': Kerberos/NTLM CONNECT tunnels are handled by the \
                     FerroVela pre-processor; g3proxy uses DirectFixed for direct paths",
                    upstream.auth_type
                );
                Self::direct_fixed_yaml()
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

// ─── connection dispatcher ────────────────────────────────────────────────────

/// Routes a single inbound client connection:
///
/// 1. Magic show request   → respond 200 OK + send `ProxySignal::Show`.
/// 2. Auth tunnel enabled  → [`auth_tunnel::handle_authenticated_tunnel`].
/// 3. Default              → splice directly to g3proxy's internal port.
async fn handle_connection(
    mut client: tokio::net::TcpStream,
    internal_port: u16,
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

    // Kerberos / NTLM: auth tunnel handler reads the request and drives the
    // challenge-response handshake with the upstream proxy itself.
    if let Some(auth) = authenticator {
        auth_tunnel::handle_authenticated_tunnel(client, internal_port, auth, config, pac).await;
        return;
    }

    // Default: forward raw bytes to g3proxy.
    let upstream_addr = format!("127.0.0.1:{}", internal_port);
    let mut upstream = match tokio::net::TcpStream::connect(&upstream_addr).await {
        Ok(s) => {
            let _ = s.set_nodelay(true);
            s
        }
        Err(e) => {
            error!("failed to connect to g3proxy at {}: {}", upstream_addr, e);
            return;
        }
    };

    match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
        Ok((down, up)) => debug!("connection closed ({} down, {} up bytes)", down, up),
        Err(e) => debug!("splice error: {}", e),
    }
}

// ─── proxy resolution (PAC / static config) ───────────────────────────────────

#[cfg(test)]
mod yaml_escape_tests {
    use super::yaml_escape;

    #[test]
    fn passthrough_normal_strings() {
        assert_eq!(yaml_escape("proxy.example.com:8080"), "proxy.example.com:8080");
        assert_eq!(yaml_escape("user@domain.com"), "user@domain.com");
        assert_eq!(yaml_escape(""), "");
    }

    #[test]
    fn escapes_double_quote() {
        // A quote without escaping would terminate the YAML scalar early.
        assert_eq!(yaml_escape(r#"pass"word"#), r#"pass\"word"#);
    }

    #[test]
    fn escapes_backslash() {
        assert_eq!(yaml_escape(r"C:\path"), r"C:\\path");
    }

    #[test]
    fn escapes_newline_and_carriage_return() {
        assert_eq!(yaml_escape("line1\nline2"), r"line1\nline2");
        assert_eq!(yaml_escape("line1\r\nline2"), r"line1\r\nline2");
    }

    #[test]
    fn injection_attempt_is_neutralised() {
        // Without escaping, a password containing `"` or `\n` would break out of the
        // YAML double-quoted scalar and inject arbitrary config keys.
        let malicious = "secret\"\n    injected_key: injected_value\n    x: \"";
        let escaped = yaml_escape(malicious);

        // No raw newlines remain — they are replaced with the two-char sequence `\n`.
        assert!(!escaped.contains('\n'));
        // Every `"` is preceded by `\` — no unescaped double-quotes remain.
        assert!(!escaped.contains("\"\n") && !escaped.starts_with('"'));

        // Exact expected output: backslash-escaped quotes and `\n` escape sequences.
        // In a raw string literal r#"..."#, `\"` is backslash+quote and `\n` is backslash+n.
        assert_eq!(
            escaped,
            r#"secret\"\n    injected_key: injected_value\n    x: \""#
        );
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
