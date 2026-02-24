use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ferrovela::config::ExceptionsConfig;

// Replicating the current logic for baseline measurement
fn match_host_baseline(host: &str, exceptions: &ExceptionsConfig) -> bool {
    for pattern in &exceptions.hosts {
        if pattern == host {
            return true;
        }
        if pattern.starts_with("*.") && host.ends_with(&pattern[2..]) {
            return true;
        }
    }
    false
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut hosts = Vec::new();
    // Add 1000 hosts
    for i in 0..1000 {
        if i % 2 == 0 {
            hosts.push(format!("host{}.example.com", i));
        } else {
            hosts.push(format!("*.sub{}.example.com", i));
        }
    }
    // Add some known matches
    hosts.push("exact.match.com".to_string());
    hosts.push("*.wildcard.match.com".to_string());

    let mut exceptions = ExceptionsConfig {
        hosts,
        ..Default::default()
    };
    exceptions.compile();

    c.bench_function("exception_match_baseline", |b| {
        b.iter(|| {
            // Test exact match
            black_box(match_host_baseline(black_box("exact.match.com"), black_box(&exceptions)));
            // Test wildcard match
            black_box(match_host_baseline(black_box("foo.wildcard.match.com"), black_box(&exceptions)));
            // Test miss
            black_box(match_host_baseline(black_box("no.match.com"), black_box(&exceptions)));
        })
    });

    c.bench_function("exception_match_optimized", |b| {
        b.iter(|| {
            // Test exact match
            black_box(exceptions.matches(black_box("exact.match.com")));
            // Test wildcard match
            black_box(exceptions.matches(black_box("foo.wildcard.match.com")));
            // Test miss
            black_box(exceptions.matches(black_box("no.match.com")));
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
