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

The application is built on **rama 0.4** and **Tokio** for high-performance asynchronous I/O. rama provides the TCP listener, HTTP/1.1 server, and a typed CONNECT upgrade pipeline.

### Workspace Layout

The repository is a Cargo workspace with three crates:

-   `crates/ferrovela-lib`: All proxy, auth, PAC, config, logging and launchd logic. Paths below are relative to `crates/ferrovela-lib/src/` unless stated otherwise.
-   `crates/ferrovela`: The proxy daemon binary (run as a launchd user agent).
-   `crates/ferrovela-ui`: The Iced configuration GUI, which also starts/stops the daemon.

### Core Components

1.  **Proxy Server (`proxy/`)**:
    -   `mod.rs`: Main proxy logic using rama's `TcpListener` → `HttpServer::auto` → `UpgradeLayer` stack.
        -   **`ConnectResponder`**: Handles `CONNECT` requests before the TCP upgrade — resolves the upstream proxy via PAC/config, applies the SSRF guard, attaches the routing decision (`ConnectRouting`) as an extension on the `UpgradeResponse`, then returns `200`. Returns an error response (403/400) to abort the upgrade if blocked.
        -   **`ConnectHandler`**: Runs on the raw upgraded socket — reads `ConnectRouting` from the `Upgraded` connection's extensions and either routes through the authenticated upstream proxy (`perform_authenticated_connect`), the unauthenticated upstream proxy, or connects directly.
        -   **`plain_http_handler`**: Handles all non-CONNECT requests — plain HTTP forwarding to upstream (with `Proxy-Authorization` for Basic auth) and direct plain HTTP.
    -   `auth_tunnel.rs`: Low-level authenticated CONNECT handshake (`perform_authenticated_connect`). Owns the multi-round 407 loop over a **single reused TCP connection** (required for stateful NTLM/Kerberos).
    -   `ssrf.rs`: Private-IP detection and SSRF guard.

2.  **Authentication (`auth/`)**:
    -   **Architecture**: Uses an `AuthSession` trait to handle multi-step authentication handshakes (essential for NTLM and Negotiate).
    -   **Components**:
        -   `UpstreamAuthenticator`: Factory trait to create sessions.
        -   `AuthSession`: Stateful session processing server challenges (407 responses).
        -   `BasicAuthenticator`: Stateless, sends credentials every time.
        -   `NtlmAuthenticator`: Implements NTLMv2 handshake (Type 1 → Challenge → Type 3) using `ntlmclient`. All three rounds share one TCP connection.
        -   `KerberosAuthenticator`: Implements SPNEGO/Kerberos handshake using `libgssapi`.

3.  **PAC Engine (`pac.rs`)**:
    -   Uses **rquickjs** (`rquickjs`), a QuickJS-based JavaScript engine, to execute PAC files.
    -   **PAC Fetch**: Remote PAC files (HTTP URLs) are always fetched using a DIRECT connection (`reqwest` with `.no_proxy()`), avoiding circular proxy dependencies.
    -   **Load-time validation**: PAC files are limited to 1 MiB, local paths are canonicalized and must not contain `..`, and the script is evaluated once up front — a script that fails to parse or does not define `FindProxyForURL` is rejected before any worker starts.
    -   **Threading Model**: Since `rquickjs::Context` is `!Send`, the JS execution logic runs in a pool of **dedicated OS threads** (`std::thread::Builder`, up to 4 workers, round-robin) with an 8 MB stack size each.
    -   **Evaluation timeout**: Each `FindProxyForURL` call is limited to 5 seconds via the QuickJS interrupt handler.
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

4.  **Configuration (`config.rs`)**:
    -   Stored in macOS preferences under the `com.ictcloud.ferrovela` domain via `core-foundation` (`CFPreferences`).
    -   Supports defining:
        -   Local listening port (`proxy_port`, default 3128).
        -   Bind IP address (`proxy_listen_ip`, default `127.0.0.1`). Exposed via `ProxyConfig::effective_listen_ip()`, which enforces the coupling with `allow_private_ips`: a non-loopback IP is only honoured when `allow_private_ips` is `true`, regardless of the stored value.
        -   PAC file location (local path or HTTP URL).
        -   Whether to allow proxying to private/RFC-1918 IP ranges (`proxy_allow_private_ips`).
        -   Upstream proxy details (URL, auth type, credentials, domain, workstation).
        -   Whether the upstream password lives in the system keyring (`upstream_use_keyring`, default `true`). When enabled, the password is stored under the service name `ferrovela` and is not written to preferences.
        -   Exception rules (hosts/domains to bypass proxy).

5.  **Service Management (`launchd.rs`)**:
    -   The daemon runs as a launchd user agent labelled `com.ictcloud.ferrovela`. `start()` writes `~/Library/LaunchAgents/com.ictcloud.ferrovela.plist`, then runs `launchctl bootstrap` + `kickstart`; `stop()` runs `launchctl bootout`.
    -   The daemon reads its configuration once at startup, so configuration changes require a service restart.
    -   Also owns the shared paths: the log file (`~/Library/Logs/ferrovela.log`) and the UI socket (`$TMPDIR/com.ictcloud.ferrovela.sock`).

