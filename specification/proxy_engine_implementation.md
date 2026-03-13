# Proxy Engine Implementation

## Overview

This document describes the implemented proxy engine introduced in `src/proxy/mod.rs` and
`src/proxy/auth_tunnel.rs`. It replaces the earlier placeholder (`run()` returning `Ok(())`)
with a fully functional proxy that combines:

- **g3proxy** (ByteDance) as the embedded HTTP proxy server and direct-connection escaper.
- A **pre-processor** listener that owns the user-facing port and handles IPC and auth routing.
- An **auth tunnel** module that drives the NTLM and Kerberos challenge-response handshakes
  that g3proxy's built-in escapers cannot perform.

---

## Architecture

```
                        ┌─────────────────────────────────────────────────┐
  Clients / System      │  FerroVela Process                              │
  proxy settings        │                                                 │
         │              │  ┌──────────────────────────────────────────┐   │
         │              │  │  Pre-processor  (127.0.0.1:configured)   │   │
         └──────────────┼─▶│                                          │   │
                        │  │  peek first bytes                        │   │
                        │  │    ├─ Magic IPC request ──▶ 200 OK       │   │
                        │  │    │                       + UI signal   │   │
                        │  │    ├─ Kerberos / NTLM ──▶ Auth Tunnel   │   │
                        │  │    └─ Basic / None    ──▶ g3proxy        │   │
                        │  └───────────────┬───────────────┬──────────┘   │
                        │                  │               │               │
                        │      ┌───────────▼───┐   ┌──────▼───────────┐   │
                        │      │  Auth Tunnel  │   │  g3proxy         │   │
                        │      │  (loopback)   │   │  (127.0.0.1:X)  │   │
                        │      └───────┬───────┘   └──────┬───────────┘   │
                        └─────────────┼──────────────────┼───────────────┘
                                      │                  │
                              Upstream proxy        Target host
                              (NTLM / Kerberos)   (direct / Basic)
```

### Components

| Component | Location | Responsibility |
|---|---|---|
| `Proxy::run()` | `src/proxy/mod.rs` | Start g3proxy engine, bind pre-processor listener, dispatch connections |
| Pre-processor loop | `src/proxy/mod.rs` | IPC detection, auth-type routing |
| `handle_connection()` | `src/proxy/mod.rs` | Per-connection dispatcher |
| `auth_tunnel` module | `src/proxy/auth_tunnel.rs` | NTLM & Kerberos CONNECT handshake |
| g3proxy engine | `g3proxy` crate | HTTP proxy server + c-ares resolver + DirectFixed / ProxyHttp escaper |

---

## Startup Sequence

```
Proxy::run()
 1. Bind 127.0.0.1:0  →  read free OS port  →  drop socket  (internal_port)
 2. build_g3proxy_yaml(internal_port)  →  write to temp YAML file
 3. g3_daemon::opts::validate_and_set_config_file()
 4. g3proxy::config::load()
 5. g3proxy::resolve::spawn_all()      ← c-ares DNS resolver
 6. g3proxy::escape::load_all()        ← DirectFixed or ProxyHttp escaper
 7. g3proxy::auth::load_all()
 8. g3proxy::audit::load_all()
 9. g3proxy::serve::spawn_offline_clean()
10. g3proxy::serve::spawn_all()        ← HttpProxy server on internal_port
11. TcpListener::bind(configured_port) ← pre-processor
12. accept loop  →  tokio::spawn(handle_connection)
```

---

## g3proxy YAML Configuration

The YAML config is generated at runtime from FerroVela's `Config`. The escaper type
depends on the configured auth method:

### No upstream / DirectFixed (Kerberos, NTLM, or no upstream configured)

```yaml
resolver:
  - name: default
    type: c-ares

escaper:
  - name: default
    type: DirectFixed
    resolver: default

server:
  - name: proxy
    type: HttpProxy
    escaper: default
    listen: "127.0.0.1:<internal_port>"
```

### Upstream with Basic auth

```yaml
resolver:
  - name: default
    type: c-ares

escaper:
  - name: default
    type: ProxyHttp
    proxy_addr: "<host:port>"
    proxy_username: "<user>"
    proxy_password: "<pass>"
    resolver: default

server:
  - name: proxy
    type: HttpProxy
    escaper: default
    listen: "127.0.0.1:<internal_port>"
```

When Kerberos or NTLM is the auth type, g3proxy is deliberately given `DirectFixed` because
the pre-processor's auth tunnel handles all proxied `CONNECT` connections itself.  g3proxy
then only sees direct connections forwarded to it by the pre-processor (plain-HTTP fallback
and exception-matched hosts).

---

## Connection Dispatcher

Each accepted connection passes through `handle_connection()` in order:

### 1. Magic IPC request

The first `MAGIC_SHOW_REQUEST.len()` bytes are peeked (non-consuming). If they match
`GET /__ferrovela/show HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n`:

