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

### Graphical Interface

FerroVela includes a simple graphical interface for managing the configuration.

To launch the configuration editor:
```bash
./target/release/ferrovela --ui
```

### Manual Configuration

Configuration can also be manually managed through a `config.toml` file.

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
