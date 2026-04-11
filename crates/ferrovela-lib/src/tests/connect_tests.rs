use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::auth::mock_kerberos::MockKerberosAuthenticator;
use crate::auth::ntlm::NtlmAuthenticator;
use crate::auth::UpstreamAuthenticator;
use crate::proxy::auth_tunnel::{perform_authenticated_connect, read_http_headers};

// ── read_http_headers ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_read_http_headers_valid() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        server
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                  Proxy-Authenticate: Negotiate\r\n\
                  Content-Length: 0\r\n\r\n",
            )
            .await
            .unwrap();
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    let result = read_http_headers(&mut client).await.unwrap();
    assert!(result.contains("407"));
    assert!(result.contains("Proxy-Authenticate: Negotiate"));
}

#[tokio::test]
async fn test_read_http_headers_connection_closed_early() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        // Partial headers — no terminating \r\n\r\n
        server.write_all(b"HTTP/1.1 200 OK\r\n").await.unwrap();
        drop(server);
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    let result = read_http_headers(&mut client).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("connection closed"));
}

#[tokio::test]
async fn test_read_http_headers_exceeds_max_size() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        // Write more than MAX_HEADER_BYTES (64 KB) without \r\n\r\n
        let huge_data = vec![b'A'; 70 * 1024];
        let _ = server.write_all(&huge_data).await;
        drop(server);
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    let result = read_http_headers(&mut client).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("exceeded maximum size"));
}

// ── perform_authenticated_connect ─────────────────────────────────────────────

/// Mock proxy accepts the initial unauthenticated CONNECT with 200.
#[tokio::test]
async fn test_perform_authenticated_connect_direct_200() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = server.read(&mut buf).await;
        let _ = server
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await;
    });

    let auth: Arc<dyn UpstreamAuthenticator> = Arc::new(MockKerberosAuthenticator::new());
    let result = perform_authenticated_connect(&addr.to_string(), "example.com:443", &auth).await;
    assert!(result.is_ok());
}

/// Mock proxy challenges with 407 Negotiate, then accepts with 200.
#[tokio::test]
async fn test_perform_authenticated_connect_407_then_200() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];

        // Round 0: unauthenticated CONNECT → 407
        let _ = server.read(&mut buf).await;
        let _ = server
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                  Proxy-Authenticate: Negotiate\r\n\
                  Content-Length: 0\r\n\r\n",
            )
            .await;

        // Round 1: authenticated CONNECT → 200
        buf.fill(0);
        let _ = server.read(&mut buf).await;
        let _ = server
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await;
    });

    let auth: Arc<dyn UpstreamAuthenticator> = Arc::new(MockKerberosAuthenticator::new());
    let result = perform_authenticated_connect(&addr.to_string(), "example.com:443", &auth).await;
    assert!(result.is_ok());
}

/// Mock proxy always returns 407 — auth loop should exhaust and return an error.
#[tokio::test]
async fn test_perform_authenticated_connect_exhausted_407() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut server, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            loop {
                match server.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        buf.fill(0);
                        if server
                            .write_all(
                                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                                  Proxy-Authenticate: Negotiate\r\n\
                                  Content-Length: 0\r\n\r\n",
                            )
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        }
    });

    let auth: Arc<dyn UpstreamAuthenticator> = Arc::new(MockKerberosAuthenticator::new());
    let result = perform_authenticated_connect(&addr.to_string(), "example.com:443", &auth).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("exhausted") || err.contains("407"),
        "unexpected error: {err}"
    );
}

/// Build a minimal but spec-compliant NTLM Type 2 (Challenge) message.
///
/// Duplicated from `auth_tests` so this module is self-contained.
fn build_ntlm_type2_challenge(server_challenge: [u8; 8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(56);
    msg.extend_from_slice(b"NTLMSSP\0");
    msg.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // MessageType = 2
    msg.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x38, 0x00, 0x00, 0x00]); // TargetNameFields
    msg.extend_from_slice(&[0x01, 0x02, 0x00, 0x00]); // NegotiateFlags
    msg.extend_from_slice(&server_challenge);
    msg.extend_from_slice(&[0x00; 8]); // Reserved
    msg.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x38, 0x00, 0x00, 0x00]); // TargetInfoFields
    msg.extend_from_slice(&[0x00; 8]); // Version (required by ntlmclient)
    msg
}

