use criterion::{criterion_group, criterion_main, Criterion};
use hyper::header::HeaderValue;
use hyper::{Method, Request, Version};
use std::hint::black_box;

pub fn request_builder_bench(c: &mut Criterion) {
    let method = Method::GET;
    let uri = "/hello".parse::<hyper::Uri>().unwrap();
    let version = Version::HTTP_11;
    let mut headers = hyper::HeaderMap::new();
    headers.insert("host", HeaderValue::from_static("example.com"));
    headers.insert("user-agent", HeaderValue::from_static("benchmark/1.0"));
    headers.insert("accept", HeaderValue::from_static("*/*"));
    headers.insert("connection", HeaderValue::from_static("keep-alive"));
    headers.insert("accept-encoding", HeaderValue::from_static("gzip, deflate"));
    headers.insert(
        "accept-language",
        HeaderValue::from_static("en-US,en;q=0.9"),
    );

    let challenge_header = HeaderValue::from_static("Basic YWxhZGRpbjpvcGVuc2VzYW1l");

    c.bench_function("request_builder_in_loop", |b| {
        b.iter(|| {
            let mut builder = Request::builder()
                .method(method.clone())
                .uri(uri.clone())
                .version(version);

            for (k, v) in headers.iter() {
                if k != "proxy-authorization" {
                    builder = builder.header(k, v);
                }
            }

            builder = builder.header(hyper::header::PROXY_AUTHORIZATION, challenge_header.clone());

            let req = builder.body(()).unwrap();
            black_box(req)
        })
    });

    c.bench_function("request_builder_optimized_clone", |b| {
        // Pre-build the request once, like we would outside the loop
        let mut builder = Request::builder()
            .method(method.clone())
            .uri(uri.clone())
            .version(version);
        for (k, v) in headers.iter() {
            if k != "proxy-authorization" {
                builder = builder.header(k, v);
            }
        }
        let base_req = builder.body(()).unwrap();

        b.iter(|| {
            let mut req = base_req.clone();
            req.headers_mut()
                .insert(hyper::header::PROXY_AUTHORIZATION, challenge_header.clone());
            black_box(req)
        })
    });
}

criterion_group!(benches, request_builder_bench);
criterion_main!(benches);
