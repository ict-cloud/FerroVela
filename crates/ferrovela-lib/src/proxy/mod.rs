use base64::Engine as _;
use log::{debug, error, info, warn};
use rama::extensions::ExtensionsRef;
use std::convert::Infallible;
use std::sync::Arc;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod auth_tunnel;
pub mod ssrf;

/// Extracts `host:port` from a proxy URL.
///
/// Handles all URL forms:
/// - Strips scheme and userinfo (`http://user:pass@host:port` → `host:port`)
/// - Re-adds brackets for IPv6 literals (`http://[::1]:8080` → `[::1]:8080`)
/// - Falls back to the scheme's default port when no port is explicit
pub(crate) fn proxy_addr_from_url(proxy_url: &str) -> Option<String> {
    let u = url::Url::parse(proxy_url).ok()?;
    let port = u.port_or_known_default()?;
    match u.host()? {
        url::Host::Ipv6(addr) => Some(format!("[{}]:{}", addr, port)),
        host => Some(format!("{}:{}", host, port)),
    }
}

// ─── shared state threaded through rama's Context ─────────────────────────────

/// Application state available to all rama service handlers.
#[derive(Clone)]
struct ProxyState {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<dyn UpstreamAuthenticator>>,
    /// Pre-computed Base64 of `"user:pass"` for Basic upstream auth.
    /// `None` when no Basic auth is configured.
    basic_auth_b64: Option<Arc<str>>,
}

// ─── extension types stored in Context during the CONNECT upgrade ─────────────

/// The resolved target and upstream proxy address, inserted by the CONNECT
/// responder so the upgrade handler can pick them up without re-running PAC.
#[derive(Clone, Debug, rama::extensions::Extension)]
struct ConnectRouting {
    /// `host:port` the client wants to reach.
    target: String,
    /// `host:port` of the upstream proxy to use, or `None` for a direct
    /// connection.
    proxy_addr: Option<String>,
}

// ─── CONNECT responder ────────────────────────────────────────────────────────

/// Custom CONNECT responder that:
/// 1. Extracts the `host:port` from the CONNECT request URI.
/// 2. Resolves the upstream proxy via PAC / static config.
/// 3. Applies the SSRF guard for direct connections.
/// 4. Injects [`ConnectRouting`] into the context and returns `200`.
///
/// If any step fails the responder returns an error response (403/400) which
/// rama's `UpgradeLayer` sends to the client — the upgrade does NOT proceed.
#[derive(Clone)]
struct ConnectResponder {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
}

impl rama::Service<rama::http::Request> for ConnectResponder {
    type Output =
        rama::http::layer::upgrade::UpgradeResponse<rama::http::Request, rama::http::Response>;
    type Error = rama::http::Response;

    async fn serve(&self, req: rama::http::Request) -> Result<Self::Output, Self::Error> {
        // CONNECT URI is authority-form: "host:port"
        let target = req
            .uri()
            .authority()
            .map(|a| a.to_string())
            .ok_or_else(|| {
                warn!("CONNECT request missing authority");
                bad_request()
            })?;

        let proxy_addr = resolve_proxy(&target, &self.config, &self.pac).await;

        // SSRF guard: only applies when we would connect directly (no upstream proxy).
        if proxy_addr.is_none()
            && !self.config.proxy.allow_private_ips
            && ssrf::is_private_target(&target)
        {
            warn!("SSRF blocked: CONNECT to private address {}", target);
            return Err(forbidden());
        }

        let response = rama::http::Response::builder()
            .status(rama::http::StatusCode::OK)
            .body(rama::http::Body::empty())
            .unwrap();

        // Attach the routing decision to the upgraded connection's extensions so
        // the upgrade handler can pick it up without re-running PAC.
        Ok(
            rama::http::layer::upgrade::UpgradeResponse::new(req, response)
                .with_extension(ConnectRouting { target, proxy_addr }),
        )
    }
}

// ─── CONNECT upgrade handler ──────────────────────────────────────────────────