- Bytes are consumed.
- `ProxySignal::Show` is sent to the UI via the mpsc channel.
- `HTTP/1.1 200 OK` is returned to the caller.
- Connection closes.

This preserves the single-instance IPC contract used by `main.rs`.

### 2. Auth tunnel (Kerberos / NTLM)

Activated when `needs_auth_tunnel()` returns `true`:

```
auth_type ∈ {"kerberos", "mock_kerberos", "ntlm"}
  AND upstream.proxy_url is Some
  AND authenticator is Some
```

`handle_authenticated_tunnel()` is called. It reads the client's HTTP request headers,
then branches:

- **`CONNECT` request** → `perform_authenticated_connect()` (see below) → splice.
- **Non-`CONNECT` request** → headers re-injected, connection forwarded to g3proxy.

### 3. Default (Basic auth / no auth)

Raw bytes forwarded to `127.0.0.1:<internal_port>` via `tokio::io::copy_bidirectional`.
g3proxy handles everything including the upstream ProxyHttp auth.

---

## Auth Tunnel: Challenge-Response Handshake

`perform_authenticated_connect()` in `src/proxy/auth_tunnel.rs` implements the
challenge-response loop used by both NTLM and Kerberos on a **single persistent TCP
connection** to the upstream proxy.

### Protocol loop

```
upstream = TcpStream::connect(proxy_addr)

CONNECT target  (no auth)
  → 200  ✓ done
  → 407  challenge₀ = Proxy-Authenticate header value

loop (max 4 rounds):
  token = session.step(challenge)   ← AuthSession trait
  CONNECT target + Proxy-Authorization: <token>
    → 200  ✓ done
    → 407  challenge = new Proxy-Authenticate value  → next round
```

`session.step()` is the `AuthSession` trait method from `src/auth/`:

| Auth type | Round 0 input | Round 0 output | Round 1 input | Round 1 output |
|---|---|---|---|---|
| **Kerberos** | `Some("Negotiate")` | `"Negotiate <AP-REQ>"` | _(typically 200)_ | — |
| **NTLM** | `Some("NTLM")` | `"NTLM <Type1>"` | `Some("NTLM <Type2 b64>")` | `"NTLM <Type3>"` |
| **mock_kerberos** | any | `"Negotiate MockKerberosToken"` | _(200 expected)_ | — |

The loop cap of 4 covers NTLM (2 auth rounds), Kerberos (1 auth round), and leaves
headroom for extended exchanges.

### PAC / exceptions integration

Inside the auth tunnel, `resolve_proxy(target, config, pac)` is called per connection.
This means:

- Hosts matching the exceptions list go **direct** (no upstream, no auth).
- PAC `DIRECT` results also go direct.
- PAC `PROXY <addr>` results use the auth tunnel with that specific proxy address.
- Static `upstream.proxy_url` is used as the fallback when no PAC is configured.

---

## Key Files

| File | Role |
|---|---|
| `src/proxy/mod.rs` | `Proxy` struct, `run()`, `build_g3proxy_yaml()`, `handle_connection()`, `resolve_proxy()` |
| `src/proxy/auth_tunnel.rs` | `perform_authenticated_connect()`, `handle_authenticated_tunnel()`, HTTP header I/O helpers |
| `src/proxy/http_utils.rs` | `parse_content_length()`, `find_header_value()` (used by auth_tunnel for body draining) |
| `src/auth/kerberos.rs` | `KerberosSession` — GSSAPI/SPNEGO token generation via `libgssapi` |
| `src/auth/ntlm.rs` | `NtlmSession` — NTLM Type1 / Type3 message generation via `ntlmclient` |
| `src/auth/basic.rs` | `BasicAuthenticator` — single-step Base64 credential header |
| `Cargo.toml` | `g3proxy` + `g3-daemon` git dependencies (ByteDance/g3, tag 1.13.x / 0.4.x) |

---

## Limitations

| Area | Limitation |
|---|---|
| **Plain HTTP via NTLM/Kerberos upstream** | Non-`CONNECT` requests are forwarded to g3proxy with `DirectFixed`; they do not go through the authenticated upstream proxy. HTTPS (`CONNECT`) is the dominant and fully supported case. |
| **PAC + NTLM/Kerberos** | PAC-based proxy selection is honoured inside the auth tunnel for `CONNECT`. Non-`CONNECT` plain HTTP requests bypassed to g3proxy do not go through PAC-selected proxies. |
| **g3proxy global state** | `g3proxy::config::load()` and the g3-daemon opts use `OnceLock` globals; `Proxy::run()` can only be called once per process. |
| **Internal port race** | There is a brief window between releasing the OS-assigned probe socket and g3proxy binding to the same port. In practice this is negligible on loopback but is not guaranteed race-free. |