/// Verifies that the entire NTLM 3-step handshake uses a single TCP connection.
///
/// NTLM authentication is stateful: the server's Type 2 challenge is tied to the
/// specific TCP session.  If the client opened a new connection for each round
/// the upstream proxy would see a new session and the handshake would fail.
///
/// The mock proxy accepts exactly one TCP connection and drives the exchange:
/// - Round 0: unauthenticated CONNECT → 407 + Proxy-Authenticate: NTLM
/// - Round 1: CONNECT + Type1       → 407 + Proxy-Authenticate: NTLM <Type2>
/// - Round 2: CONNECT + Type3       → 200 Connection established
///
/// Assertion: `accept()` was called exactly once.
#[tokio::test]
async fn test_ntlm_uses_same_tcp_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let type2_bytes = build_ntlm_type2_challenge([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    let type2_b64 = base64::prelude::BASE64_STANDARD.encode(&type2_bytes);
    let type2_header = format!("NTLM {type2_b64}");

    let accept_count = Arc::new(AtomicUsize::new(0));
    let accept_count_srv = Arc::clone(&accept_count);

    tokio::spawn(async move {
        // The mock proxy must accept exactly ONE connection for NTLM to work.
        let (mut server, _) = listener.accept().await.unwrap();
        accept_count_srv.fetch_add(1, Ordering::SeqCst);

        let mut buf = [0u8; 4096];

        // Round 0: no auth → 407 NTLM (no token)
        let _ = server.read(&mut buf).await;
        let _ = server
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                  Proxy-Authenticate: NTLM\r\n\
                  Content-Length: 0\r\n\r\n",
            )
            .await;

        // Round 1: CONNECT + Type1 → 407 NTLM <Type2 challenge>
        buf.fill(0);
        let _ = server.read(&mut buf).await;
        let challenge_response = format!(
            "HTTP/1.1 407 Proxy Authentication Required\r\n\
             Proxy-Authenticate: {type2_header}\r\n\
             Content-Length: 0\r\n\r\n"
        );
        let _ = server.write_all(challenge_response.as_bytes()).await;

        // Round 2: CONNECT + Type3 → 200
        buf.fill(0);
        let _ = server.read(&mut buf).await;
        let _ = server
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await;
    });

    let auth: Arc<dyn UpstreamAuthenticator> = Arc::new(NtlmAuthenticator::new(
        "user".into(),
        "pass".into(),
        "DOMAIN".into(),
        "WORKSTATION".into(),
    ));

    let result = perform_authenticated_connect(&addr.to_string(), "example.com:443", &auth).await;

    assert!(result.is_ok(), "NTLM handshake failed: {:?}", result.err());
    assert_eq!(
        accept_count.load(Ordering::SeqCst),
        1,
        "NTLM handshake must reuse a single TCP connection across all three rounds"
    );
}

/// Mock proxy returns a non-200/non-407 status (e.g. 503) — should return an error.
#[tokio::test]
async fn test_perform_authenticated_connect_unexpected_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut server, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        // Initial unauthenticated CONNECT → 407 (to enter the auth loop)
        let _ = server.read(&mut buf).await;
        let _ = server
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                  Proxy-Authenticate: Negotiate\r\n\
                  Content-Length: 0\r\n\r\n",
            )
            .await;

        // First auth attempt → 503
        buf.fill(0);
        let _ = server.read(&mut buf).await;
        let _ = server
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
            .await;
    });

    let auth: Arc<dyn UpstreamAuthenticator> = Arc::new(MockKerberosAuthenticator::new());
    let result = perform_authenticated_connect(&addr.to_string(), "example.com:443", &auth).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("503"), "unexpected error: {err}");
}
