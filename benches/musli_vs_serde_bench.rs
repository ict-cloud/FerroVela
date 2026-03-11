use criterion::{criterion_group, criterion_main, Criterion};
use ferrovela::config::{Config, ExceptionsConfig, ProxyConfig, UpstreamConfig};
use std::hint::black_box;

fn generate_config() -> Config {
    Config {
        proxy: ProxyConfig {
            port: 8080,
            pac_file: Some("http://internal.wpad/wpad.dat".to_string()),
            allow_private_ips: true,
        },
        upstream: Some(UpstreamConfig {
            auth_type: "basic".to_string(),
            username: Some("proxy_user".to_string()),
            password: Some("super_secret_password".to_string()),
            domain: Some("corp.local".to_string()),
            use_keyring: false,
            workstation: Some("WORKSTATION1".to_string()),
            proxy_url: Some("192.168.1.100:3128".to_string()),
        }),
        exceptions: Some(ExceptionsConfig {
            hosts: vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "*.internal".to_string(),
                "*.corp.local".to_string(),
            ],
        }),
    }
}

fn bench_serde_serialization(c: &mut Criterion) {
    let config = generate_config();
    c.bench_function("serde serialize toml", |b| {
        b.iter(|| {
            let s = toml::to_string(black_box(&config)).unwrap();
            black_box(s);
        })
    });
}

fn bench_serde_deserialization(c: &mut Criterion) {
    let config = generate_config();
    let toml_str = toml::to_string(&config).unwrap();
    c.bench_function("serde deserialize toml", |b| {
        b.iter(|| {
            let c: Config = toml::from_str(black_box(&toml_str)).unwrap();
            black_box(c);
        })
    });
}

fn bench_musli_serialization(c: &mut Criterion) {
    let config = generate_config();
    c.bench_function("musli serialize binary (storage)", |b| {
        b.iter(|| {
            let bytes = musli::storage::to_vec(black_box(&config)).unwrap();
            black_box(bytes);
        })
    });
}

fn bench_musli_deserialization(c: &mut Criterion) {
    let config = generate_config();
    let bytes = musli::storage::to_vec(&config).unwrap();
    c.bench_function("musli deserialize binary (storage)", |b| {
        b.iter(|| {
            let c: Config = musli::storage::decode(black_box(bytes.as_slice())).unwrap();
            black_box(c);
        })
    });
}

criterion_group!(
    benches,
    bench_serde_serialization,
    bench_serde_deserialization,
    bench_musli_serialization,
    bench_musli_deserialization
);
criterion_main!(benches);
