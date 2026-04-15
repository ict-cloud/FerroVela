use base64::Engine as _;
use log::{debug, error, info, warn};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

use crate::auth::{create_authenticator, UpstreamAuthenticator};
use crate::config::Config;
use crate::pac::PacEngine;

pub mod auth_tunnel;
pub mod http_utils;
pub mod ssrf;

pub const MAGIC_SHOW_REQUEST: &str =
    "GET /__ferrovela/show HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";

const MAGIC_SHOW_PATH: &str = "/__ferrovela/show";

#[derive(Debug, Clone)]
pub enum ProxySignal {
    Show,
}

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
    signal_sender: Option<Sender<ProxySignal>>,
    /// Pre-computed Base64 of `"user:pass"` for Basic upstream auth.
    /// `None` when no Basic auth is configured.
    basic_auth_b64: Option<Arc<str>>,
}

// ─── extension types stored in Context during the CONNECT upgrade ─────────────

/// The resolved target and upstream proxy address, inserted by the CONNECT
/// responder so the upgrade handler can pick them up without re-running PAC.
#[derive(Clone, Debug)]
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

impl rama::Service<ProxyState, rama::http::Request> for ConnectResponder {
    type Response = (
        rama::http::Response,
        rama::Context<ProxyState>,
        rama::http::Request,
    );
    type Error = rama::http::Response;

