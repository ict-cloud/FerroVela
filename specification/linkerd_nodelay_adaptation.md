# Linkerd2-Proxy TCP_NODELAY Adaptation Summary

## Background

When investigating high-performance proxy implementations, [linkerd2-proxy](https://github.com/linkerd/linkerd2-proxy) stands out for its robust use of Tokio and hyper under the hood. During an analysis of their codebase, it was observed that the core proxy connection routines explicitly configure `set_nodelay(true)` (TCP_NODELAY) and occasionally `keepalive` times on all managed `TcpStream` connections.

Specifically, in `linkerd2-proxy/linkerd/proxy/transport/src/lib.rs`, `set_nodelay_or_warn(socket)` is systematically called:

```rust
fn set_nodelay_or_warn(socket: &TcpStream) {
    if let Err(e) = socket.set_nodelay(true) {
        tracing::warn!("failed to set nodelay: {}", e);
    }
}
```

## Problem

By default, operating systems employ Nagle's algorithm on TCP streams. Nagle's algorithm buffers small outgoing packets in an attempt to send fewer, larger packets, thereby reducing network congestion. However, for an HTTP/HTTPS proxy—which frequently deals in many small, interactive request/response headers or chunks—this artificial delay drastically increases latency.

In FerroVela, `TcpStream` instances were being instantiated for `CONNECT` tunneling (in `src/proxy/connect.rs`), HTTP proxy forwarding (`src/proxy/nonconnect.rs`), and the primary listener socket (`src/proxy/mod.rs`), but Nagle's algorithm was left enabled by default.

## Solution

To adapt this pattern from linkerd2-proxy, `set_nodelay(true)` was explicitly added to all points where `TcpStream` acts as a network primitive in FerroVela:

1. **Proxy Listener**: In `src/proxy/mod.rs`, when a new incoming client connection is accepted.
2. **Direct Forwarding**: In `connect_direct` (`connect.rs`) and HTTP direct requests (`nonconnect.rs`), when connecting to the destination server.
3. **Upstream Proxying**: In `connect_via_upstream` (`connect.rs`) and HTTP upstream proxying (`nonconnect.rs`), when connecting to an upstream corporate proxy.

## Performance Results

Running the built-in streaming benchmarks (`test_proxy_throughput` in `performance_tests.rs`) showed a significant improvement.

**Before Adaptation (Typical run):**
```
Total Requests: 5000
Total Duration: ~570-800ms
Requests Per Second (RPS): ~6200 - 8700
```

**After Adaptation:**
```
Total Requests: 5000
Total Duration: 369.08ms
Requests Per Second (RPS): 13547.20
```

The TCP nodelay optimization roughly **doubles** the requests-per-second throughput in the raw proxy streaming benchmark, effectively making the proxy much more responsive to concurrent HTTP traffic and fast TLS handshakes.
