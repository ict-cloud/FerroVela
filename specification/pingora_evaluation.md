# Pingora Evaluation Proof of Concept

## Overview

This document outlines a proof of concept (POC) replacing the existing `hyper`-based proxy implementation in FerroVela with Cloudflare's `pingora` framework. Pingora provides a high-performance framework specifically built for proxying, load-balancing, and gateway services.

The goal of this POC is to evaluate how `pingora` simplifies standard HTTP proxying and CONNECT tunneling while addressing the existing advanced features (like PAC configuration, exceptions, and upstream authentication).

## Architecture changes

The previous architecture utilized `hyper::server::conn::http1` and manually built the connection handshake to either an upstream proxy or directly to a target. It had specialized loops for checking NTLM/Kerberos 407 challenges.

In the Pingora-based architecture, the `ferrovela` application launches a Pingora `Server` and registers an `http_proxy_service`. The proxying behavior is entirely dictated by implementing the `ProxyHttp` trait.

### Routing Logic (`upstream_peer`)
Pingora simplifies connection routing via the `upstream_peer` method. Based on the request (`req`), we construct the destination `HttpPeer`:
- If exceptions apply or no upstream proxy is specified via PAC/Config, we construct an `HttpPeer` targeting the host directly.
- If an upstream proxy is resolved via the PAC script or config file, the `HttpPeer` targets the upstream proxy.
Pingora handles establishing the TCP connection and TLS handshake transparently, reducing manual boilerplate.

### Standard HTTP Proxying
For standard HTTP proxying (GET, POST), Pingora automatically forwards the requests to the `HttpPeer` constructed in `upstream_peer`.

### CONNECT Tunneling
Pingora provides built-in support for proxying CONNECT requests. By specifying the destination in `upstream_peer`, Pingora will complete the CONNECT handshake and begin proxying byte streams transparently.

## Advanced Features Support

### PAC Configuration & Exceptions
Pingora integrates perfectly with the existing PAC resolution logic. The asynchronous `upstream_peer` trait method allows for `await`ing the result of `resolve_proxy`, easily integrating exceptions and Javascript Boa execution context.

### Upstream Authentication (NTLM/Kerberos)
Pingora easily supports basic HTTP proxy authentication through the `upstream_request_filter` by inserting a `Proxy-Authorization` header.

However, advanced stateful proxy handshakes (such as NTLM and Kerberos SPNEGO) which require multi-step challenge-response handshakes on the same TCP connection are trickier. While Pingora has hooks like `upstream_response_filter` which can inspect the `407 Proxy Authentication Required` status code, retrying a request on the same connection with updated handshake headers requires maintaining an `AuthSession` per connection/session state. This may require building custom HTTP clients for upstream communication or using Pingora's retry mechanisms deeply. For the purpose of this POC, basic one-step authorization is implemented.

### IPC Signal Handling (MAGIC_SHOW_PATH)
Pingora easily supports custom application endpoints. By implementing the `request_filter` trait method, we can intercept requests matching the magic path, trigger our Tokio IPC channels to reveal the UI window, and return an HTTP `200 OK` early without forwarding the request upstream.

## Performance
By running the `request_bench` and `connect_bench` Criterion benchmarks, Pingora proves its reputation for zero-cost abstraction overhead when handling HTTP headers and bytes allocation.

## Conclusion
Pingora heavily reduces the boilerplate code necessary for maintaining TCP connections, retries, and protocol-level edge cases (such as draining bodies on 407s). The core `ProxyHttp` trait defines a clear structure. Future work would include building robust state-machine extensions for multi-step NTLM/Kerberos proxy authentication natively within the Pingora framework.
