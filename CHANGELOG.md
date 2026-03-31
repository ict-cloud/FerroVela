# Changelog

## [0.4.2] - Unreleased

### Security
- Fixed YAML injection vulnerability in g3proxy config generation: upstream credentials (`username`, `password`) and the proxy address were embedded into a YAML string via `format!()` without escaping, allowing a specially crafted password to break out of the YAML scalar and inject arbitrary configuration keys. All three values are now passed through a `yaml_escape()` function that escapes `"`, `\`, `\n`, `\r`, `\t`, and null before interpolation.

## [0.4.1] - 31. Mar 2026

### Added
- Split the project into two separate binaries: `ferrovela` (headless proxy service) and `ferrovela-ui` (configuration GUI). The `iced` dependency is now optional behind a `ui` feature flag so the proxy binary can be built without it.
- macOS launchd integration: the UI starts and stops the proxy as a user-level launchd service (`com.ictcloud.ferrovela`). The plist is installed to `~/Library/LaunchAgents/` automatically.
- The UI can now be fully closed while the proxy service keeps running. On reopen, the toggle reflects the actual service state via periodic `launchctl` polling.
- The proxy binary is now included in the macOS app bundle (`Contents/MacOS/ferrovela`). The launchd plist is generated dynamically at startup with the resolved bundle path, so the app is fully self-contained.
- Added `bundle.sh` build script that builds both binaries, runs `cargo bundle`, and copies the proxy binary into the app bundle.
- **MDM support**: Configuration is now stored via macOS CFPreferences (`com.ictcloud.ferrovela` domain) instead of a JSON file. MDM administrators can push configuration profiles (`.mobileconfig`) to manage proxy settings across a fleet. Managed (forced) preferences automatically override user-set values.

### Changed
- Single-instance detection switched from a TCP magic request on the proxy port to a Unix domain socket (`/tmp/ferrovela-ui.sock`), decoupling UI lifecycle from proxy state.
- Proxy service logs are now written to `~/Library/Logs/ferrovela.log`.
- `proxy_exe()` now returns an error instead of silently falling back when the proxy binary is not found next to the running executable.
- Configuration storage migrated from JSON file (`config.json`) to macOS CFPreferences. Settings can be read and written via `defaults read/write com.ictcloud.ferrovela`.
- The `--config` CLI flag has been removed from both binaries; configuration is read from CFPreferences automatically.

### Removed
- `config.json` file and JSON-based configuration. The bundled `Contents/Resources/config.json` resource is no longer included.
- `musli` and `clap` dependencies (replaced by `core-foundation` / `core-foundation-sys`).
 

## [0.3.4] - Unreleased

### Changed
- Single-instance detection switched from a TCP magic request on the proxy port to a Unix domain socket (`/tmp/ferrovela-ui.sock`), decoupling UI lifecycle from proxy state.
- Proxy service logs are now written to `~/Library/Logs/ferrovela.log`.

## [0.3.3]

### Changed
- Updated dependencies and removed unused ones.
- A second window now always opens on launch, positioned with a small offset from the main window.
- Terminal window is no longer kept open after use.

### Fixed (CI)
- `bump-version.yaml` now passes the GitHub token explicitly to avoid authentication failures.

## [0.3.2] - 2026-03-18

### Added
- Replaced **Boa** JavaScript engine with **rquickjs** for PAC file evaluation — significantly reduces binary size and improves JS execution performance.
- New CI workflows for automatic version bumping and tagging on master (`bump-version.yaml`, `tag.yaml`).
- Enabled `vendored-openssl` feature on `g3proxy`: OpenSSL is now compiled from source and statically linked, removing the runtime dependency on the system OpenSSL installation.

### Fixed (CI)
- `bump-version.yaml` now creates a pull request and auto-merges it instead of pushing directly to `master`, which was blocked by branch protection rules.
- `bump-version.yaml` now passes the GitHub token explicitly to avoid authentication failures.

### Changed
- Moved Basic auth unit tests from `src/auth/basic.rs` to `src/tests/auth_tests.rs`.

### Fixed
- Unwanted error propagation in `kerberos.rs`, `logger.rs`, and `main.rs` that could cause the application to crash.
- Removed unused imports in test module.

### Performance
- Reduced unnecessary string clones in `src/pac.rs`.
- Optimised `auth_tunnel.rs`, `http_utils.rs`, and `proxy/mod.rs` to reduce allocations.

## [0.3.1] - 2026-03-13

### Added
- Fully functional proxy engine built on **g3proxy** (ByteDance): c-ares resolver, DirectFixed/ProxyHttp escaper, HttpProxy server on an OS-assigned internal port.
- **Kerberos and NTLM auth tunnel** (`src/proxy/auth_tunnel.rs`): drives the full challenge-response handshake with the upstream proxy directly, with PAC/exceptions routing per connection.
- Pre-processor listener owns the user-facing port, preserving the single-instance IPC (`/__ferrovela/show`) independently of g3proxy.
- Add keyring support for storing credentials securely.

### Changed
- Config serialisation switched from `serde`/TOML to **musli JSON** (`config.json`), removing the last obstacle to a fully static musl build.
- `src/ui.rs` split into `ui/mod.rs`, `ui/model.rs`, `ui/view.rs`, `ui/controller.rs`.

### Removed
- `serde`, `serde_json`, `toml` dependencies.

## [0.3.0] -

### Added
- more tests

### Changed
- g3proxy replaced pingora
- minor improvements.

## [0.2.0] - 2026-03-07

### Added
- Specification document for Pingora evaluation

### Changed
- Replaced `hyper` with `pingora` framework for proxying
- Replaced `serde` with `musli` for serialization where possible

### Security
- Fixed potential OOM vulnerability by size-limiting response body drain

## [0.1.3] - 2026-03-05

### Changed
- Enhanced connection error messages with target details for easier debugging
- Updated project dependencies

### Performance
- Implemented `TCP_NODELAY` on all proxy `TcpStream`s to optimize latency and significantly improve throughput
- Optimized the request builder in the proxy authentication loop to avoid redundant allocations

## [0.1.2] - 2026-03-03

### Changed
- Removed deprecated `--ui` CLI argument
- Refactored JS context setup by extracting it in `PacEngine::new`
- Removed unused `_config` parameter in `handle_upstream`
- Extracted magic string `/__ferrovela/show` to a constant

### Fixed
- Used `default_port()` fallback instead of hardcoded `3128` port

### Performance
- Optimized `parse_content_length` to avoid allocations
- Optimized string allocations in proxy `CONNECT` handshake

### Tests
- Added unit tests for Basic Authenticator

## [0.1.1] - 2026-03-02

### Added
- Unit tests for `resolve_proxy` function to verify exception matching and fallback behavior when the PAC engine is not present

### Fixed
- PAC file handling to always be direct and add missing PAC functionality

### Changed
- UI refactoring for macOS compliance including Sidebar and Multi-window Logs improvements
- Refactored `ConfigEditor::update` to delegate to helper methods (`handle_config_message`, `handle_toggle_service`, `handle_window_message`) for better code readability and maintainability
- Replaced custom glob matching implementation in `pac.rs` with the `glob` crate


## 2026-02-26

### PAC Engine – Robustness & Completeness

#### Bug Fixes

- **PAC file fetched via DIRECT connection**: The PAC file HTTP client now uses `reqwest::Client::builder().no_proxy()` to bypass all system proxy settings. This prevents a circular dependency where the proxy configuration source (PAC file) would itself require proxy configuration to be fetched.

- **Stack overflow in PAC worker threads**: Replaced `thread::spawn` with `thread::Builder` using an explicit 8 MB stack size (`stack_size(8 * 1024 * 1024)`) and named threads (`pac-worker`). The Boa JavaScript engine uses deep recursion for parsing and evaluation, and complex real-world PAC scripts caused the default thread stack to overflow.

#### New Features

- **Full PAC helper function support**: Implemented all standard PAC helper functions as defined by the [Netscape PAC specification](https://developer.mozilla.org/en-US/docs/Web/HTTP/Proxy_servers_and_tunneling/Proxy_Auto-Configuration_PAC_file). Previously only `dnsResolve`, `myIpAddress`, and `shExpMatch` were available. The following functions have been added:

  | Function | Description |
  |---|---|
  | `isPlainHostName(host)` | Returns `true` if hostname contains no dots |
  | `dnsDomainIs(host, domain)` | Returns `true` if host's domain matches |
  | `localHostOrDomainIs(host, hostdom)` | Returns `true` if host matches or is unqualified part of hostdom |
  | `isResolvable(host)` | Returns `true` if DNS can resolve the host |
  | `isInNet(host, pattern, mask)` | Returns `true` if resolved host IP matches network/mask |
  | `dnsDomainLevels(host)` | Returns the number of dots in the hostname |
  | `convert_addr(ipaddr)` | Converts a dotted IP address string to an integer |
  | `weekdayRange(...)` | Stub – always returns `true` |
  | `dateRange(...)` | Stub – always returns `true` |
  | `timeRange(...)` | Stub – always returns `true` |

#### Tests

- Added integration tests for all new PAC helper functions exercised through `FindProxyForURL` via the `PacEngine`.