6.  **Logging (`logger.rs`)**:
    -   A `log` backend that writes to a file at `Info` level, rotating at 10 MiB and keeping 3 backups.

7.  **User Interface (`crates/ferrovela-ui/src/ui/`)**:
    -   Built using **Iced** (`iced`) for a macOS GUI.
    -   Provides a tabbed configuration editor with four tabs: **Proxy**, **Upstream**, **Exceptions**, and **Advanced**, plus a separate log viewer window.
    -   Reads and writes settings via `CFPreferences` and controls the daemon through `launchd.rs` (start / stop / restart). It does not talk to the running proxy directly.
    -   **Single instance**: A second UI launch connects to the Unix socket of the first (mode `0600`, peer UID checked), which brings the existing window to the front; the second process then exits.
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
cargo build --release      # both binaries
./bundle.sh                # FerroVela.app (requires cargo-bundle), ad-hoc signed
```

### Run
The binaries take no command-line arguments; configuration is read from CFPreferences at startup.

```bash
./target/release/ferrovela       # proxy daemon in the foreground
./target/release/ferrovela-ui    # configuration GUI (starts the daemon via launchd)
```

### Configuration Example
Settings live in the `com.ictcloud.ferrovela` preferences domain and are normally edited through the GUI. They can also be set with `defaults`:

```bash
defaults write com.ictcloud.ferrovela proxy_port -int 3128
defaults write com.ictcloud.ferrovela proxy_pac_file -string "http://wpad/wpad.dat"
defaults write com.ictcloud.ferrovela upstream_auth_type -string "ntlm"
defaults write com.ictcloud.ferrovela upstream_username -string "user"
defaults write com.ictcloud.ferrovela upstream_domain -string "CORP"
defaults write com.ictcloud.ferrovela upstream_workstation -string "MYPC"
defaults write com.ictcloud.ferrovela upstream_proxy_url -string "http://10.0.0.1:3128"
defaults write com.ictcloud.ferrovela exceptions_hosts -array "localhost" "127.0.0.1" "*.internal"
```

Other keys: `proxy_allow_private_ips` (bool), `proxy_listen_ip` (string), `upstream_use_keyring` (bool), `upstream_password` (string, plain text; if set it takes precedence over the keyring — the GUI only writes it when the keyring is disabled). `upstream_auth_type` is one of `none`, `basic`, `ntlm`, `kerberos`.

## Developer Notes

-   **rquickjs & Async**: The `PacEngine` struct is the bridge between the async world and the synchronous, thread-local QuickJS runtime. Any new PAC functions must be added to `register_pac_functions` in `pac.rs`, which is called for the validation context and for every worker thread. Worker threads use `thread::Builder` with 8 MB stack size.
-   **PAC Fetch**: Remote PAC files are fetched with `reqwest::Client::builder().no_proxy()` to ensure DIRECT connections.
-   **rama extensions**: `ConnectRouting` is attached to the `UpgradeResponse` by `ConnectResponder` and read from the `Upgraded` connection's extensions by `ConnectHandler`. This avoids repeating PAC evaluation across the upgrade boundary.
-   **Tests and CFPreferences**: Tests that read or write preferences must hold `config::PREFS_LOCK`, since they share the real `com.ictcloud.ferrovela` domain.
-   **NTLM connection reuse**: All three rounds of an NTLM handshake (Negotiate → Challenge → Authenticate) must share a single TCP connection. `perform_authenticated_connect` in `auth_tunnel.rs` enforces this.
-   **Error Handling**: The application uses `anyhow` for error propagation and `log` for observability.

## Authentication Implementation

### Kerberos (MacOS First)
- Implemented using `libgssapi` (GSSAPI wrapper).
- **SPN Construction**: The Service Principal Name is derived per CONNECT from the upstream proxy actually selected (PAC result or static `proxy_url`): `HTTP@<hostname>`. No static `proxy_url` is required when a PAC file is used.
- **Assumptions**:
    - The host machine has a valid Kerberos TGT (via `kinit` or domain login).
- **Handshake**: Fully implemented via `AuthSession::step`. Handles `Negotiate` challenges.
- **407 handling**: Response headers are read without consuming anything past `\r\n\r\n`; the 407 error body is then drained by `Content-Length` so the connection stays usable. If the proxy answers with `Connection: close` / `Proxy-Connection: close`, the authenticated retry opens a new TCP connection.

### NTLM
- Implemented using `ntlmclient` crate.
- Supports NTLMv2.
- **Handshake**: Fully implemented (Type 1 → Type 2 → Type 3) inside `perform_authenticated_connect`.
- **Connection reuse**: All three rounds reuse a single TCP connection — the server's Type 2 challenge is tied to the session, so opening a new connection would break the handshake.

## Future Work
1.  Implement actual DNS resolution for `dnsResolve` in PAC (currently returns host as-is).
2.  Implement `myIpAddress` to return real interface IP (currently returns 127.0.0.1).
3.  Implement real `weekdayRange`, `dateRange`, `timeRange` (currently stubs returning true).
