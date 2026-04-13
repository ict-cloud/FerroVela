use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela_lib::proxy::http_utils::find_subsequence;
use memchr::memmem;
use std::hint::black_box;

/// The old byte-at-a-time implementation (pre-optimization baseline).
/// Kept here so the bench continues to document *why* the current approach
/// is faster even after the library has moved to `memchr::memmem`.
fn naive_find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Equivalent to `find_subsequence` from http_utils — `memchr::memmem::find`.
/// Listed explicitly to confirm the library function delegates correctly.
fn memchr_find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    memmem::find(haystack, needle)
}

fn bench_subsequence(c: &mut Criterion) {
    let needle = b"\r\n\r\n";

    // Scenario 1: Typical HTTP headers (~500 bytes), needle at the end.
    let mut small_haystack = Vec::new();
    for _ in 0..20 {
        small_haystack.extend_from_slice(b"Header-Key: Header-Value\r\n");
    }
    small_haystack.extend_from_slice(b"\r\n");

    // Scenario 2: Large buffer (16 KB), needle at the end.
    let mut large_haystack = Vec::new();
    for _ in 0..600 {
        large_haystack.extend_from_slice(b"Header-Key: Header-Value\r\n");
    }
    large_haystack.extend_from_slice(b"\r\n");

    // Scenario 3: Large buffer, needle at the beginning (early-exit case).
    let mut early_haystack = Vec::new();
    early_haystack.extend_from_slice(b"\r\n\r\n");
    for _ in 0..600 {
        early_haystack.extend_from_slice(b"Header-Key: Header-Value\r\n");
    }

    let mut group = c.benchmark_group("find_subsequence");

    // ── small haystack ──────────────────────────────────────────────────────
    group.bench_function("naive_small", |b| {
        b.iter(|| naive_find_subsequence(black_box(&small_haystack), black_box(needle)))
    });
    group.bench_function("memchr_small", |b| {
        b.iter(|| memchr_find_subsequence(black_box(&small_haystack), black_box(needle)))
    });
    group.bench_function("lib_small", |b| {
        b.iter(|| find_subsequence(black_box(&small_haystack), black_box(needle)))
    });

    // ── large haystack ──────────────────────────────────────────────────────
    group.bench_function("naive_large", |b| {
        b.iter(|| naive_find_subsequence(black_box(&large_haystack), black_box(needle)))
    });
    group.bench_function("memchr_large", |b| {
        b.iter(|| memchr_find_subsequence(black_box(&large_haystack), black_box(needle)))
    });
    group.bench_function("lib_large", |b| {
        b.iter(|| find_subsequence(black_box(&large_haystack), black_box(needle)))
    });

    // ── early exit ──────────────────────────────────────────────────────────
    group.bench_function("naive_early", |b| {
        b.iter(|| naive_find_subsequence(black_box(&early_haystack), black_box(needle)))
    });
    group.bench_function("memchr_early", |b| {
        b.iter(|| memchr_find_subsequence(black_box(&early_haystack), black_box(needle)))
    });
    group.bench_function("lib_early", |b| {
        b.iter(|| find_subsequence(black_box(&early_haystack), black_box(needle)))
    });

    group.finish();
}

criterion_group!(benches, bench_subsequence);
criterion_main!(benches);
