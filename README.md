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

## Building and Running

1.  **Install Rust**: Ensure you have Rust and Cargo installed.
2.  **Install Dependencies**:
    -   On Ubuntu/Debian: `sudo apt-get install libkrb5-dev libgssapi-krb5-2`
    -   On MacOS: usually installed by default (via Xcode Command Line Tools).
3.  **Install cargo-bundle**: `cargo install cargo-bundle`
4.  **Build the app bundle**:
    ```bash
    ./bundle.sh
    ```
    This builds both binaries, creates the macOS app bundle, and copies the proxy service into it. The resulting bundle is at `target/release/bundle/osx/FerroVela.app`.
5.  **Run**: Open `FerroVela.app` or run `./target/release/ferrovela-ui` directly.

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

## Dependencies

- `g3proxy`: Proxy engine for upstream chaining (ByteDance).
- `tokio`: Asynchronous runtime.
- `core-foundation`: macOS CFPreferences integration for configuration storage.
- `rquickjs`: JavaScript engine for PAC file evaluation.
- `reqwest`: HTTP client for remote PAC file fetching (DIRECT, no-proxy).
- `iced`: For the graphical user interface.
- `libgssapi`: For Kerberos/GSSAPI integration.
- `ntlmclient`: For NTLMv2 authentication.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for a detailed list of changes.

