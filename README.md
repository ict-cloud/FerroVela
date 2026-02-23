# FerroVela

A Rust-based local proxy designed for developers, offering robust configuration and upstream authentication support.

## Features

- **PAC File Support**: Seamlessly reads and interprets `.pac` files for dynamic proxy configuration.
-   **Protocol Support**: Routes both HTTP (GET, POST, etc.) and HTTPS (CONNECT) traffic efficiently.
- **Smart Exceptions**: configure exceptions based on domains, glob patterns, or IP addresses.
- **Corporate Authentication**: Supports Basic, NTLM, and Kerberos authentication for upstream proxies.
- **Developer Friendly**: 
    - Easy debugging with detailed logging.
    - Simple configuration via a TOML file.

## Configuration

### Graphical Interface

FerroVela includes a simple graphical interface for managing the configuration.

To launch the configuration editor:
```bash
./target/release/ferrovela --ui
```

### Authentication Types

FerroVela supports multiple authentication methods for upstream proxies:

#### NTLM

To use NTLM authentication:
1. Set `auth_type = "ntlm"` in `config.toml`.
2. Provide `username`, `password`, `domain`, and `workstation`.

```toml
[upstream]
auth_type = "ntlm"
username = "user"
password = "password"
domain = "CORP"
workstation = "MYPC"
proxy_url = "http://proxy.corp.com:8080"
```

#### Kerberos

To use Kerberos authentication:
1. Ensure your machine is joined to the domain or you have a valid Kerberos ticket (obtainable via `kinit`).
2. Set `auth_type = "kerberos"` in `config.toml`.
3. FerroVela will automatically use the cached credentials (TGT) to authenticate with the upstream proxy using SPNEGO.

```toml
[upstream]
auth_type = "kerberos"
proxy_url = "http://proxy.corp.com:8080"
# username/password/domain are ignored for Kerberos (uses system ticket)
```

#### Basic

To use Basic authentication:

```toml
[upstream]
auth_type = "basic"
username = "user"
password = "password"
proxy_url = "http://proxy.corp.com:8080"
```

### Manual Configuration

Configuration can also be manually managed through a `config.toml` file.

```toml
[proxy]
port = 3128
pac_file = "http://wpad/wpad.dat" # or local path

[upstream]
auth_type = "ntlm" # or "basic", "kerberos", "none"
username = "user"
password = "password"
domain = "CORP" # Required for NTLM
workstation = "WORKSTATION" # Optional for NTLM
proxy_url = "http://upstream:8080"

[exceptions]
# bypass upstream proxy for these
hosts = ["localhost", "127.0.0.1", "*.local"]
```

## Building and Running

1.  **Install Rust**: Ensure you have Rust and Cargo installed.
2.  **Install Dependencies**:
    -   On Ubuntu/Debian: `sudo apt-get install libkrb5-dev libgssapi-krb5-2`
    -   On MacOS: usually installed by default (via Xcode Command Line Tools).
3.  **Build**: `cargo build --release`
4.  **Run**: `./target/release/ferrovela`

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

- `hyper`: For low-level HTTP handling.
- `tokio`: Asynchronous runtime.
- `serde`/`toml`: Configuration parsing.
- `iced`: For the graphical user interface.
- `libgssapi`: For Kerberos/GSSAPI integration.
- `ntlmclient`: For NTLMv2 authentication.

## Running as a Service on MacOS

To run FerroVela as a background service on MacOS using `launchd`, you can use the provided installation script.

1.  **Run the install script**:
    ```bash
    ./service/macos/install.sh
    ```

    This script will:
    -   Build the release binary.
    -   Install the binary to `~/.local/bin/ferrovela`.
    -   Install the configuration to `~/.config/ferrovela/config.toml` (if not already present).
    -   Create and load a `launchd` plist at `~/Library/LaunchAgents/com.ferrovela.plist`.

2.  **Manage the service**:
    -   **Stop**: `launchctl unload ~/Library/LaunchAgents/com.ferrovela.plist`
    -   **Start**: `launchctl load ~/Library/LaunchAgents/com.ferrovela.plist`
    -   **Logs**: Check `/tmp/ferrovela.log` and `/tmp/ferrovela.err`.

Ensure `~/.local/bin` is in your `PATH` if you want to run `ferrovela` manually from the command line.
