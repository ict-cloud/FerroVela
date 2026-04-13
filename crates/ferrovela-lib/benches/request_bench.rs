use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela_lib::proxy::http_utils::find_header_value;
use std::hint::black_box;

// ─── baseline: old serialize_http_request approach ───────────────────────────

/// Mirrors the pre-optimization approach: build a `String`, then call
/// `format!("{}: {}\r\n", name, v)` per header — one throwaway heap allocation
/// per header line.
fn serialize_headers_naive(method: &str, uri: &str, headers: &[(&str, &str)]) -> Vec<u8> {
    let mut out = format!("{} {} HTTP/1.1\r\n", method, uri);
    for (name, value) in headers {
        out.push_str(&format!("{}: {}\r\n", name, value));
    }
    out.push_str("\r\n");
    out.into_bytes()
}

// ─── optimized: write_http_request approach ──────────────────────────────────

/// Mirrors the post-optimization approach: preallocate `Vec<u8>` and write
/// with `extend_from_slice` — zero intermediate allocations per header.
fn serialize_headers_optimized(
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    basic_auth_b64: Option<&str>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(2048);
    out.extend_from_slice(method.as_bytes());
    out.push(b' ');
    out.extend_from_slice(uri.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\n");
    for (name, value) in headers {
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if let Some(b64) = basic_auth_b64 {
        out.extend_from_slice(b"Proxy-Authorization: Basic ");
        out.extend_from_slice(b64.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out
}

// ─── shared fixture ──────────────────────────────────────────────────────────

fn typical_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Host", "example.com"),
        ("User-Agent", "Mozilla/5.0"),
        ("Accept", "text/html,application/xhtml+xml"),
        ("Accept-Language", "en-US,en;q=0.9"),
        ("Accept-Encoding", "gzip, deflate, br"),
        ("Connection", "keep-alive"),
        ("Upgrade-Insecure-Requests", "1"),
        ("Cache-Control", "max-age=0"),
        ("Pragma", "no-cache"),
        ("DNT", "1"),
    ]
}

// ─── benchmarks ──────────────────────────────────────────────────────────────

fn bench_serialize_headers(c: &mut Criterion) {
    let headers = typical_headers();
    let method = "GET";
    let uri = "http://example.com/path?query=value";
    let basic_auth = Some("dXNlcjpwYXNz"); // base64("user:pass")

    let mut group = c.benchmark_group("Request Serialization");

    // Old approach: String + format! per header
    group.bench_function("naive_no_auth", |b| {
        b.iter(|| serialize_headers_naive(black_box(method), black_box(uri), black_box(&headers)))
    });

    // New approach: Vec<u8> + extend_from_slice, no auth
    group.bench_function("optimized_no_auth", |b| {
        b.iter(|| {
            serialize_headers_optimized(
                black_box(method),
                black_box(uri),
                black_box(&headers),
                black_box(None),
            )
        })
    });

    // New approach: with precomputed Basic auth injected
    group.bench_function("optimized_basic_auth", |b| {
        b.iter(|| {
            serialize_headers_optimized(
                black_box(method),
                black_box(uri),
                black_box(&headers),
                black_box(basic_auth),
            )
        })
    });

    group.finish();
}

fn bench_find_header_value(c: &mut Criterion) {
    // Realistic response header block
    let headers = concat!(
        "HTTP/1.1 407 Proxy Authentication Required\r\n",
        "Proxy-Authenticate: NTLM TlRMTVNTUAABAAAAB4IIogAAAAAAAAAAAAAAAAAAAAA=\r\n",
        "Content-Length: 0\r\n",
        "Connection: keep-alive\r\n",
        "Server: nginx/1.18.0\r\n",
        "\r\n",
    );

    let mut group = c.benchmark_group("Header Lookup");

    // Lookup a header that appears early in the block
    group.bench_function("find_proxy_authenticate", |b| {
        b.iter(|| find_header_value(black_box(headers), black_box("Proxy-Authenticate")))
    });

    // Lookup a header that appears late (worst case scan)
    group.bench_function("find_server_last", |b| {
        b.iter(|| find_header_value(black_box(headers), black_box("Server")))
    });

    // Lookup a header that is absent (full scan)
    group.bench_function("find_absent", |b| {
        b.iter(|| find_header_value(black_box(headers), black_box("X-Custom-Header")))
    });

    group.finish();
}

criterion_group!(benches, bench_serialize_headers, bench_find_header_value);
criterion_main!(benches);