/// Handles the raw tunnel after a successful CONNECT negotiation.
///
/// Reads [`ConnectRouting`] from the context (set by [`ConnectResponder`]) and:
/// - Routes through the upstream proxy with authentication (NTLM/Kerberos/Basic),
/// - Routes through the upstream proxy without authentication,
/// - Or connects directly to the target.
///
/// Then copies bytes bidirectionally between the client's `Upgraded` socket and
/// the upstream `TcpStream`.
#[derive(Clone)]
struct ConnectHandler {
    state: ProxyState,
}

impl rama::Service<rama::http::layer::upgrade::Upgraded> for ConnectHandler {
    type Output = ();
    type Error = Infallible;

    async fn serve(
        &self,
        mut upgraded: rama::http::layer::upgrade::Upgraded,
    ) -> Result<(), Infallible> {
        let state = &self.state;

        let routing = match upgraded.extensions().get_ref::<ConnectRouting>() {
            Some(r) => r.clone(),
            None => {
                error!("ConnectHandler: ConnectRouting missing from extensions");
                return Ok(());
            }
        };

        let target = &routing.target;

        match routing.proxy_addr.as_deref() {
            Some(proxy_url) => {
                let addr = auth_tunnel::normalize_proxy_addr(proxy_url);
                if let Some(auth) = &state.authenticator {
                    // Authenticated tunnel (NTLM, Kerberos, Basic).
                    match auth_tunnel::perform_authenticated_connect(&addr, target, auth).await {
                        Ok(mut upstream) => {
                            let r =
                                tokio::io::copy_bidirectional(&mut upgraded, &mut upstream).await;
                            debug!("tunnel {} via {} closed: {:?}", target, addr, r);
                        }
                        Err(e) => {
                            error!("auth tunnel to {} via {}: {}", target, addr, e);
                        }
                    }
                } else {
                    // Unauthenticated tunnel through upstream proxy.
                    match tokio::net::TcpStream::connect(&addr).await {
                        Ok(mut upstream) => {
                            if let Err(e) = upstream.set_nodelay(true) {
                                debug!("set_nodelay failed: {}", e);
                            }
                            if let Err(e) =
                                auth_tunnel::send_connect(&mut upstream, target, None).await
                            {
                                error!("send CONNECT {} to upstream {}: {}", target, addr, e);
                                return Ok(());
                            }
                            match auth_tunnel::read_proxy_response(&mut upstream).await {
                                Ok((200, ..)) => {
                                    let r =
                                        tokio::io::copy_bidirectional(&mut upgraded, &mut upstream)
                                            .await;
                                    debug!("tunnel {} via {} closed: {:?}", target, addr, r);
                                }
                                Ok((status, ..)) => {
                                    error!("upstream returned {} for CONNECT {}", status, target);
                                }
                                Err(e) => error!("upstream response for {}: {}", target, e),
                            }
                        }
                        Err(e) => error!("connect to upstream {}: {}", addr, e),
                    }
                }
            }
            None => {
                // Direct TCP connect.
                match tokio::net::TcpStream::connect(target.as_str()).await {
                    Ok(mut upstream) => {
                        if let Err(e) = upstream.set_nodelay(true) {
                            debug!("set_nodelay failed: {}", e);
                        }
                        let r = tokio::io::copy_bidirectional(&mut upgraded, &mut upstream).await;
                        debug!("direct tunnel {} closed: {:?}", target, r);
                    }
                    Err(e) => error!("direct connect to {}: {}", target, e),
                }
            }
        }

        Ok(())
    }
}

// ─── plain HTTP handler ───────────────────────────────────────────────────────

