use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::auth::mock_kerberos::MockKerberosAuthenticator;
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
