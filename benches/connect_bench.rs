use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use ferrovela::config::{Config, ProxyConfig, UpstreamConfig};
use ferrovela::proxy::Proxy;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;

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

fn connect_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    // Setup environment once
    let (_target_port, proxy_port) = rt.block_on(async {
        let tp = start_performance_target_server().await;
        let pp = start_performance_proxy(tp).await;
        (tp, pp)
    });

    let proxy_addr = format!("127.0.0.1:{}", proxy_port);

    let mut group = c.benchmark_group("connect");
    group.throughput(Throughput::Elements(1));

    group.bench_function("connect_throughput", |b| {
        b.to_async(&rt).iter(|| async {
            if let Ok(mut stream) = TcpStream::connect(&proxy_addr).await {
                let req = b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
                if stream.write_all(req).await.is_ok() {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                }
            }
        })
    });

    group.finish();
}

criterion_group!(benches, connect_benchmark);
criterion_main!(benches);
