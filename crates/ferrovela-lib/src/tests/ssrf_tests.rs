/// Integration tests for SSRF enforcement.
///
/// These tests spin up a real TCP listener acting as a mock proxy client and
/// verify that the auth tunnel rejects CONNECT requests to private addresses
/// with 403 Forbidden, while allowing connections to public addresses.
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::{Config, ProxyConfig};
use crate::pac::PacEngine;
use crate::proxy::auth_tunnel::handle_authenticated_tunnel;

// ── helpers ───────────────────────────────────────────────────────────────────

fn config_allow_private(allow: bool) -> Arc<Config> {
    Arc::new(Config {
        proxy: ProxyConfig {
            allow_private_ips: allow,
            ..Default::default()
        },
        upstream: None,
        exceptions: None,
    })
}

/// Spin up the auth tunnel handler for a single connection, send `request`,
/// and return the full response bytes.
async fn tunnel_response(request: &[u8], allow_private_ips: bool) -> Vec<u8> {
    // Internal g3proxy port — bind a listener so the address is valid, but the
    // SSRF check fires before any connection reaches it.
    let g3_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let internal_port = g3_listener.local_addr().unwrap().port();

    // Listener that plays the role of the proxy client.
    let outer = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let outer_addr = outer.local_addr().unwrap();

    let config = config_allow_private(allow_private_ips);
    let pac: Arc<Option<PacEngine>> = Arc::new(None);

    // Spawn the handler.
    let config_c = Arc::clone(&config);
    let pac_c = Arc::clone(&pac);
    tokio::spawn(async move {
        let (conn, _) = outer.accept().await.unwrap();
        // No authenticator → direct-connect path, which is where the SSRF guard lives.
        handle_authenticated_tunnel(conn, internal_port, {
            // We need *something* for the authenticator parameter but the direct
            // CONNECT path doesn't use it; use a mock.
            use crate::auth::mock_kerberos::MockKerberosAuthenticator;
            use crate::auth::UpstreamAuthenticator;
            Arc::from(Box::new(MockKerberosAuthenticator) as Box<dyn UpstreamAuthenticator>)
        }, config_c, pac_c).await;
    });

    let mut client = TcpStream::connect(outer_addr).await.unwrap();
    client.write_all(request).await.unwrap();

    let mut response = Vec::new();
    // Read until the connection is closed.
    let _ = client.read_to_end(&mut response).await;
    response
}

// ── SSRF guard in auth tunnel ─────────────────────────────────────────────────

#[tokio::test]
async fn direct_connect_to_loopback_is_blocked() {
    let req = b"CONNECT 127.0.0.1:8080 HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n";
    let resp = tunnel_response(req, false).await;
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(
        resp_str.starts_with("HTTP/1.1 403"),
        "expected 403, got: {resp_str}"
    );
}

#[tokio::test]
async fn direct_connect_to_rfc1918_is_blocked() {
    for target in &["10.0.0.1:80", "172.16.0.1:80", "192.168.1.1:80"] {
        let req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
        let resp = tunnel_response(req.as_bytes(), false).await;
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(
            resp_str.starts_with("HTTP/1.1 403"),
            "expected 403 for {target}, got: {resp_str}"
        );
    }
}

#[tokio::test]
async fn direct_connect_to_link_local_is_blocked() {
    // 169.254.169.254 is the cloud metadata endpoint commonly targeted in SSRF.
    let req = b"CONNECT 169.254.169.254:80 HTTP/1.1\r\nHost: 169.254.169.254\r\n\r\n";
    let resp = tunnel_response(req, false).await;
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(
        resp_str.starts_with("HTTP/1.1 403"),
        "expected 403, got: {resp_str}"
    );
}

#[tokio::test]
async fn direct_connect_to_private_allowed_when_flag_set() {
    // With allow_private_ips = true the guard must not block; the connection
    // will fail because nothing listens on 10.0.0.1, but the response should
    // be 502, not 403.
    let req = b"CONNECT 10.0.0.1:80 HTTP/1.1\r\nHost: 10.0.0.1:80\r\n\r\n";
    let resp = tunnel_response(req, true).await;
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(
        !resp_str.starts_with("HTTP/1.1 403"),
        "expected connection attempt (not 403) when allow_private_ips=true, got: {resp_str}"
    );
}

// ── unit tests for the IP classification logic ────────────────────────────────

#[test]
fn ssrf_module_blocks_private_ips() {
    use crate::proxy::ssrf::is_private_target;

    // Blocked
    assert!(is_private_target("127.0.0.1:80"));
    assert!(is_private_target("10.1.2.3:443"));
    assert!(is_private_target("172.20.0.1:80"));
    assert!(is_private_target("192.168.0.1:80"));
    assert!(is_private_target("169.254.169.254:80"));
    assert!(is_private_target("0.0.0.0:80"));
    assert!(is_private_target("[::1]:443"));
    assert!(is_private_target("[fc00::1]:80"));
    assert!(is_private_target("[fe80::1]:80"));

    // Allowed
    assert!(!is_private_target("1.1.1.1:443"));
    assert!(!is_private_target("8.8.8.8:53"));
    assert!(!is_private_target("example.com:443"));
}
