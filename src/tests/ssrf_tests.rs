extern crate ferrovela;
use ferrovela::config::{Config, ProxyConfig};
use ferrovela::proxy::Proxy;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn test_ssrf_protection() {
    // 1. Start a "sensitive" internal service on localhost
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind sensitive service");
    let sensitive_port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            // Read something first
            let mut buf = [0; 1024];
            if let Ok(n) = socket.read(&mut buf).await {
                if n > 0 {
                    let _ = socket.write_all(b"Sensitive Data").await;
                }
            }
        }
    });

    // 2. Start the proxy
    let proxy_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind proxy");
    let proxy_port = proxy_listener.local_addr().unwrap().port();

    let config = Config {
        proxy: ProxyConfig {
            port: proxy_port,
            pac_file: None,
            allow_private_ips: false,
        },
        upstream: None,
        exceptions: None,
    };

    let proxy = Proxy::new(Arc::new(config), None, None);
    tokio::spawn(async move {
        let _ = proxy.run_with_listener(proxy_listener).await;
    });

    // 3. Attempt to CONNECT to the sensitive service via the proxy
    let mut client = TcpStream::connect(format!("127.0.0.1:{}", proxy_port))
        .await
        .expect("Failed to connect to proxy");

    let connect_req = format!(
        "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        sensitive_port, sensitive_port
    );
    client.write_all(connect_req.as_bytes()).await.unwrap();

    // Read the 200 OK (or whatever response)
    let mut buf = [0; 1024];
    let n = client.read(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);

    println!("Proxy Response: {}", resp);

    // If we didn't even get 200 OK, that's fine (maybe future improvement),
    // but if we did, we must check if the tunnel actually works.
    if resp.contains("200") {
        // Try to talk to the service
        if client.write_all(b"Hello").await.is_err() {
            // Write failed, connection closed. Safe.
            return;
        }

        let n = match client.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => 0, // Read error, connection closed. Safe.
        };

        if n > 0 {
            let data = String::from_utf8_lossy(&buf[..n]);
            if data.contains("Sensitive Data") {
                 panic!("Vulnerability confirmed: Proxy allowed connection to localhost/internal network!");
            }
        }
    }
}
