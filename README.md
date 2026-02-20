# FerroVela

A Rust-based local proxy designed for developers, offering robust configuration and upstream authentication support.

## Features

- **PAC File Support**: Seamlessly reads and interprets `.pac` files for dynamic proxy configuration.
- **Protocol Support**: Routes both HTTP and HTTPS traffic efficiently.
- **Smart Exceptions**: configure exceptions based on domains, glob patterns, or IP addresses.
- **Corporate Authentication**: Supports NTLM and Kerberos authentication for upstream proxies.
- **Developer Friendly**: 
    - Easy debugging with detailed logging.
    - Simple configuration via a TOML file.

## Configuration

Configuration is managed through a `config.toml` file.

```toml
[proxy]
port = 8080
pac_file = "http://wpad/wpad.dat" # or local path

[upstream]
auth_type = "ntlm" # or "kerberos", "basic", "none"
username = "user"
password = "password" # Optional, can prompt or use system creds if supported

[exceptions]
# bypass upstream proxy for these
hosts = ["localhost", "127.0.0.1", "*.local"]
```

## Building and Running

1. **Install Rust**: Ensure you have Rust and Cargo installed.
2. **Build**: `cargo build --release`
3. **Run**: `./target/release/ferrovela`

## Dependencies

- `hyper`: For low-level HTTP handling.
- `tokio`: Asynchronous runtime.
- `serde`/`toml`: Configuration parsing.
