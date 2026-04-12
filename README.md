# FerroVela

A Rust-based local proxy designed for developers, offering robust configuration and upstream authentication support.

## Features

- **PAC File Support**: Seamlessly reads and interprets `.pac` files for dynamic proxy configuration. PAC files are always fetched using a DIRECT connection (no proxy). All standard PAC helper functions are supported, including `isPlainHostName`, `dnsDomainIs`, `localHostOrDomainIs`, `isResolvable`, `isInNet`, `dnsDomainLevels`, `shExpMatch`, and more.
-   **Protocol Support**: Routes both HTTP (GET, POST, etc.) and HTTPS (CONNECT) traffic efficiently.
- **Smart Exceptions**: configure exceptions based on domains, glob patterns, or IP addresses.
- **Corporate Authentication**: Supports Basic, NTLM, and Kerberos authentication for upstream proxies.
- **MDM Ready**: Configuration stored via macOS CFPreferences — deploy settings across a fleet with `.mobileconfig` profiles.
- **Developer Friendly**:
    - Detailed logging with automatic rotation: the log file (`~/Library/Logs/ferrovela.log`) rotates at 10 MiB, keeping three compressed backups (`.1`–`.3`).
    - Simple configuration via the GUI or `defaults` command.

## Configuration

### Graphical Interface

FerroVela includes a graphical interface for managing the configuration.

To launch the configuration editor:
```bash
./target/release/ferrovela
```

The UI provides three configuration tabs — **Proxy**, **Upstream**, and **Exceptions** — with the following behaviours:

- **Save feedback**: every change is persisted immediately; a green confirmation or red error message appears below the service toggle and auto-dismisses after a few seconds.
- **Progressive disclosure**: the Upstream tab shows only the fields relevant to the chosen authentication type (e.g. Domain and Workstation are hidden unless NTLM is selected).
- **Restart banner**: if the proxy service is running when settings are changed, a warning banner appears with a *Restart Now* button to apply the new configuration without manually toggling the service off and on.

### Authentication Types

FerroVela supports multiple authentication methods for upstream proxies:

#### NTLM

To use NTLM authentication:
1. Set `upstream_auth_type` to `ntlm`.
2. Provide `upstream_username`, `upstream_password`, `upstream_domain`, and `upstream_workstation`.

```bash
defaults write com.ictcloud.ferrovela upstream_auth_type -string "ntlm"
defaults write com.ictcloud.ferrovela upstream_username -string "user"
defaults write com.ictcloud.ferrovela upstream_password -string "password"
defaults write com.ictcloud.ferrovela upstream_domain -string "CORP"
defaults write com.ictcloud.ferrovela upstream_workstation -string "MYPC"
defaults write com.ictcloud.ferrovela upstream_proxy_url -string "http://proxy.corp.com:8080"
```

#### Kerberos

To use Kerberos authentication:
1. Ensure your machine is joined to the domain or you have a valid Kerberos ticket (obtainable via `kinit`).
2. Set `upstream_auth_type` to `kerberos`.
3. FerroVela will automatically use the cached credentials (TGT) to authenticate with the upstream proxy using SPNEGO.

> In the GUI, selecting *Kerberos* hides the password field — no password is required or stored.

```bash
defaults write com.ictcloud.ferrovela upstream_auth_type -string "kerberos"
defaults write com.ictcloud.ferrovela upstream_proxy_url -string "http://proxy.corp.com:8080"
```

#### Basic

To use Basic authentication:

```bash
defaults write com.ictcloud.ferrovela upstream_auth_type -string "basic"
defaults write com.ictcloud.ferrovela upstream_username -string "user"
defaults write com.ictcloud.ferrovela upstream_password -string "password"
defaults write com.ictcloud.ferrovela upstream_proxy_url -string "http://proxy.corp.com:8080"
```

### Manual Configuration

Configuration is stored in macOS preferences under the `com.ictcloud.ferrovela` domain. You can read and write settings using the `defaults` command:

```bash
# Set proxy port and PAC file
defaults write com.ictcloud.ferrovela proxy_port -int 3128
defaults write com.ictcloud.ferrovela proxy_pac_file -string "http://wpad/wpad.dat"

# Set exceptions
defaults write com.ictcloud.ferrovela exceptions_hosts -array "localhost" "127.0.0.1" "*.local"

# Read current configuration
defaults read com.ictcloud.ferrovela
```

### MDM Deployment

For fleet-wide configuration, create a `.mobileconfig` profile targeting the `com.ictcloud.ferrovela` preference domain. MDM-managed (forced) preferences automatically override user-set values.

Available preference keys:

| Key | Type | Description |
|---|---|---|
| `proxy_port` | Integer | Local proxy port (default: 3128) |
| `proxy_pac_file` | String | URL or path to PAC file |
| `proxy_allow_private_ips` | Boolean | Allow proxying to private IPs |
| `upstream_auth_type` | String | `none`, `basic`, `ntlm`, or `kerberos` |
| `upstream_username` | String | Upstream proxy username |
| `upstream_password` | String | Upstream proxy password (see credential security note below) |
| `upstream_use_keyring` | Boolean | Store password in system keychain (recommended) |
| `upstream_domain` | String | NTLM domain |
| `upstream_workstation` | String | NTLM workstation |
| `upstream_proxy_url` | String | Upstream proxy URL |
| `exceptions_hosts` | Array of String | Hosts to bypass proxy |

