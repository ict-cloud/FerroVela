# Changelog

## [0.4.3] - Unreleased

### Added
- **Dark mode support.** The UI now follows the macOS system appearance. The theme switches between Light and Dark automatically when the system preference changes (polled every three seconds).
- **Inline input validation.** The Port, PAC File, and Proxy URL fields are now validated as the user types. Invalid values show a red error message directly below the field and block saving until corrected. Port must be a number between 1 and 65535; PAC File must be an `http(s)://` URL or an existing file path; Proxy URL must start with `http://` or `https://` and include a host.
- **Service status indicator.** The service toggle row now shows a coloured dot — green when the proxy is running, grey when stopped — making the current state immediately scannable.

- **Resizable main and log windows.** Both windows now enforce a minimum size (600×450 for the main window, 500×350 for the log window) and grow freely beyond the default 800×600.
- **Log viewer search.** A search bar at the top of the log window filters lines in real time (case-insensitive). Empty the field to show all entries.
- **Log level coloring.** Log lines are coloured by severity — ERROR in red, WARN in amber (adjusted for dark mode), DEBUG/TRACE in grey, INFO in the default theme text colour.
- **Log viewer auto-scroll.** The log window automatically scrolls to the latest entry on each refresh tick (every 500 ms while the window is open).
- **Logs button moved to sidebar.** The "Show Logs" button has been removed from the service control bar and added to the bottom of the sidebar. The button highlights when the log window is open.

### Changed
- **Upstream tab: progressive disclosure.** The Upstream settings panel now shows only the fields relevant to the selected authentication type. Selecting *None* hides all credential fields; *Basic* shows username, password, keyring toggle, and proxy URL; *NTLM* additionally reveals domain and workstation; *Kerberos* shows only a principal field and proxy URL (no password — the system Kerberos ticket cache is used).
- **Status feedback.** Save confirmations ("Saved successfully!") and service-toggle outcomes are now displayed below the service control bar in green (success) or red (error). Messages auto-clear after approximately three seconds.
- **Restart-required banner.** When a configuration change is saved while the proxy service is running, a yellow warning banner appears with a *Restart Now* button. The banner dismisses automatically when the service is restarted or stopped.


## [0.4.2] - 2. Apr 2026

### Security
- Fixed YAML injection vulnerability in g3proxy config generation: upstream credentials (`username`, `password`) and the proxy address were embedded into a YAML string via `format!()` without escaping, allowing a specially crafted password to break out of the YAML scalar and inject arbitrary configuration keys. All three values are now passed through a `yaml_escape()` function that escapes `"`, `\`, `\n`, `\r`, `\t`, and null before interpolation.
- Keyring is now the default credential storage. `upstream_use_keyring` defaults to `true` in `UpstreamConfig` and the CFPreferences fallback (for installs where the key has never been written). Existing users who have `upstream_use_keyring = false` stored in preferences are unaffected — the explicit saved value is still respected.
- Hardened PAC file fetching and parsing. HTTP fetches now carry a 30-second timeout (previously unbounded). Responses larger than 1 MiB — locally or remotely — are rejected before workers are spawned. Scripts are validated in a throw-away QuickJS context at load time: syntax errors and a missing `FindProxyForURL` now fail `PacEngine::new` immediately instead of surfacing as per-request errors. Each worker thread installs a QuickJS interrupt handler that aborts any `FindProxyForURL` call that runs longer than 5 seconds, preventing a malicious PAC script with an infinite loop from hanging a worker thread indefinitely. The caller's choice of protocol (http vs https) is preserved — no automatic scheme upgrade is applied.
- Credentials are no longer written to log files. Removed username from keyring operation log messages; replaced the raw proxy URL (which may contain `user:pass@`) in the parse-failure warning with a static string; replaced a fragile `Proxy-Authorization` header slice (which could expose token bytes) with an explicit scheme-name-only log. `UpstreamConfig` now has a hand-written `Debug` impl that emits `[REDACTED]` for the `password` and `proxy_url` fields, preventing accidental exposure if the struct is ever formatted with `{:?}`.
- Hardened the UI IPC Unix socket against unauthorized access. The socket path moved from `/tmp/ferrovela-ui.sock` (world-writable directory) to `$TMPDIR/{SERVICE_LABEL}.sock`, which on macOS is a per-user, per-session directory managed by launchd. The socket file is now created with mode `0600` so the kernel rejects connection attempts from other users before any data is exchanged. Incoming connections are additionally verified with `getpeereid(2)` to confirm the peer shares the current process's effective UID; connections from unexpected peers are dropped without triggering the show-window signal.
- Replaced the predictable g3proxy config temp file path (`/tmp/ferrovela_g3proxy.yaml`) with a `NamedTempFile` created via `tempfile::Builder`. The file now has a random suffix, is created with mode 0600 (owner read/write only), and is deleted immediately after g3proxy has parsed the config into memory, eliminating the predictable-path race and symlink-attack vectors.
- Enforced the `proxy_allow_private_ips` config flag (previously defined but never checked). CONNECT requests to private/loopback IP ranges (RFC 1918, 127.0.0.0/8, 169.254.0.0/16, ::1, fc00::/7, fe80::/10) are now rejected with 403 Forbidden at both enforcement points: the auth tunnel direct-connect path and the g3proxy splice path. Set `proxy_allow_private_ips = true` to restore direct access to internal networks.
- Added log rotation to prevent the log file from growing without bound. The logger now tracks bytes written and rotates at 10 MiB: the current file moves to `ferrovela.log.1`, shifting any existing backups down, keeping up to three (`.1`–`.3`). The byte counter is seeded from the existing file size on startup so a process restart does not reset the threshold.
- Hardened local PAC file path handling against path traversal. `PacEngine::new` now validates the path in two steps before reading any content: (1) a pre-canonicalization check rejects any path that contains a `..` component outright; (2) `std::fs::canonicalize` resolves the path to its absolute, symlink-free form and confirms it exists. The resolved path is additionally verified to be a regular file (not a directory, device node, or named pipe) before the content is read.

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
