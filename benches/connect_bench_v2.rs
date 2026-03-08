use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela::config::{Config, ExceptionsConfig, ProxyConfig, UpstreamConfig};
use ferrovela::proxy::resolve_proxy;
use std::hint::black_box;
use std::sync::Arc;

fn benchmark_connect_v2(c: &mut Criterion) {
    let mut group = c.benchmark_group("Connect Handling");
    group.sample_size(100);

    // 1. Baseline: Direct connection resolution (no PAC, no upstream)
    group.bench_function("direct_connection_resolution", |b| {
        let config = Arc::new(Config {
            proxy: ProxyConfig {
                port: 3128,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: None,
            exceptions: None,
        });
        let pac = Arc::new(None);

        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                let _ = resolve_proxy(black_box("example.com:443"), &config, &pac).await;
            });
    });

    // 2. Exception matching performance
    group.bench_function("exception_matching", |b| {
        let config = Arc::new(Config {
            proxy: ProxyConfig {
                port: 3128,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: Some(UpstreamConfig {
                proxy_url: Some("10.0.0.1:3128".to_string()),
                auth_type: "none".to_string(),
                username: None,
                password: None,
                domain: None,
                workstation: None,
            }),
            exceptions: Some(ExceptionsConfig {
                hosts: vec!["localhost".to_string(), "*.internal".to_string()],
            }),
        });
        let pac = Arc::new(None);

        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                let _ = resolve_proxy(black_box("internal.corp:443"), &config, &pac).await;
            });
    });

    // 3. Upstream proxy selection (no PAC)
    group.bench_function("upstream_selection_static", |b| {
        let config = Arc::new(Config {
            proxy: ProxyConfig {
                port: 3128,
                pac_file: None,
                allow_private_ips: false,
            },
            upstream: Some(UpstreamConfig {
                proxy_url: Some("10.0.0.1:3128".to_string()),
                auth_type: "none".to_string(),
                username: None,
                password: None,
                domain: None,
                workstation: None,
            }),
            exceptions: None,
        });
        let pac = Arc::new(None);

        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                let _ = resolve_proxy(black_box("example.com:443"), &config, &pac).await;
            });
    });

    group.finish();
}

criterion_group!(benches, benchmark_connect_v2);
criterion_main!(benches);
