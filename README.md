# FerroVela

A Rust-based local proxy designed for developers, offering robust configuration and upstream authentication support.

## Features

- **PAC File Support**: Seamlessly reads and interprets `.pac` files for dynamic proxy configuration. PAC files are always fetched using a DIRECT connection (no proxy). All standard PAC helper functions are supported, including `isPlainHostName`, `dnsDomainIs`, `localHostOrDomainIs`, `isResolvable`, `isInNet`, `dnsDomainLevels`, `shExpMatch`, and more.
-   **Protocol Support**: Routes both HTTP (GET, POST, etc.) and HTTPS (CONNECT) traffic efficiently.
- **Smart Exceptions**: configure exceptions based on domains, glob patterns, or IP addresses.
- **Corporate Authentication**: Supports Basic, NTLM, and Kerberos authentication for upstream proxies.
- **Developer Friendly**: 
    - Easy debugging with detailed logging.
    - Simple configuration via a JSON file.

## Configuration

### Graphical Interface

FerroVela includes a simple graphical interface for managing the configuration.

To launch the configuration editor:
```bash
./target/release/ferrovela
```

### Authentication Types

FerroVela supports multiple authentication methods for upstream proxies:

#### NTLM

To use NTLM authentication:
1. Set `"auth_type": "ntlm"` in `config.json`.
2. Provide `username`, `password`, `domain`, and `workstation`.

```json
{
  "upstream": {
    "auth_type": "ntlm",
    "username": "user",
    "password": "password",
    "domain": "CORP",
    "workstation": "MYPC",
    "proxy_url": "http://proxy.corp.com:8080"
  }
}
```

#### Kerberos

To use Kerberos authentication:
1. Ensure your machine is joined to the domain or you have a valid Kerberos ticket (obtainable via `kinit`).
2. Set `"auth_type": "kerberos"` in `config.json`.
3. FerroVela will automatically use the cached credentials (TGT) to authenticate with the upstream proxy using SPNEGO.

```json
{
  "upstream": {
    "auth_type": "kerberos",
    "proxy_url": "http://proxy.corp.com:8080"
  }
}
```

#### Basic

To use Basic authentication:

```json
{
  "upstream": {
    "auth_type": "basic",
    "username": "user",
    "password": "password",
    "proxy_url": "http://proxy.corp.com:8080"
  }
}
```

### Manual Configuration

Configuration can also be manually managed through a `config.json` file.

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
    "workstation": "WORKSTATION",
    "proxy_url": "http://upstream:8080"
  },
  "exceptions": {
    "hosts": ["localhost", "127.0.0.1", "*.local"]
  }
}
```

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
- `musli`: Configuration parsing and serialization (JSON format).
- `rquickjs`: JavaScript engine for PAC file evaluation.
- `reqwest`: HTTP client for remote PAC file fetching (DIRECT, no-proxy).
- `iced`: For the graphical user interface.
- `libgssapi`: For Kerberos/GSSAPI integration.
- `ntlmclient`: For NTLMv2 authentication.

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for a detailed list of changes.

