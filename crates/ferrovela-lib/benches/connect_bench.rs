use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela_lib::proxy::auth_tunnel::{http_method, parse_connect_target};
use ferrovela_lib::proxy::http_utils::{find_header_value, parse_content_length};
use std::hint::black_box;

// ─── CONNECT request parsing ─────────────────────────────────────────────────

fn bench_parse_connect_target(c: &mut Criterion) {
    let mut group = c.benchmark_group("CONNECT Request Parsing");

    let connect_line = "CONNECT example.com:443 HTTP/1.1";
    let connect_ipv6 = "CONNECT [2001:db8::1]:443 HTTP/1.1";
    let not_connect = "GET http://example.com/ HTTP/1.1";

    group.bench_function("parse_connect_typical", |b| {
        b.iter(|| parse_connect_target(black_box(connect_line)))
    });

    group.bench_function("parse_connect_ipv6", |b| {
        b.iter(|| parse_connect_target(black_box(connect_ipv6)))
    });

    group.bench_function("parse_connect_not_connect", |b| {
        b.iter(|| parse_connect_target(black_box(not_connect)))
    });

    group.bench_function("http_method", |b| {
        b.iter(|| http_method(black_box(connect_line)))
    });

    group.finish();
}

// ─── 407 response parsing (NTLM/Kerberos challenge loop) ─────────────────────
//
// On every auth round-trip the proxy reads a 407 response and extracts:
//   1. The status code (parse_content_length is a proxy for the header scan).
//   2. The Proxy-Authenticate challenge token.
// These two operations happen up to four times per authenticated CONNECT.

fn bench_challenge_parsing(c: &mut Criterion) {
    // Typical 407 NTLM Type-2 challenge response
    let ntlm_407 = concat!(
        "HTTP/1.1 407 Proxy Authentication Required\r\n",
        "Proxy-Authenticate: NTLM TlRMTVNTUAACAAAADAAMADgAAAAFgomi",
        "ESXXVVRt8QUAAAAAAAAA4ADgBEAAAABgGwIwAAAA9DAE8AUgBQAAIADABD",
        "AE8AUgBQAAEAFABTAEUAUgBWAEUAUgAEABQAYwBvAHIAcAAuAGMAbwBtAAMAJABz",
        "AGUAcgB2AGUAcgAuAGMAbwByAHAALgBjAG8AbQAFABQAYwBvAHIAcAAuAGMAbwBtAAAAAAA=\r\n",
        "Content-Length: 0\r\n",
        "Connection: keep-alive\r\n",
        "Proxy-Connection: keep-alive\r\n",
        "\r\n",
    );

    // Typical 407 Kerberos/Negotiate challenge
    let negotiate_407 = concat!(
        "HTTP/1.1 407 Proxy Authentication Required\r\n",
        "Proxy-Authenticate: Negotiate YIIGhgYJKoZIhvcSAQICAQBuggZ1MIIGcaADAgEFoQMCAQ==\r\n",
        "Content-Length: 0\r\n",
        "Connection: keep-alive\r\n",
        "\r\n",
    );

    let mut group = c.benchmark_group("Challenge Response Parsing");

    group.bench_function("find_ntlm_challenge", |b| {
        b.iter(|| find_header_value(black_box(ntlm_407), black_box("Proxy-Authenticate")))
    });

    group.bench_function("find_negotiate_challenge", |b| {
        b.iter(|| find_header_value(black_box(negotiate_407), black_box("Proxy-Authenticate")))
    });

    group.bench_function("parse_content_length_407", |b| {
        b.iter(|| parse_content_length(black_box(ntlm_407)))
    });

    group.finish();
}

criterion_group!(benches, bench_parse_connect_target, bench_challenge_parsing);
criterion_main!(benches);