/// Handles plain (non-CONNECT) HTTP requests.
///
/// - Upstream proxy configured → forward request in absolute-form, adding
///   `Proxy-Authorization` for Basic auth.
/// - No upstream (or exception) → direct connection in origin-form.
async fn plain_http_handler(
    state: ProxyState,
    mut req: rama::http::Request,
) -> Result<rama::http::Response, Infallible> {
    // ── Derive target host:port for proxy resolution ───────────────────
    // §9: Access URI components directly without cloning the whole Uri.
    // `host` becomes an owned String so the borrow on `req` is released
    // before `req` is moved into the forward helpers below.
    let host = req
        .uri()
        .host_str()
        .map(|h| h.into_owned())
        .unwrap_or_default();
    let port = req
        .uri()
        .port_u16()
        .or_else(|| {
            if req.uri().scheme_str() == Some("https") {
                Some(443)
            } else {
                Some(80)
            }
        })
        .unwrap_or(80);
    let target = if host.contains(':') {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    };

    if target.is_empty() || host.is_empty() {
        return Ok(bad_request());
    }

    let result = match resolve_proxy(&target, &state.config, &state.pac).await {
        Some(proxy_url) => {
            let upstream_addr = auth_tunnel::normalize_proxy_addr(&proxy_url);
            match format!("http://{upstream_addr}").parse() {
                Ok(proxy) => {
                    req.extensions()
                        .insert(rama::net::client::ProxyRoute::Proxy(proxy));
                }
                Err(e) => {
                    error!("plain HTTP: invalid upstream {}: {}", upstream_addr, e);
                    return Ok(bad_gateway());
                }
            }
            if let Some(b64) = state.basic_auth_b64.as_deref() {
                match format!("Basic {b64}").parse() {
                    Ok(value) => {
                        req.headers_mut().insert("proxy-authorization", value);
                    }
                    Err(e) => {
                        error!("plain HTTP: invalid Proxy-Authorization value: {}", e);
                        return Ok(bad_gateway());
                    }
                }
            }
            forward_plain_http(req, &upstream_addr).await
        }
        None => {
            // SSRF guard for plain HTTP direct connections.
            if !state.config.proxy.allow_private_ips && ssrf::is_private_target(&target) {
                warn!("SSRF blocked: plain HTTP to private address {}", target);
                return Ok(forbidden());
            }
            forward_plain_http(req, &target).await
        }
    };

    Ok(result.unwrap_or_else(|e| {
        error!("plain HTTP to {}: {}", target, e);
        bad_gateway()
    }))
}

/// Send `req` over a fresh HTTP/1.1 connection to `addr` (upstream proxy or
/// origin server) and stream the response back.
///
/// rama's encoder writes the request-target in origin-form, or in
/// absolute-form when the request carries an HTTP `ProxyRoute` extension.
async fn forward_plain_http(
    mut req: rama::http::Request,
    addr: &str,
) -> Result<rama::http::Response, Box<dyn std::error::Error + Send + Sync>> {
    let stream = tokio::net::TcpStream::connect(addr).await?;
    if let Err(e) = stream.set_nodelay(true) {
        debug!("plain HTTP: set_nodelay on {} failed: {}", addr, e);
    }
    *req.version_mut() = rama::http::Version::HTTP_11;

    let (mut sender, conn) =
        rama::http::core::client::conn::http1::handshake(rama::tcp::TcpStream::from(stream))
            .await?;
    let peer = addr.to_owned();
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            debug!("plain HTTP connection to {} ended: {}", peer, e);
        }
    });
    Ok(sender.send_request(req).await?.map(rama::http::Body::new))
}

// ─── response helpers ─────────────────────────────────────────────────────────

fn bad_request() -> rama::http::Response {
    rama::http::Response::builder()
        .status(rama::http::StatusCode::BAD_REQUEST)
        .header("Content-Length", "0")
        .body(rama::http::Body::empty())
        .unwrap()
}

fn forbidden() -> rama::http::Response {
    rama::http::Response::builder()
        .status(rama::http::StatusCode::FORBIDDEN)
        .header("Content-Length", "0")
        .body(rama::http::Body::empty())
        .unwrap()
}

fn bad_gateway() -> rama::http::Response {
    rama::http::Response::builder()
        .status(rama::http::StatusCode::BAD_GATEWAY)
        .header("Content-Length", "0")
        .body(rama::http::Body::empty())
        .unwrap()
}

// ─── Proxy struct ─────────────────────────────────────────────────────────────

pub struct Proxy {
    config: Arc<Config>,
    pac: Arc<Option<PacEngine>>,
    authenticator: Option<Arc<dyn UpstreamAuthenticator>>,
    basic_auth_b64: Option<Arc<str>>,
}

