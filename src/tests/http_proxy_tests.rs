use crate::config::{Config, ProxyConfig, UpstreamConfig, ExceptionsConfig};
use crate::proxy::Proxy;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn start_target_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("Failed to bind target");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0; 1024];
                    loop {
                        match socket.read(&mut buf).await {
                            Ok(n) if n == 0 => return,
                            Ok(n) => {
                                let req = String::from_utf8_lossy(&buf[..n]);
                                if req.starts_with("GET /") {
                                    let response = "HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!";
                                    socket.write_all(response.as_bytes()).await.unwrap();
                                    return; // Simple one-shot
                                }
                            }
                            Err(_) => return,
                        }
                    }
                });
            }
        }
    });
    port
}

async fn start_proxy(upstream: Option<UpstreamConfig>, exceptions: Option<ExceptionsConfig>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("Failed to bind proxy");
    let port = listener.local_addr().unwrap().port();

    let config = Config {
        proxy: ProxyConfig {
            port,
            pac_file: None,
            allow_private_ips: true,
        },
        upstream,
        exceptions,
    };

    let proxy = Proxy::new(Arc::new(config), None, None);
    tokio::spawn(async move {
        let _ = proxy.run_with_listener(listener).await;
    });

    port
}

#[tokio::test]
async fn test_http_proxy_get() {
    let target_port = start_target_server().await;
    let proxy_port = start_proxy(None, None).await;

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port)).await.expect("Failed to connect to proxy");

    // Construct a standard HTTP Proxy request
    // GET http://127.0.0.1:<target_port>/ HTTP/1.1
    let target_url = format!("http://127.0.0.1:{}/", target_port);
    let req = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", target_url, target_port);

    client.write_all(req.as_bytes()).await.unwrap();

    let mut buf = [0; 1024];
    let n = client.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);

    // Expect 200 OK
    assert!(resp.contains("200 OK"), "Expected 200 OK, got {}", resp);
    assert!(resp.contains("Hello, World!"), "Expected body Hello, World!, got {}", resp);
}

#[tokio::test]
async fn test_http_proxy_get_via_upstream() {
    // Mock Upstream Proxy
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream_listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = upstream_listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0; 1024];
                    let n = socket.read(&mut buf).await.unwrap();
                    let req = String::from_utf8_lossy(&buf[..n]);

                    // Verify it receives the request
                    // We expect absolute URI like GET http://... because we configured an upstream
                    // The proxy should forward the request to us.
                    if req.contains("GET http://") {
                         socket.write_all(b"HTTP/1.1 200 OK\r\n\r\nUpstream Hit").await.unwrap();
                    } else {
                         // Fallback for debugging
                         let msg = format!("HTTP/1.1 400 Bad Request\r\n\r\nNot Proxy Request: {}", req);
                         socket.write_all(msg.as_bytes()).await.unwrap();
                    }
                });
            }
        }
    });

    let upstream_config = UpstreamConfig {
        auth_type: "none".to_string(),
        username: None,
        password: None,
        domain: None,
        workstation: None,
        proxy_url: Some(format!("127.0.0.1:{}", upstream_port)),
    };

    let proxy_port = start_proxy(Some(upstream_config), None).await;

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port)).await.expect("Failed to connect to proxy");

    // Construct a standard HTTP Proxy request
    let target_url = "http://example.com/";
    let req = format!("GET {} HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n", target_url);

    client.write_all(req.as_bytes()).await.unwrap();

    let mut buf = [0; 1024];
    let n = client.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);

    assert!(resp.contains("200 OK"), "Expected 200 OK, got {}", resp);
    assert!(resp.contains("Upstream Hit"), "Expected Upstream Hit, got {}", resp);
}

#[tokio::test]
async fn test_http_proxy_get_exception() {
    let target_port = start_target_server().await;

    // Upstream that fails (binds but sends 500)
    let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream_listener.local_addr().unwrap().port();
     tokio::spawn(async move {
        while let Ok((mut socket, _)) = upstream_listener.accept().await {
             let _ = socket.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n").await;
        }
    });

    let upstream_config = UpstreamConfig {
        auth_type: "none".to_string(),
        username: None,
        password: None,
        domain: None,
        workstation: None,
        proxy_url: Some(format!("127.0.0.1:{}", upstream_port)),
    };

    let exceptions = ExceptionsConfig {
        hosts: vec!["127.0.0.1".to_string()],
    };

    let proxy_port = start_proxy(Some(upstream_config), Some(exceptions)).await;

    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port)).await.unwrap();
    // Use 127.0.0.1 to match exception
    let target_url = format!("http://127.0.0.1:{}/", target_port);
    let req = format!("GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", target_url, target_port);

    client.write_all(req.as_bytes()).await.unwrap();

    let mut buf = [0; 1024];
    let n = client.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);

    assert!(resp.contains("200 OK"), "Expected 200 OK (Direct), got {}", resp);
    assert!(resp.contains("Hello, World!"), "Expected body Hello, World!, got {}", resp);
}
