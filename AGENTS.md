# FerroVela - Agent Documentation

## Goals, restrictions and output for agents

This section describes the goals, the restrictions and the expected output for agents that work on this repository.

### Goals

- An agent is the solution architect for the requested tasks and outlines a description for it.
- An agent is the software engineer for the requested feature and implements a solution for it.
- An agent is the security engineer for the requested feature and takes measures to guarantee the security of the solution.
- An agent is the testing engineer for the requested task and proposes appropriate tests for it.

### Restrictions

- Any request for a new feature must be based on the DEV branch if not otherwise specified.
- The agent adheres to the standards described in this document.
- If points are not clear an has to ask for clarification.
- If not other specified any documentation should be in the README.md or in the folder "specification".
- Any Rust code must comply with ```cargo fmt``` and ```cargo clippy -- -D warnings```

### Output

- The agent provides a detailed description of the solution.
- The agent provides a detailed implementation of the solution.
- The agent provides a detailed security analysis of the solution.
- The agent provides a detailed testing plan for the solution.

## Project Overview

FerroVela is a Rust-based local proxy server designed to route HTTP/HTTPS traffic for developer machines. It supports dynamic proxy configuration via PAC files, static upstream proxy chaining, and domain-based exceptions.

## Architecture

The application is built on **rama 0.2** and **Tokio** for high-performance asynchronous I/O. rama provides the TCP listener, HTTP/1.1 server, and a typed CONNECT upgrade pipeline.

### Core Components

1.  **Proxy Server (`src/proxy/`)**:
    -   `mod.rs`: Main proxy logic using rama's `TcpListener` → `HttpServer::auto` → `UpgradeLayer` stack.
        -   **`ConnectResponder`**: Handles `CONNECT` requests before the TCP upgrade — resolves the upstream proxy via PAC/config, applies the SSRF guard, stores the routing decision in the rama `Context`, then returns `200`. Returns an error response (403/400) to abort the upgrade if blocked.
        -   **`ConnectHandler`**: Runs on the raw upgraded socket — reads `ConnectRouting` from the `Context` and either routes through the authenticated upstream proxy (`perform_authenticated_connect`), the unauthenticated upstream proxy, or connects directly.
        -   **`plain_http_handler`**: Handles all non-CONNECT requests — magic IPC endpoint, plain HTTP forwarding to upstream, and direct plain HTTP.
    -   `auth_tunnel.rs`: Low-level authenticated CONNECT handshake (`perform_authenticated_connect`). Owns the multi-round 407 loop over a **single reused TCP connection** (required for stateful NTLM/Kerberos).
    -   `ssrf.rs`: Private-IP detection and SSRF guard.
    -   `http_utils.rs`: HTTP parsing helpers.

2.  **Authentication (`src/auth/`)**:
    -   **Architecture**: Uses an `AuthSession` trait to handle multi-step authentication handshakes (essential for NTLM and Negotiate).
    -   **Components**:
        -   `UpstreamAuthenticator`: Factory trait to create sessions.
        -   `AuthSession`: Stateful session processing server challenges (407 responses).
        -   `BasicAuthenticator`: Stateless, sends credentials every time.
        -   `NtlmAuthenticator`: Implements NTLMv2 handshake (Type 1 → Challenge → Type 3) using `ntlmclient`. All three rounds share one TCP connection.
        -   `KerberosAuthenticator`: Implements SPNEGO/Kerberos handshake using `libgssapi`.

3.  **PAC Engine (`src/pac.rs`)**:
    -   Uses **rquickjs** (`rquickjs`), a QuickJS-based JavaScript engine, to execute PAC files.
    -   **PAC Fetch**: Remote PAC files (HTTP URLs) are always fetched using a DIRECT connection (`reqwest` with `.no_proxy()`), avoiding circular proxy dependencies.
    -   **Threading Model**: Since `rquickjs::Context` is `!Send`, the JS execution logic runs in **dedicated OS threads** (`std::thread::Builder`) with an 8 MB stack size.
    -   **Communication**: The main Tokio runtime communicates with the PAC threads via `tokio::sync::mpsc` channels for requests and `tokio::sync::oneshot` for responses.
    -   **Implemented JS Functions** (full PAC spec coverage):
        -   `isPlainHostName(host)`: Returns true if hostname has no dots.
        -   `dnsDomainIs(host, domain)`: Returns true if host ends with domain.
        -   `localHostOrDomainIs(host, hostdom)`: Returns true if exact match or unqualified host matches.
        -   `isResolvable(host)`: Returns true if DNS resolves the host.
        -   `isInNet(host, pattern, mask)`: Returns true if resolved host IP matches network/mask.
        -   `dnsResolve(host)`: *Mocked* (returns host).
        -   `dnsDomainLevels(host)`: Returns number of dots in hostname.
        -   `myIpAddress()`: *Mocked* (returns 127.0.0.1).
        -   `shExpMatch(str, pattern)`: Implemented using glob matching (supports `*` and `?`).
        -   `convert_addr(ipaddr)`: Converts dotted IP string to integer.
        -   `weekdayRange(...)`: *Stub* (returns true).
        -   `dateRange(...)`: *Stub* (returns true).
        -   `timeRange(...)`: *Stub* (returns true).

