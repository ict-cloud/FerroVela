use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela::proxy::http_utils::parse_content_length;
use std::hint::black_box;

fn bench_parse_content_length(c: &mut Criterion) {
    let headers = "Host: example.com\r\nUser-Agent: curl/7.68.0\r\nAccept: */*\r\nContent-Length: 42\r\nConnection: keep-alive\r\n";
    c.bench_function("parse_content_length", |b| {
        b.iter(|| parse_content_length(black_box(headers)))
    });
}

criterion_group!(benches, bench_parse_content_length);
criterion_main!(benches);
