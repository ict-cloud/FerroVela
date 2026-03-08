use criterion::{criterion_group, criterion_main, Criterion};

fn benchmark_target_split(c: &mut Criterion) {
    let mut group = c.benchmark_group("Target Split");
    let target = "example.com:443";

    group.bench_function("vec_collect", |b| {
        b.iter(|| {
            let parts: Vec<&str> = target.split(':').collect();
            let host = parts[0];
            let port = parts
                .get(1)
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(80);
            criterion::black_box((host, port));
        });
    });

    group.bench_function("split_once", |b| {
        b.iter(|| {
            let (host, port) = if let Some((h, p)) = target.split_once(':') {
                (h, p.parse::<u16>().unwrap_or(80))
            } else {
                (target, 80)
            };
            criterion::black_box((host, port));
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_target_split);
criterion_main!(benches);
