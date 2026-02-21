# FerroVela - Agent Documentation

## Project Overview
FerroVela is a Rust-based local proxy server designed to route HTTP/HTTPS traffic for developer machines. It supports dynamic proxy configuration via PAC files, static upstream proxy chaining, and domain-based exceptions.

## Architecture

The application is built on **Hyper 1.0** and **Tokio** for high-performance asynchronous I/O.

### Core Components

1.  **Proxy Server (`src/proxy.rs`)**:
    -   Uses `hyper::server::conn::http1` to handle incoming connections.
    -   **HTTPS/Tunneling**: Implements the `CONNECT` method to create TCP tunnels to target servers or upstream proxies.
    -   **Upstream Chaining**: Can forward traffic to a parent proxy defined in `config.toml` or returned by the PAC script.
    -   **Authentication**: Currently supports **Basic Authentication** for upstream proxies.

2.  **PAC Engine (`src/pac.rs`)**:
    -   Uses **Boa (`boa_engine`)**, a pure Rust JavaScript engine, to execute PAC files.
    -   **Threading Model**: Since `boa_engine::Context` is `!Send`, the JS execution logic runs in a **dedicated OS thread** (`std::thread`).
    -   **Communication**: The main Tokio runtime communicates with the PAC thread via `tokio::sync::mpsc` channels for requests and `tokio::sync::oneshot` for responses.
    -   **Implemented JS Functions**:
        -   `dnsResolve(host)`: *Mocked* (returns host).
        -   `myIpAddress()`: *Mocked* (returns 127.0.0.1).
        -   `shExpMatch(str, pattern)`: Implemented using regex (supports `*` and `?`).

3.  **Configuration (`src/config.rs`)**:
    -   Managed via `config.toml`.
    -   Parsed using `serde` and `toml`.
    -   Supports defining:
        -   Local listening port.
        -   PAC file location (local path or HTTP URL).
        -   Upstream proxy details (URL, Auth type, Credentials).
        -   Exception rules (Hosts/Domains to bypass proxy).

4.  **User Interface (`src/ui.rs`)**:
    -   Built using **Iced** (`iced`) for a cross-platform GUI.
    -   Provides a form-based editor for `config.toml`.
    -   Launched via the `--ui` command-line flag.
    -   Synchronous save to disk using `serde` and `toml`.

## Current Status & Capabilities

-   [x] **HTTPS Tunneling**: Full support for `CONNECT` method.
-   [x] **PAC Support**: 
    -   Remote (HTTP) and Local file loading.
    -   `FindProxyForURL` execution.
-   [x] **Exceptions**:
    -   Exact match (`localhost`).
    -   Wildcard/Glob match (`*.local`).
-   [x] **Upstream Auth**:
    -   **Basic**: Implemented.
    -   **NTLM/Kerberos**: *Not Implemented* (Architecture allows for it, but handshake logic is missing).
-   [ ] **Plain HTTP Proxying**: Currently returns 501 Not Implemented for non-CONNECT requests (standard HTTP proxying). Focus is on HTTPS tunneling.

## Usage

### Build
```bash
cargo build --release
```

### Run
```bash
./target/release/ferrovela --config config.toml
```

### Configuration Example
```toml
[proxy]
port = 8080
pac_file = "http://wpad/wpad.dat" 

[upstream]
auth_type = "basic"
username = "user"
password = "password"
proxy_url = "10.0.0.1:3128"

[exceptions]
hosts = ["localhost", "127.0.0.1", "*.internal"]
```

## Developer Notes

-   **Boa & Async**: The `PacEngine` struct is the bridge between the async world and the synchronous, thread-local Boa engine. Any new PAC functions must be registered inside the spawned thread closure in `src/pac.rs`.
-   **Error Handling**: The application uses `anyhow` for error propagation and `log` for observability.
-   **Security**: Credentials in `config.toml` are read as plain text.

## Future Work
1.  Implement actual DNS resolution for `dnsResolve` in PAC.
2.  Implement `myIpAddress` to return real interface IP.
3.  Add full NTLM/Kerberos handshake support for corporate proxies.
4.  Implement standard HTTP forwarding (GET/POST/etc) for non-SSL traffic.
