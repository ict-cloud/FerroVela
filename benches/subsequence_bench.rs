use criterion::{criterion_group, criterion_main, Criterion};
use memchr::memmem;

// The current implementation in src/proxy/connect.rs
fn current_find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// The proposed optimization using memchr
fn optimized_find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    memmem::find(haystack, needle)
}

fn bench_subsequence(c: &mut Criterion) {
    let needle = b"\r\n\r\n";

    // Scenario 1: Typical HTTP headers (approx 500 bytes), needle at the end
    let mut small_haystack = Vec::new();
    for _ in 0..20 {
        small_haystack.extend_from_slice(b"Header-Key: Header-Value\r\n");
    }
    small_haystack.extend_from_slice(b"\r\n"); // Completes \r\n\r\n

    // Scenario 2: Large buffer (16KB), needle at the end
    let mut large_haystack = Vec::new();
    for _ in 0..600 {
        large_haystack.extend_from_slice(b"Header-Key: Header-Value\r\n");
    }
    large_haystack.extend_from_slice(b"\r\n");

    // Scenario 3: Large buffer, needle at the beginning (early exit)
    let mut early_haystack = Vec::new();
    early_haystack.extend_from_slice(b"\r\n\r\n");
    for _ in 0..600 {
        early_haystack.extend_from_slice(b"Header-Key: Header-Value\r\n");
    }

    let mut group = c.benchmark_group("find_subsequence");

    group.bench_function("current_small", |b| {
        b.iter(|| current_find_subsequence(criterion::black_box(&small_haystack), criterion::black_box(needle)))
    });

    group.bench_function("memchr_small", |b| {
        b.iter(|| optimized_find_subsequence(criterion::black_box(&small_haystack), criterion::black_box(needle)))
    });

    group.bench_function("current_large", |b| {
        b.iter(|| current_find_subsequence(criterion::black_box(&large_haystack), criterion::black_box(needle)))
    });

    group.bench_function("memchr_large", |b| {
        b.iter(|| optimized_find_subsequence(criterion::black_box(&large_haystack), criterion::black_box(needle)))
    });

    group.bench_function("current_early", |b| {
        b.iter(|| current_find_subsequence(criterion::black_box(&early_haystack), criterion::black_box(needle)))
    });

    group.bench_function("memchr_early", |b| {
        b.iter(|| optimized_find_subsequence(criterion::black_box(&early_haystack), criterion::black_box(needle)))
    });

    group.finish();
}

criterion_group!(benches, bench_subsequence);
criterion_main!(benches);