impl Proxy {
    pub fn new(config: Arc<Config>, pac: Option<PacEngine>) -> Self {
        let authenticator = if let Some(upstream_conf) = &config.upstream {
            create_authenticator(upstream_conf)
                .map(|b| -> Arc<dyn UpstreamAuthenticator> { Arc::from(b) })
        } else {
            None
        };

        // Pre-compute the Base64 credential string for Basic auth so that
        // `forward_plain_http_to_upstream` never performs crypto per request.
        let basic_auth_b64 = config
            .upstream
            .as_ref()
            .filter(|u| u.auth_type == "basic")
            .and_then(|u| {
                let user = u.username.as_deref().unwrap_or("");
                let pass = u.password.as_deref().unwrap_or("");
                if user.is_empty() {
                    return None;
                }
                let encoded = base64::prelude::BASE64_STANDARD.encode(format!("{}:{}", user, pass));
                Some(Arc::from(encoded.as_str()))
            });

        Proxy {
            config,
            pac: Arc::new(pac),
            authenticator,
            basic_auth_b64,
        }
    }

    /// Run the proxy using rama's HTTP/1.1 server stack.
    ///
    /// All connections flow through rama's `TcpListener` → `HttpServer` →
    /// `UpgradeLayer`:
    /// - `CONNECT` requests are intercepted by [`ConnectResponder`] (SSRF check,
    ///   PAC resolution) and handed off to [`ConnectHandler`] (auth tunnel or
    ///   direct TCP connect).
    /// - Everything else reaches [`plain_http_handler`] (plain HTTP forwarding).
    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use rama::{
            http::{layer::upgrade::UpgradeLayer, matcher::MethodMatcher, server::HttpServer},
            rt::Executor,
            service::service_fn,
            tcp::server::TcpListener,
            Layer,
        };

        if self.config.proxy.listen_ip != "127.0.0.1" && !self.config.proxy.allow_private_ips {
            warn!(
                "proxy_listen_ip={} is ignored because allow_private_ips is disabled; binding to 127.0.0.1",
                self.config.proxy.listen_ip
            );
        }
        let listen_addr = format!(
            "{}:{}",
            self.config.proxy.effective_listen_ip(),
            self.config.proxy.port
        );

        let state = ProxyState {
            config: Arc::clone(&self.config),
            pac: Arc::clone(&self.pac),
            authenticator: self.authenticator.clone(),
            basic_auth_b64: self.basic_auth_b64.clone(),
        };

        let exec = Executor::default();

        let connect_responder = ConnectResponder {
            config: Arc::clone(&self.config),
            pac: Arc::clone(&self.pac),
        };
        let connect_handler = ConnectHandler {
            state: state.clone(),
        };

        let http_service = HttpServer::auto(exec.clone()).service(
            UpgradeLayer::new_with_services(
                exec.clone(),
                MethodMatcher::CONNECT,
                connect_responder,
                connect_handler,
            )
            .into_layer(service_fn(move |req| {
                plain_http_handler(state.clone(), req)
            })),
        );

        info!("Listening on http://{}", listen_addr);

        TcpListener::build(exec)
            .bind_address(listen_addr)
            .await?
            .serve(http_service)
            .await;

        Ok(())
    }

    /// Run the proxy on an already-bound listener.
    ///
    /// The full rama pipeline (HTTP parsing, CONNECT upgrade, SSRF guard, PAC,
    /// authentication) applies exactly as in [`Proxy::run`].  The test suite
    /// uses this variant so it can bind to port 0 and discover the actual port
    /// before starting the proxy.
    #[cfg(test)]
    pub async fn run_with_listener(
        &self,
        listener: tokio::net::TcpListener,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use rama::{
            http::{layer::upgrade::UpgradeLayer, matcher::MethodMatcher, server::HttpServer},
            rt::Executor,
            service::service_fn,
            tcp::server::TcpListener,
            Layer,
        };

        let state = ProxyState {
            config: Arc::clone(&self.config),
            pac: Arc::clone(&self.pac),
            authenticator: self.authenticator.clone(),
            basic_auth_b64: self.basic_auth_b64.clone(),
        };

        let exec = Executor::default();

        let connect_responder = ConnectResponder {
            config: Arc::clone(&self.config),
            pac: Arc::clone(&self.pac),
        };
        let connect_handler = ConnectHandler {
            state: state.clone(),
        };

        let http_service = HttpServer::auto(exec.clone()).service(
            UpgradeLayer::new_with_services(
                exec.clone(),
                MethodMatcher::CONNECT,
                connect_responder,
                connect_handler,
            )
            .into_layer(service_fn(move |req| {
                plain_http_handler(state.clone(), req)
            })),
        );

        let std_listener = listener.into_std()?;
        TcpListener::try_from_std_tcp_listener(std_listener, exec)?
            .serve(http_service)
            .await;

        Ok(())
    }
}

