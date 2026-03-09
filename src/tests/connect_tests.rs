use crate::config::{Config, ProxyConfig};
use crate::proxy::{Proxy, ProxySignal, MAGIC_SHOW_REQUEST};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Helper to create a proxy on an OS-assigned port and return
/// (proxy_abort_handle, bound_port).
async fn start_test_proxy(
    signal_sender: Option<tokio::sync::mpsc::Sender<ProxySignal>>,
) -> (tokio::task::JoinHandle<()>, u16) {
    let config = Arc::new(Config {
        proxy: ProxyConfig {
            port: 0,
            pac_file: None,
            allow_private_ips: false,
        },
        upstream: None,
        exceptions: None,
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound_port = listener.local_addr().unwrap().port();

    let proxy = Proxy::new(config, None, signal_sender);

    let handle = tokio::spawn(async move {
        let _ = proxy.run_with_listener(listener).await;
    });

    // Give the accept loop a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (handle, bound_port)
}

/// When the service toggle is activated, the proxy must bind to the
/// configured port (or a free port when port == 0) on localhost and
/// accept incoming TCP connections.
#[tokio::test]
async fn test_service_toggle_exposes_port() {
    let (handle, port) = start_test_proxy(None).await;

    // The port must be reachable
    let result = TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(
        result.is_ok(),
        "Proxy should accept connections on port {} after toggle, but got: {:?}",
        port,
        result.err()
    );

    handle.abort();
}

/// Multiple sequential connections to the proxy must all succeed,
/// proving the accept loop keeps running after serving one client.
#[tokio::test]
async fn test_proxy_accepts_multiple_connections() {
    let (handle, port) = start_test_proxy(None).await;

    for i in 0..3 {
        let result = TcpStream::connect(format!("127.0.0.1:{}", port)).await;
        assert!(
            result.is_ok(),
            "Connection {} to proxy should succeed, got: {:?}",
            i,
            result.err()
        );
        // Drop the stream immediately so the proxy handler finishes
        drop(result);
    }

    handle.abort();
}

/// Sending the magic show IPC request must return an HTTP 200 OK
/// response and dispatch a `ProxySignal::Show` on the signal channel.
#[tokio::test]
async fn test_magic_show_request_returns_200_and_signals() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let (handle, port) = start_test_proxy(Some(tx)).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Should connect to proxy");

    stream
        .write_all(MAGIC_SHOW_REQUEST.as_bytes())
        .await
        .expect("Should send magic request");

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.expect("Should read response");
    let response = String::from_utf8_lossy(&buf[..n]);

    assert!(
        response.contains("200 OK"),
        "Expected HTTP 200 OK for magic show request, got: {}",
        response
    );

    // The proxy must have sent a Show signal
    let signal = rx.try_recv();
    assert!(
        signal.is_ok(),
        "Expected ProxySignal::Show on the channel after magic request"
    );

    handle.abort();
}

/// A malformed request must receive a 400 Bad Request and not crash
/// the proxy.
#[tokio::test]
async fn test_malformed_request_returns_400() {
    let (handle, port) = start_test_proxy(None).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Should connect to proxy");

    stream
        .write_all(b"NOT_HTTP\r\n\r\n")
        .await
        .expect("Should send garbage");

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.expect("Should read response");
    let response = String::from_utf8_lossy(&buf[..n]);

    assert!(
        response.contains("400 Bad Request"),
        "Expected 400 for malformed request, got: {}",
        response
    );

    // Proxy must still be alive for the next connection
    let probe = TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(probe.is_ok(), "Proxy must keep running after a bad request");

    handle.abort();
}

/// An HTTP GET proxied through the proxy in DIRECT mode (no upstream)
/// must reach the origin server and relay its response back to the
/// client.
#[tokio::test]
async fn test_http_get_proxy_direct() {
    // Spin up a tiny origin server
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_port = origin.local_addr().unwrap().port();

    let origin_handle = tokio::spawn(async move {
        loop {
            if let Ok((mut s, _)) = origin.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let body = "origin-ok";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        }
    });

    let (proxy_handle, proxy_port) = start_test_proxy(None).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .expect("Should connect to proxy");

    let req = format!(
        "GET http://127.0.0.1:{}/hello HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        origin_port, origin_port
    );
    stream.write_all(req.as_bytes()).await.unwrap();

    let mut resp_buf = Vec::new();
    let _ = stream.read_to_end(&mut resp_buf).await;
    let response = String::from_utf8_lossy(&resp_buf);

    assert!(
        response.contains("200 OK"),
        "Expected proxied 200 OK, got: {}",
        response
    );
    assert!(
        response.contains("origin-ok"),
        "Expected origin body through proxy, got: {}",
        response
    );

    proxy_handle.abort();
    origin_handle.abort();
}

/// An HTTP CONNECT request must establish a bidirectional tunnel to
/// the target server and relay data in both directions.
#[tokio::test]
async fn test_connect_tunnel_echo() {
    // Spin up a TCP echo server (target behind the CONNECT tunnel)
    let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo.local_addr().unwrap().port();

    let echo_handle = tokio::spawn(async move {
        if let Ok((mut s, _)) = echo.accept().await {
            let mut buf = vec![0u8; 1024];
            if let Ok(n) = s.read(&mut buf).await {
                let _ = s.write_all(&buf[..n]).await;
            }
        }
    });

    let (proxy_handle, proxy_port) = start_test_proxy(None).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .expect("Should connect to proxy");

    let connect_req = format!(
        "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        echo_port, echo_port
    );
    stream.write_all(connect_req.as_bytes()).await.unwrap();

    // Read tunnel establishment response
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(
        resp.contains("200 Connection Established"),
        "Expected 200 Connection Established, got: {}",
        resp
    );

    // Send data through tunnel and expect echo
    let payload = b"tunnel-payload-12345";
    stream.write_all(payload).await.unwrap();

    let mut echo_buf = vec![0u8; 1024];
    let n = stream.read(&mut echo_buf).await.unwrap();
    assert_eq!(
        &echo_buf[..n],
        payload,
        "Data sent through CONNECT tunnel must be echoed back"
    );

    proxy_handle.abort();
    echo_handle.abort();
}

/// CONNECT to an unreachable target should return 502 Bad Gateway
/// rather than crashing the proxy.
#[tokio::test]
async fn test_connect_unreachable_target_returns_502() {
    let (handle, port) = start_test_proxy(None).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("Should connect to proxy");

    // Port 1 is almost certainly not listening
    let connect_req = "CONNECT 127.0.0.1:1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n";
    stream.write_all(connect_req.as_bytes()).await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);

    assert!(
        resp.contains("502 Bad Gateway"),
        "Expected 502 for unreachable target, got: {}",
        resp
    );

    handle.abort();
}

/// After the proxy task is aborted (service toggle off), the port
/// must stop accepting new connections.
#[tokio::test]
async fn test_proxy_stops_after_abort() {
    let (handle, port) = start_test_proxy(None).await;

    // Confirm it's alive
    let alive = TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(alive.is_ok(), "Proxy should be running initially");
    drop(alive);

    // Abort (simulates the UI toggling the service off)
    handle.abort();
    let _ = handle.await;

    // Allow OS to reclaim the socket
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // The port should no longer be accepting
    let dead = TcpStream::connect(format!("127.0.0.1:{}", port)).await;
    assert!(
        dead.is_err(),
        "Proxy should NOT accept connections after abort"
    );
}
