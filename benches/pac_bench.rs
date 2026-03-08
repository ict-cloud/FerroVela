use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela::pac::PacEngine;
use tokio::runtime::Runtime;

fn criterion_benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    // Create a temporary PAC script
    let pac_script = "function FindProxyForURL(url, host) { return 'DIRECT'; }";
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp_file.path(), pac_script).unwrap();

    let engine = rt.block_on(async {
        PacEngine::new(temp_file.path().to_str().unwrap())
            .await
            .unwrap()
    });

    c.bench_function("pac_find_proxy", |b| {
        b.to_async(&rt).iter(|| async {
            engine
                .find_proxy("http://example.com", "example.com")
                .await
                .unwrap()
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