4.  **Configuration (`src/config.rs`)**:
    -   Stored in macOS preferences under the `com.ictcloud.ferrovela` domain via `core-foundation` (`CFPreferences`).
    -   Supports defining:
        -   Local listening port (`proxy_port`, default 3128).
        -   Bind IP address (`proxy_listen_ip`, default `127.0.0.1`). Exposed via `ProxyConfig::effective_listen_ip()`, which enforces the coupling with `allow_private_ips`: a non-loopback IP is only honoured when `allow_private_ips` is `true`, regardless of the stored value.
        -   PAC file location (local path or HTTP URL).
        -   Whether to allow proxying to private/RFC-1918 IP ranges (`proxy_allow_private_ips`).
        -   Upstream proxy details (URL, auth type, credentials, domain, workstation).
        -   Exception rules (hosts/domains to bypass proxy).

5.  **User Interface (`crates/ferrovela-ui/src/ui/`)**:
    -   Built using **Iced** (`iced`) for a macOS GUI.
    -   Provides a tabbed configuration editor with four tabs: **Proxy**, **Upstream**, **Exceptions**, and **Advanced**.
    -   Reads and writes settings via `CFPreferences`; signals the running proxy daemon via a magic HTTP request.
    -   **Advanced tab** (`ui/auth.rs`, `ui/view.rs`): Exposes `Allow private IPs` and `Listen IP`. Both controls are locked by default; clicking the lock triggers the macOS Authorization Services sheet (`system.preferences` right) on a background thread via `tokio::task::spawn_blocking` → `iced::Task::perform`. The tab relocks automatically on navigation away. The `Listen IP` field is only editable when `Allow private IPs` is also enabled, mirroring the library-level coupling.

## Current Status & Capabilities

-   [x] **HTTPS Tunneling**: Full support for `CONNECT` method.
-   [x] **Standard HTTP Proxying**: Support for GET, POST, etc.
-   [x] **PAC Support**: 
    -   Remote (HTTP) and Local file loading.
    -   `FindProxyForURL` execution.
-   [x] **Exceptions**:
    -   Exact match (`localhost`).
    -   Wildcard/Glob match (`*.local`).
-   [x] **Upstream Auth**:
    -   **Basic**: Implemented.
    -   **Kerberos**: Implemented (Full Handshake).
    -   **NTLM**: Implemented (Full Handshake).

## Usage

### Build
```bash
cargo build --release
```

### Run
```bash
./target/release/ferrovela --config config.json
```

### Configuration Example
```json
{
  "proxy": {
    "port": 3128,
    "pac_file": "http://wpad/wpad.dat"
  },
  "upstream": {
    "auth_type": "ntlm",
    "username": "user",
    "password": "password",
    "domain": "CORP",
    "workstation": "MYPC",
    "proxy_url": "10.0.0.1:3128"
  },
  "exceptions": {
    "hosts": ["localhost", "127.0.0.1", "*.internal"]
  }
}
```

## Developer Notes

-   **rquickjs & Async**: The `PacEngine` struct is the bridge between the async world and the synchronous, thread-local QuickJS runtime. Any new PAC functions must be registered inside the spawned thread closure in `src/pac.rs`. Worker threads use `thread::Builder` with 8 MB stack size.
-   **PAC Fetch**: Remote PAC files are fetched with `reqwest::Client::builder().no_proxy()` to ensure DIRECT connections.
-   **rama Context extensions**: `ConnectRouting` is inserted into the rama `Context` by `ConnectResponder` and consumed by `ConnectHandler`. This avoids repeating PAC evaluation across the upgrade boundary.
-   **NTLM connection reuse**: All three rounds of an NTLM handshake (Negotiate → Challenge → Authenticate) must share a single TCP connection. `perform_authenticated_connect` in `auth_tunnel.rs` enforces this.
-   **Error Handling**: The application uses `anyhow` for error propagation and `log` for observability.

## Authentication Implementation

### Kerberos (MacOS First)
- Implemented using `libgssapi` (GSSAPI wrapper).
- **SPN Construction**: The Service Principal Name is derived from the upstream proxy hostname: `HTTP@<hostname>`.
- **Assumptions**:
    - The host machine has a valid Kerberos TGT (via `kinit` or domain login).
- **Handshake**: Fully implemented via `AuthSession::step`. Handles `Negotiate` challenges.

### NTLM
- Implemented using `ntlmclient` crate.
- Supports NTLMv2.
- **Handshake**: Fully implemented (Type 1 → Type 2 → Type 3) inside `perform_authenticated_connect`.
- **Connection reuse**: All three rounds reuse a single TCP connection — the server's Type 2 challenge is tied to the session, so opening a new connection would break the handshake.

## Future Work
1.  Implement actual DNS resolution for `dnsResolve` in PAC (currently returns host as-is).
2.  Implement `myIpAddress` to return real interface IP (currently returns 127.0.0.1).
3.  Implement real `weekdayRange`, `dateRange`, `timeRange` (currently stubs returning true).
4.  Add Keyring integration for secure credential storage (instead of plain text config).