    async fn serve(
        &self,
        mut ctx: rama::Context<ProxyState>,
        req: rama::http::Request,
    ) -> Result<Self::Response, Self::Error> {
        // CONNECT URI is authority-form: "host:port"
        let target = req
            .uri()
            .authority()
            .map(|a| a.as_str().to_owned())
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

        // Store routing decision in the context so the upgrade handler can use it.
        ctx.insert(ConnectRouting { target, proxy_addr });

        let response = rama::http::Response::builder()
            .status(rama::http::StatusCode::OK)
            .body(rama::http::Body::empty())
            .unwrap();
        Ok((response, ctx, req))
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
struct ConnectHandler;

impl rama::Service<ProxyState, rama::http::layer::upgrade::Upgraded> for ConnectHandler {
    type Response = ();
    type Error = Infallible;

    async fn serve(
        &self,
        ctx: rama::Context<ProxyState>,
        mut upgraded: rama::http::layer::upgrade::Upgraded,
    ) -> Result<(), Infallible> {
        let state = ctx.state();

        let routing = match ctx.get::<ConnectRouting>() {
            Some(r) => r.clone(),
            None => {
                error!("ConnectHandler: ConnectRouting missing from context");
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
                            let _ =
                                tokio::io::copy_bidirectional(&mut upgraded, &mut upstream).await;
                        }
                        Err(e) => {
                            error!("auth tunnel to {} via {}: {}", target, addr, e);
                        }
                    }
                } else {
                    // Unauthenticated tunnel through upstream proxy.
                    match tokio::net::TcpStream::connect(&addr).await {
                        Ok(mut upstream) => {
                            let _ = upstream.set_nodelay(true);
                            if auth_tunnel::send_connect_request(&mut upstream, target, None)
                                .await
                                .is_ok()
                            {
                                match auth_tunnel::read_connect_response(&mut upstream).await {
                                    Ok(200) => {
                                        let _ = tokio::io::copy_bidirectional(
                                            &mut upgraded,
                                            &mut upstream,
                                        )
                                        .await;
                                    }
                                    Ok(status) => {
                                        error!(
                                            "upstream returned {} for CONNECT {}",
                                            status, target
                                        );
                                    }
                                    Err(e) => error!("upstream response for {}: {}", target, e),
                                }
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
                        let _ = upstream.set_nodelay(true);
                        let _ = tokio::io::copy_bidirectional(&mut upgraded, &mut upstream).await;
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
/// - Magic show request (`GET /__ferrovela/show`) → 200 + signal.
/// - Upstream proxy configured → forward request, adding `Proxy-Authorization`
///   for Basic auth.
/// - No upstream (or exception) → direct connection, rewrites request to
///   origin-form.
async fn plain_http_handler(
    ctx: rama::Context<ProxyState>,
    req: rama::http::Request,
) -> Result<rama::http::Response, Infallible> {
    let state = ctx.state();

    // ── Magic IPC show request ─────────────────────────────────────────
    if req.uri().path() == MAGIC_SHOW_PATH {
        if let Some(sender) = &state.signal_sender {
            let _ = sender.send(ProxySignal::Show).await;
        }
        return Ok(rama::http::Response::builder()
            .status(rama::http::StatusCode::OK)
            .header("Content-Length", "0")
            .header("Connection", "close")
            .body(rama::http::Body::empty())
            .unwrap());
    }

    // ── Derive target host:port for proxy resolution ───────────────────
    // §9: Access URI components directly without cloning the whole Uri.
    // `host` becomes an owned String so the borrow on `req` is released
    // before `req` is moved into the forward helpers below.
    let host = req.uri().host().unwrap_or("").to_owned();
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

    let resolved = resolve_proxy(&target, &state.config, &state.pac).await;

    match resolved {
        Some(proxy_url) => {
            let upstream_addr = auth_tunnel::normalize_proxy_addr(&proxy_url);
            forward_plain_http_to_upstream(req, &upstream_addr, state.basic_auth_b64.as_deref())
                .await
        }
        None => {
            // SSRF guard for plain HTTP direct connections.
            if !state.config.proxy.allow_private_ips && ssrf::is_private_target(&target) {
                warn!("SSRF blocked: plain HTTP to private address {}", target);
                return Ok(forbidden());
            }
            forward_plain_http_direct(req, &target).await
        }
    }
}

/// Forward a plain HTTP request to an upstream proxy, injecting Basic auth.
///
/// `basic_auth_b64` is the pre-computed Base64 of `"user:pass"` (from
/// `ProxyState`); when `Some`, `Proxy-Authorization: Basic <b64>` is injected.
async fn forward_plain_http_to_upstream(
    req: rama::http::Request,
    upstream_addr: &str,
    basic_auth_b64: Option<&str>,
) -> Result<rama::http::Response, Infallible> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut upstream = match tokio::net::TcpStream::connect(upstream_addr).await {
        Ok(s) => {
            let _ = s.set_nodelay(true);
            s
        }
        Err(e) => {
            error!("plain HTTP: connect to upstream {}: {}", upstream_addr, e);
            return Ok(bad_gateway());
        }
    };

    // Serialize the request to wire format using the zero-allocation writer.
    let mut raw = Vec::with_capacity(2048);
    write_http_request(&mut raw, req, true, basic_auth_b64);

    if upstream.write_all(&raw).await.is_err() {
        return Ok(bad_gateway());
    }

    // 8 KiB initial capacity covers headers + most small response bodies
    // without triggering the realloc-doubling cascade from Vec::new().
    let mut buf = Vec::with_capacity(8 * 1024);
    let _ = upstream.read_to_end(&mut buf).await;
    parse_raw_response(buf)
}

/// Forward a plain HTTP request directly to the target server (origin-form).
async fn forward_plain_http_direct(
    req: rama::http::Request,
    target: &str,
) -> Result<rama::http::Response, Infallible> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut upstream = match tokio::net::TcpStream::connect(target).await {
        Ok(s) => {
            let _ = s.set_nodelay(true);
            s
        }
        Err(e) => {
            error!("plain HTTP: direct connect to {}: {}", target, e);
            return Ok(bad_gateway());
        }
    };

    let mut raw = Vec::with_capacity(2048);
    write_http_request(&mut raw, req, false, None);

    if upstream.write_all(&raw).await.is_err() {
        return Ok(bad_gateway());
    }

    let mut buf = Vec::with_capacity(8 * 1024);
    let _ = upstream.read_to_end(&mut buf).await;
    parse_raw_response(buf)
}

/// Serialise a rama `Request` to HTTP/1.1 wire format, writing directly into
/// the caller-supplied `Vec<u8>` via `extend_from_slice`.
///
/// - `absolute_form = true`  → absolute-form URI  (upstream proxy path)
/// - `absolute_form = false` → origin-form path   (direct connection path)
/// - `basic_auth_b64`        → when `Some`, appends `Proxy-Authorization: Basic <b64>\r\n`
///
/// No intermediate `String` allocations per header — all bytes are copied
/// directly from rama's owned header map into `out`.
fn write_http_request(
    out: &mut Vec<u8>,
    req: rama::http::Request,
    absolute_form: bool,
    basic_auth_b64: Option<&str>,
) {
    let (parts, _body) = req.into_parts();

    // Request line
    out.extend_from_slice(parts.method.as_str().as_bytes());
    out.push(b' ');
    if absolute_form {
        out.extend_from_slice(parts.uri.to_string().as_bytes());
    } else {
        let path = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        out.extend_from_slice(path.as_bytes());
    }
    out.extend_from_slice(b" HTTP/1.1\r\n");

    // Headers — zero allocations: name and value bytes come from rama's map.
    for (name, value) in &parts.headers {
        if let Ok(v) = value.to_str() {
            out.extend_from_slice(name.as_str().as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(v.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
    }

    // Inject pre-computed Basic Proxy-Authorization if provided.
    if let Some(b64) = basic_auth_b64 {
        out.extend_from_slice(b"Proxy-Authorization: Basic ");
        out.extend_from_slice(b64.as_bytes());
        out.extend_from_slice(b"\r\n");
    }

    out.extend_from_slice(b"\r\n");
}

/// Parse a raw HTTP/1.1 response buffer into a rama `Response`.
///
/// §2: Uses `drain` to strip header bytes from `buf` in-place (a `memmove`
/// within the existing allocation) rather than copying body bytes into a new
/// `Vec` — eliminates one heap allocation per plain-HTTP response.
fn parse_raw_response(mut buf: Vec<u8>) -> Result<rama::http::Response, Infallible> {
    // Parse the status code from the first line (UTF-8 borrow, no allocation).
    let status: u16 = {
        let raw_str = String::from_utf8_lossy(&buf);
        raw_str
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(502)
        // `raw_str` borrow is released here before buf is mutated below.
    };

    let status_code =
        rama::http::StatusCode::from_u16(status).unwrap_or(rama::http::StatusCode::BAD_GATEWAY);

    // Locate the header/body boundary using SIMD-accelerated memmem.
    let body_start = crate::proxy::http_utils::find_subsequence(&buf, b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(buf.len());

    // Drain header bytes in-place: body bytes shift to the front of `buf`
    // (one memmove within the same allocation, no new Vec).
    drop(buf.drain(..body_start));

    Ok(rama::http::Response::builder()
        .status(status_code)
        .body(rama::http::Body::from(buf))
        .unwrap())
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
    signal_sender: Option<Sender<ProxySignal>>,
    basic_auth_b64: Option<Arc<str>>,
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
            signal_sender,
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
    /// - Everything else reaches [`plain_http_handler`] (magic IPC, plain HTTP
    ///   forwarding).
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
            signal_sender: self.signal_sender.clone(),
            basic_auth_b64: self.basic_auth_b64.clone(),
        };

        let exec = Executor::default();

        let connect_responder = ConnectResponder {
            config: Arc::clone(&self.config),
            pac: Arc::clone(&self.pac),
        };

        let http_service = HttpServer::auto(exec).service(
            UpgradeLayer::new(MethodMatcher::CONNECT, connect_responder, ConnectHandler)
                .into_layer(service_fn(plain_http_handler)),
        );

        info!("Listening on http://{}", listen_addr);

        TcpListener::build_with_state(state)
            .bind(listen_addr)
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
            signal_sender: self.signal_sender.clone(),
            basic_auth_b64: self.basic_auth_b64.clone(),
        };

        let exec = Executor::default();

        let connect_responder = ConnectResponder {
            config: Arc::clone(&self.config),
            pac: Arc::clone(&self.pac),
        };

        let http_service = HttpServer::auto(exec).service(
            UpgradeLayer::new(MethodMatcher::CONNECT, connect_responder, ConnectHandler)
                .into_layer(service_fn(plain_http_handler)),
        );

        let std_listener = listener.into_std()?;
        TcpListener::try_from(std_listener)?
            .with_state(state)
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
