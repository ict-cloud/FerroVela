use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela_lib::proxy::auth_tunnel::find_header_value;
use std::hint::black_box;

// ─── 407 response parsing (NTLM/Kerberos challenge loop) ─────────────────────
//
// On every auth round-trip the proxy reads a 407 response and extracts the
// Proxy-Authenticate challenge token, up to four times per authenticated CONNECT.

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

    group.finish();
}

criterion_group!(benches, bench_challenge_parsing);
criterion_main!(benches);
