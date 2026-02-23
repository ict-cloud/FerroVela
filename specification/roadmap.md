# FerroVela Improvement Roadmap

Based on the comparison with enterprise standards (PreProxy benchmark), this roadmap outlines the recommended improvements for FerroVela.

## Phase 1: Critical Feature Parity (High Priority)

These features are essential for FerroVela to function in typical corporate environments where NTLM and complex PAC files are standard.

1.  **Implement NTLM Authentication:**
    - **Current Status:** Placeholder in UI, ignored in backend.
    - **Plan:**
        - Integrate `ntlm` crate (or similar pure-Rust implementation) into `src/auth/ntlm.rs`.
        - Implement `UpstreamAuthenticator` for NTLM, handling the challenge-response flow (requires connection state or re-auth on 407).
        - Ensure cross-platform compatibility (Windows SSPI vs Linux NTLM).

2.  **Enhance PAC Engine Compliance:**
    - **Current Status:** Missing `isInNet`, `dnsResolve` (mocked), `myIpAddress` (mocked).
    - **Plan:**
        - Implement real DNS resolution in `pac.rs` using `trust-dns-resolver` or `std::net::ToSocketAddrs` (sync/async bridge needed for Boa).
        - Implement `isInNet` using `ipnet` crate to check IP ranges.
        - Implement `myIpAddress` using `local-ip-address` crate.
        - Add support for other standard PAC functions (`dnsDomainIs`, `shExpMatch` improvements).

## Phase 2: Reliability & Performance (Medium Priority)

Improvements to make FerroVela more robust and efficient.

3.  **Optimize Configuration Handling:**
    - **Current Status:** Writes to disk on every keystroke.
    - **Plan:**
        - Implement debouncing in `ui.rs` (wait 500ms after last edit before saving).
        - Use atomic file writing (write to `.tmp` then rename) to prevent corruption.
        - Only restart service on explicit "Apply" or "Restart" action, not auto-restart on config change.

4.  **Implement Gateway Mode:**
    - **Current Status:** Hardcoded binding to `127.0.0.1`.
    - **Plan:**
        - Add `bind_address` to `config.toml` (default `127.0.0.1`).
        - Allow binding to `0.0.0.0` or specific interface IPs to serve other devices on the LAN.

5.  **Log Rotation:**
    - **Current Status:** Appends to `service.log` indefinitely.
    - **Plan:**
        - Integrate `log4rs` or implemented simple file rotation (e.g., rename on size limit).

## Phase 3: Advanced Features (Low Priority)

Nice-to-have features for power users.

6.  **SOCKS5 Support:**
    - Allow FerroVela to act as a SOCKS proxy, enabling non-HTTP traffic tunneling.

7.  **Traffic Inspection/MitM (Optional):**
    - TLS termination for inspecting/modifying traffic (requires CA cert generation).