// ─── proxy resolution (PAC / static config) ───────────────────────────────────

pub async fn resolve_proxy(
    target: &str,
    config: &Arc<Config>,
    pac: &Arc<Option<PacEngine>>,
) -> Option<String> {
    let host = target.split(':').next().unwrap_or(target);

    if let Some(exceptions) = &config.exceptions {
        if exceptions.matches(host) {
            debug!("Exception matched host: {}, direct", host);
            return None;
        }
    }

    if let Some(pac_engine) = &**pac {
        let url = format!("https://{}/", target);
        match pac_engine.find_proxy(&url, host).await {
            Ok(proxy_str) => {
                debug!("PAC returned: {}", proxy_str);
                let first = proxy_str.split(';').next().unwrap_or("").trim();
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

#[cfg(test)]
mod plain_http_tests {
    use super::*;
    use crate::config::{ProxyConfig, UpstreamConfig};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// One-shot server: returns what it received, answers with a header + body.
    async fn echo_server() -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let mut n = 0;
            while !String::from_utf8_lossy(&buf[..n]).ends_with("ping") {
                n += sock.read(&mut buf[n..]).await.unwrap();
            }
            sock.write_all(b"HTTP/1.1 201 Created\r\nX-Test: yes\r\nContent-Length: 4\r\n\r\npong")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (port, seen)
    }

    /// POST through the proxy; returns (request as seen by the server, response).
    async fn roundtrip(upstream: bool) -> (String, String) {
        let (port, seen) = echo_server().await;
        let config = Config {
            proxy: ProxyConfig {
                allow_private_ips: true,
                ..Default::default()
            },
            upstream: upstream.then(|| UpstreamConfig {
                auth_type: "basic".into(),
                username: Some("u".into()),
                password: Some("p".into()),
                use_keyring: false,
                proxy_url: Some(format!("http://127.0.0.1:{port}")),
                ..Default::default()
            }),
            exceptions: None,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_port = listener.local_addr().unwrap().port();
        let proxy = Proxy::new(Arc::new(config), None);
        tokio::spawn(async move { proxy.run_with_listener(listener).await });

        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        let req = format!(
            "POST http://127.0.0.1:{port}/x?y=1 HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping"
        );
        client.write_all(req.as_bytes()).await.unwrap();
        let mut resp = String::new();
        client.read_to_string(&mut resp).await.unwrap();
        (seen.await.unwrap().to_lowercase(), resp.to_lowercase())
    }

    #[tokio::test]
    async fn plain_http_forwards_body_and_response_headers() {
        let (seen, resp) = roundtrip(true).await;
        assert!(seen.starts_with("post http://127.0.0.1:"), "{seen}");
        assert!(seen.contains("/x?y=1 http/1.1"), "{seen}");
        assert!(seen.contains("proxy-authorization: basic dtpw"), "{seen}");
        assert!(resp.starts_with("http/1.1 201"), "{resp}");
        assert!(
            resp.contains("x-test: yes") && resp.ends_with("pong"),
            "{resp}"
        );

        let (seen, resp) = roundtrip(false).await;
        assert!(seen.starts_with("post /x?y=1 http/1.1"), "{seen}");
        assert!(!seen.contains("proxy-authorization"), "{seen}");
        assert!(
            resp.starts_with("http/1.1 201") && resp.ends_with("pong"),
            "{resp}"
        );
    }
}
