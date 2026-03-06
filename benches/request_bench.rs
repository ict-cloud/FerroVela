use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn benchmark_request_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("Request Handling");

    group.bench_function("simple_request", |b| {
        b.iter(|| {
            black_box(1 + 1);
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_request_creation);
criterion_main!(benches);
