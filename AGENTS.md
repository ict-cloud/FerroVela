# FerroVela - Agent Documentation

## Project Overview
FerroVela is a Rust-based local proxy server designed to route HTTP/HTTPS traffic for developer machines. It supports dynamic proxy configuration via PAC files, static upstream proxy chaining, and domain-based exceptions.

## Architecture

The application is built on **Hyper 1.0** and **Tokio** for high-performance asynchronous I/O.

### Core Components

1.  **Proxy Server (`src/proxy/`)**:
    -   Uses `hyper::server::conn::http1` to handle incoming connections.
    -   **Modular Design**: Logic is split into `connect.rs` (HTTPS tunneling) and `nonconnect.rs` (standard HTTP proxying).
    -   **HTTPS/Tunneling**: Implements the `CONNECT` method to create TCP tunnels to target servers or upstream proxies.
    -   **Standard HTTP**: Implements standard HTTP forwarding (GET, POST, etc.) for non-SSL traffic.
    -   **Upstream Chaining**: Can forward traffic to a parent proxy defined in `config.toml` or returned by the PAC script.
    -   **Authentication**: Supports **Basic**, **NTLM**, and **Kerberos** (Negotiate) authentication for upstream proxies.

2.  **Authentication (`src/auth/`)**:
    -   **Architecture**: Uses an `AuthSession` trait to handle multi-step authentication handshakes (essential for NTLM and Negotiate).
    -   **Components**:
        -   `UpstreamAuthenticator`: Factory trait to create sessions.
        -   `AuthSession`: Stateful session processing server challenges (407 responses).
        -   `BasicAuthenticator`: Stateless, sends credentials every time.
        -   `NtlmAuthenticator`: Implements NTLMv2 handshake (Type 1 -> Challenge -> Type 3) using `ntlmclient`.
        -   `KerberosAuthenticator`: Implements SPNEGO/Kerberos handshake using `libgssapi`.

3.  **PAC Engine (`src/pac.rs`)**:
    -   Uses **Boa (`boa_engine`)**, a pure Rust JavaScript engine, to execute PAC files.
    -   **Threading Model**: Since `boa_engine::Context` is `!Send`, the JS execution logic runs in a **dedicated OS thread** (`std::thread`).
    -   **Communication**: The main Tokio runtime communicates with the PAC thread via `tokio::sync::mpsc` channels for requests and `tokio::sync::oneshot` for responses.
    -   **Implemented JS Functions**:
        -   `dnsResolve(host)`: *Mocked* (returns host).
        -   `myIpAddress()`: *Mocked* (returns 127.0.0.1).
        -   `shExpMatch(str, pattern)`: Implemented using regex (supports `*` and `?`).

4.  **Configuration (`src/config.rs`)**:
    -   Managed via `config.toml`.
    -   Parsed using `serde` and `toml`.
    -   Supports defining:
        -   Local listening port.
        -   PAC file location (local path or HTTP URL).
        -   Upstream proxy details (URL, Auth type, Credentials, Domain, Workstation).
        -   Exception rules (Hosts/Domains to bypass proxy).

5.  **User Interface (`src/ui.rs`)**:
    -   Built using **Iced** (`iced`) for a cross-platform GUI.
    -   Provides a form-based editor for `config.toml`.
    -   Launched via the `--ui` command-line flag.
    -   Synchronous save to disk using `serde` and `toml`.

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
./target/release/ferrovela --config config.toml
```

### Configuration Example
```toml
[proxy]
port = 3128
pac_file = "http://wpad/wpad.dat" 

[upstream]
auth_type = "ntlm"
username = "user"
password = "password"
domain = "CORP"
workstation = "MYPC"
proxy_url = "10.0.0.1:3128"

[exceptions]
hosts = ["localhost", "127.0.0.1", "*.internal"]
```

## Developer Notes

-   **Boa & Async**: The `PacEngine` struct is the bridge between the async world and the synchronous, thread-local Boa engine. Any new PAC functions must be registered inside the spawned thread closure in `src/pac.rs`.
-   **Error Handling**: The application uses `anyhow` for error propagation and `log` for observability.
-   **Security**: Credentials in `config.toml` are read as plain text.

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
- **Handshake**: Fully implemented (Type 1 -> Type 2 -> Type 3).
- **HTTP Handling**: Buffers request bodies to allow replaying requests during the handshake loop.

## Future Work
1.  Implement actual DNS resolution for `dnsResolve` in PAC.
2.  Implement `myIpAddress` to return real interface IP.
3.  Add Keyring integration for secure credential storage (instead of plain text config).
