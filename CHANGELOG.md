# Changelog

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
