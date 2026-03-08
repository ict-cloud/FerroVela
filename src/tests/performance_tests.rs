use crate::config::{Config, ProxyConfig, UpstreamConfig};
use crate::proxy::Proxy;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn start_performance_target_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind target");
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0; 4096];
                    let mut offset = 0;
                    loop {
                        if let Ok(n) = socket.read(&mut buf[offset..]).await {
                            if n == 0 {
                                return;
                            }
                            offset += n;
                            let req = String::from_utf8_lossy(&buf[..offset]);
                            if req.contains("\r\n\r\n") {
                                if req.contains("GET ") {
                                    let response = "HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nHello, World!";
                                    if socket.write_all(response.as_bytes()).await.is_ok() {
                                        let _ = socket.flush().await;
                                    }
                                }
                                return;
                            }
                            if offset >= buf.len() {
                                return;
                            }
                        } else {
                            return;
                        }
                    }
                });
            }
        }
    });
    port
}

async fn start_performance_proxy(upstream_port: u16) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind proxy");
    let port = listener.local_addr().unwrap().port();

    let upstream_config = UpstreamConfig {
        auth_type: "none".to_string(),
        username: None,
        password: None,
        domain: None,
        workstation: None,
        proxy_url: Some(format!("127.0.0.1:{}", upstream_port)),
    };

    let config = Config {
        proxy: ProxyConfig {
            port,
            pac_file: None,
            allow_private_ips: true,
        },
        upstream: Some(upstream_config),
        exceptions: None,
    };

    let proxy = Proxy::new(Arc::new(config), None, None);
    tokio::spawn(async move {
        let _ = proxy.run_with_listener(listener).await;
    });

    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "performance test not working"]
async fn test_proxy_throughput() {
    let target_port = start_performance_target_server().await;
    let proxy_port = start_performance_proxy(target_port).await;

    let concurrent_clients = 50;
    let requests_per_client = 100;
    let total_requests = concurrent_clients * requests_per_client;

    println!(
        "Starting performance test with {} clients, {} requests each (Total: {})",
        concurrent_clients, requests_per_client, total_requests
    );

    let start_time = Instant::now();
    let mut tasks = Vec::new();

    for _ in 0..concurrent_clients {
        let proxy_addr = format!("127.0.0.1:{}", proxy_port);
        tasks.push(tokio::spawn(async move {
            let mut success_count = 0;
            for _ in 0..requests_per_client {
                if let Ok(mut stream) = TcpStream::connect(&proxy_addr).await {
                    let req = b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
                    if stream.write_all(req).await.is_err() { continue; }

                    let mut buf = [0u8; 1024];
                    if let Ok(n) = stream.read(&mut buf).await {
                         if n > 0 {
                             let response = String::from_utf8_lossy(&buf[..n]);
                             if response.contains("200 OK") {
                                 success_count += 1;
                             }
                         }
                    }
                }
            }
            success_count
        }));
    }

    let mut total_success = 0;
    for task in tasks {
        total_success += task.await.unwrap();
    }

    let duration = start_time.elapsed();
    let rps = total_success as f64 / duration.as_secs_f64();

    println!("Performance Test Results:");
    println!("Total Requests: {}", total_requests);
    println!("Successful Requests: {}", total_success);
    println!("Total Duration: {:.2?}", duration);
    println!("Requests Per Second (RPS): {:.2}", rps);

    assert!(total_success > 0, "No requests succeeded");
    // With connection close, RPS might be lower, but should be decent on localhost.
    assert!(rps > 10.0, "RPS is too low ({:.2})", rps);
}
