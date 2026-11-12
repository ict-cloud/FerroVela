use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela_lib::config::{Config, ProxyConfig};
use ferrovela_lib::proxy::Proxy;
use std::hint::black_box;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\nHello, World!";

/// Starts a local origin and a proxy (direct mode, private targets allowed so
/// the loopback origin is reachable) on `rt`; returns the proxy's port.
async fn start(origin_port_out: &mut u16) -> u16 {
    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    *origin_port_out = origin.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = origin.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                // The request content does not matter; a failed write shows up as
                // a bad response, which the client asserts on.
                let _ = sock.read(&mut buf).await;
                sock.write_all(RESPONSE).await.unwrap();
            });
        }
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let config = Config {
        proxy: ProxyConfig {
            allow_private_ips: true,
            ..Default::default()
        },
        upstream: None,
        exceptions: None,
    };
    let proxy = Proxy::new(Arc::new(config), None);
    tokio::spawn(async move { proxy.serve(listener).await });
    port
}

/// End-to-end cost of one plain HTTP request through the proxy (new client
/// connection each time, as the Connection: close request implies).
fn bench_plain_request(c: &mut Criterion) {
    c.bench_function("proxy_plain_request", |b| {
        // Build the servers on the same runtime that drives the iterations.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut origin_port = 0;
        let proxy_port = rt.block_on(start(&mut origin_port));
        let request = format!(
            "GET http://127.0.0.1:{origin_port}/ HTTP/1.1\r\nHost: 127.0.0.1:{origin_port}\r\nConnection: close\r\n\r\n"
        );

        b.to_async(rt).iter(|| async {
            let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
            client
                .write_all(black_box(request.as_bytes()))
                .await
                .unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            assert!(response.starts_with(b"HTTP/1.1 200"), "bad response");
            black_box(response);
        });
    });
}

criterion_group!(benches, bench_plain_request);
criterion_main!(benches);
