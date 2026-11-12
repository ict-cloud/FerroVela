use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela_lib::pac::PacEngine;
use std::hint::black_box;
use std::io::Write;

/// Per-request PAC cost: one `FindProxyForURL` call crossing the channel into
/// a QuickJS worker thread.  This runs for every CONNECT when a PAC is set.
fn bench_find_proxy(c: &mut Criterion) {
    let mut pac = tempfile::NamedTempFile::new().unwrap();
    pac.write_all(
        br#"function FindProxyForURL(url, host) {
            if (dnsDomainIs(host, ".internal")) return "DIRECT";
            return "PROXY proxy.corp:8080";
        }"#,
    )
    .unwrap();
    let path = pac.path().to_str().unwrap().to_owned();
    let engine = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(PacEngine::new(&path))
        .unwrap();

    c.bench_function("pac_find_proxy", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                black_box(
                    engine
                        .find_proxy(black_box("https://example.com/"), black_box("example.com"))
                        .await
                        .unwrap(),
                );
            });
    });
}

criterion_group!(benches, bench_find_proxy);
criterion_main!(benches);
