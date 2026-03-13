# Gap Analysis: Preproxy vs. FerroVela

This document outlines the critical differences between the target application "Preproxy" and the current state of "FerroVela", along with a prioritized roadmap to bridge the gap.

## Critical Functional Differences

| Feature | Preproxy (Target) | FerroVela (Current) | Gap Severity |
| :--- | :--- | :--- | :--- |
| **NTLM Authentication** | Fully supported for upstream proxies. | **Missing**. UI option exists but backend logic is unimplemented. | **Critical** |
| **Keychain Integration** | Reads credentials securely from macOS Keychain. | Credentials stored in plain text `config.json` (Basic) or relies on system TGT (Kerberos). | **High** |
| **IPv6 Support** | Listens on IPv6 loopback. | Listens only on IPv4 (`127.0.0.1`). | Medium |
| **Exception Logic** | Supports domains, IPs, and glob patterns. | Supports only exact match and simple `*.domain` wildcards. | Medium |
| **PAC Fallback** | Falls back to DIRECT connection if PAC fails. | Falls back to configured upstream proxy (if any). | Low |
| **Configuration** | UI + CLI args + Env Vars. | UI + CLI args + Config File. | Low |

## Prioritization & Approach

### Phase 1: Critical Connectivity (Immediate)
The most critical missing feature is **NTLM Authentication**. Without this, the proxy cannot function in many corporate environments where NTLM is the standard or fallback for Kerberos.

**Plan:**
1.  Research Rust NTLM libraries (e.g., `ntlm`, `reqwest` features).
2.  Implement `UpstreamAuthenticator` for NTLM.
3.  Integrate NTLM handshake logic into `src/proxy/connect.rs` and `src/proxy/nonconnect.rs`.
4.  Verify against a mock or real NTLM proxy.

### Phase 2: Security & Usability (High Priority)
**Keychain Integration** is essential for security and user experience on macOS. Users should not store passwords in plain text.

**Plan:**
1.  Add `security-framework` dependency.
2.  Implement logic to read/write credentials to Keychain.
3.  Update `config.json` to support a "Keychain" source or simply omit password fields.

### Phase 3: Network Robustness (Medium Priority)
**IPv6 Support** and **Advanced Exceptions**.

**Plan:**
1.  Update `src/main.rs` and `src/proxy/mod.rs` to bind to `[::1]` or dual-stack.
2.  Enhance `resolve_proxy` in `src/proxy/mod.rs` to support CIDR (e.g., `192.168.0.0/24`) and better globs.

### Phase 4: Polish
**PAC Fallback** and **UI Improvements**.

## Recommendation
Start immediately with **Phase 1: NTLM Authentication**. This is the single biggest blocker for functional parity.