> **Credential security**: When `upstream_use_keyring` is `false`, the password is stored as plaintext in the macOS preferences database. Enable `upstream_use_keyring` to store credentials in the system keychain instead. Credentials are escaped before being written to the internal proxy configuration, so special characters (quotes, backslashes, newlines) in passwords are handled safely.

## Development Container

A `.devcontainer` configuration is included for use with VS Code Dev Containers or GitHub Codespaces. It provides Rust, rust-analyzer, and all required native build dependencies (clang, Kerberos headers, X11/Wayland/GL libs). Note that because FerroVela is macOS-only, the container is useful for editing, code navigation, and running unit tests — a full binary build requires macOS.

## Requirements

- **macOS** — FerroVela is a macOS-only application. It relies on launchd, CFPreferences, and the macOS Kerberos framework.
- **Rust (latest stable)** — install via [rustup](https://rustup.rs).
- **Xcode Command Line Tools** — provides the Kerberos headers and `codesign` used during the build:
  ```bash
  xcode-select --install
  ```
- **cargo-bundle** — packages the app as a `.app` bundle:
  ```bash
  cargo install cargo-bundle
  ```

## Building and Running

1. **Build the app bundle**:
   ```bash
   ./bundle.sh
   ```
   This compiles both binaries in release mode, assembles the macOS `.app` bundle, copies the proxy service binary into it, and ad-hoc codesigns all three artifacts with the required network entitlements. The resulting bundle is at:
   ```
   target/release/bundle/osx/FerroVela.app
   ```

2. **Run**: Open `FerroVela.app` from Finder or from the terminal:
   ```bash
   open target/release/bundle/osx/FerroVela.app
   ```

   > **First launch — Gatekeeper**: Because the bundle uses ad-hoc signing (not a Developer ID), macOS will block it on the first open. Right-click the app in Finder and choose **Open**, then confirm the prompt. You only need to do this once.

### Development builds

To run the UI directly without assembling a full bundle (useful during development):
```bash
cargo build --release -p ferrovela -p ferrovela-ui
./target/release/ferrovela-ui
```
The UI will look for the `ferrovela` proxy binary alongside itself, so both must be built before launching.

## Performance Testing

FerroVela includes a performance test suite to measure throughput and latency of the proxy logic.

To run the performance tests:
```bash
cargo test performance_tests -- --nocapture
```

The output will display metrics such as:
- Total Requests
- Successful Requests
- Total Duration
- Requests Per Second (RPS)

Example output:
```
Starting performance test with 50 clients, 100 requests each (Total: 5000)
Performance Test Results:
Total Requests: 5000
Successful Requests: 5000
Total Duration: 800.16ms
Requests Per Second (RPS): 6248.71
```

Criterion micro-benchmarks covering the CONNECT path, header parsing, and request serialization are also available:

```bash
# Save a baseline before making changes
cargo bench -p ferrovela-lib -- --save-baseline main

# Compare against the baseline after changes
cargo bench -p ferrovela-lib -- --baseline main
```

### Performance design notes

- **Release profile**: compiled with `opt-level = 3` (speed-optimized, not size-optimized). This enables autovectorization and aggressive inlining across the whole binary.
- **CONNECT hot path**: header bytes are decoded zero-copy (`String::from_utf8` ownership transfer). The `Proxy-Authenticate` header is parsed without heap allocation (`memchr::memmem` + borrowed `&str`). NTLM/Basic credentials are held as `Arc<str>` — `create_session()` is atomic reference-count increments, not heap copies.
- **Plain HTTP path**: request headers are serialized by writing byte slices directly into a pre-allocated `Vec<u8>` — no intermediate per-header `String` allocations. Basic `Proxy-Authorization` is pre-computed once at startup. Response body bytes are extracted in-place via `drain` (memmove, no second allocation).
- **Exception matching**: host patterns are pre-compiled at config load into a `HashSet` (exact matches, O(1)) and a suffix `Vec` (wildcard patterns, O(n)); the per-request scan of the raw pattern list is eliminated.

## Dependencies

- `rama`: HTTP proxy framework providing the TCP listener, HTTP/1.1 server, and CONNECT upgrade pipeline.
- `tokio`: Asynchronous runtime.
- `core-foundation`: macOS CFPreferences integration for configuration storage.
- `rquickjs`: JavaScript engine for PAC file evaluation.
- `reqwest`: HTTP client for remote PAC file fetching (DIRECT, no-proxy).
- `iced`: Graphical user interface framework.
- `keyring`: Stores credentials in the macOS system keychain.
- `libgssapi`: Kerberos/GSSAPI integration (uses the macOS Kerberos framework; requires Xcode Command Line Tools).
- `ntlmclient`: NTLMv2 authentication.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for a detailed list of changes.

