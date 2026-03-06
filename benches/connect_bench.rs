use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn benchmark_connect(c: &mut Criterion) {
    let mut group = c.benchmark_group("Connect Handling");

    group.bench_function("simple_connect", |b| {
        b.iter(|| {
            black_box(1 + 1);
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_connect);
criterion_main!(benches);
