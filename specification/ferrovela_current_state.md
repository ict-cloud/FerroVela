# FerroVela Current State Analysis

## Overview
FerroVela is a local proxy server written in Rust, designed to authenticate upstream requests using various protocols. It provides a GUI for configuration and service management.

## Capabilities

### Authentication
- **Basic:** Fully supported.
- **Kerberos:** Supported via `libgssapi`. Uses an optimistic approach (sends `Negotiate <token>` immediately).
- **NTLM:** **Not Implemented.** The option exists in the UI configuration, but the backend logic (`src/auth/mod.rs`) currently ignores it, falling back to no authentication.

### Proxy Features
- **Protocols:** Supports HTTP and HTTPS (via `CONNECT` tunneling).
- **SOCKS:** Not supported.
- **PAC Support:**
  - Implemented using `boa_engine` (JavaScript execution).
  - Supports loading from local file or URL.
  - **Limitations:**
    - `dnsResolve`: Mocked to return the hostname, not the IP.
    - `myIpAddress`: Mocked to return `127.0.0.1`.
    - Missing standard PAC functions like `isInNet`, `dnsDomainIs`, `isPlainHostName`, etc.
- **Chaining:** Can chain to an upstream proxy defined in config or determined by PAC.

### Configuration & Persistence
- **Format:** TOML (`config.toml`).
- **Persistence:** Configuration is saved to disk immediately upon any change in the UI (e.g., every keystroke in text fields), which is a potential performance and data integrity issue.
- **Hot Reload:** Service must be restarted manually via UI to apply changes.

### Logging
- **Mechanism:** Custom `SimpleLogger` implementation.
- **Destination:** Writes to `service.log` in the working directory and `stderr`.
- **Rotation:** No log rotation implemented.
- **Levels:** Hardcoded to `INFO`.

### User Interface
- **Framework:** `iced`.
- **Features:**
  - Start/Stop service.
  - Configure port, PAC, upstream auth, exceptions.
  - View logs (reads `service.log`).

## Known Gaps (vs. Enterprise Standards)
1.  **Missing NTLM:** Critical for many corporate environments.
2.  **Incomplete PAC:** The PAC engine lacks essential network functions (`isInNet` being the most critical).
3.  **No Log Rotation:** `service.log` will grow indefinitely.
4.  **Inefficient Config Saving:** frequent disk writes during editing.
5.  **No Gateway Mode:** Binds to `127.0.0.1` (hardcoded in `src/proxy/mod.rs`), preventing use as a gateway for other devices.
