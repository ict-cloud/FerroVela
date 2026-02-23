# Feature Comparison: FerroVela vs. Enterprise Proxy Standard (PreProxy Benchmark)

This document compares FerroVela's current implementation against a hypothetical "PreProxy" standard, representing a typical enterprise-grade local proxy tool.

| Feature Area | Enterprise Standard (PreProxy) | FerroVela Current State | Gap / Action |
| :--- | :--- | :--- | :--- |
| **Authentication** | | | |
| NTLM Support | **Full Support** (Crucial for Windows environments) | **Missing** (UI option exists, backend ignores) | **Critical:** Implement NTLM authentication strategy. |
| Kerberos Support | **Full Support** (via GSSAPI/SSPI) | **Basic Support** (via `libgssapi`, optimistic) | **Enhance:** Verify cross-platform compatibility (Windows vs Linux). |
| Basic Auth | **Full Support** | **Full Support** | None. |
| **PAC Engine** | | | |
| Functionality | **Full Spec Compliance** (Netscape PAC standard) | **Partial** (`dnsResolve` mocked, `isInNet` missing) | **Critical:** Implement `dnsResolve` (real DNS lookup) and `isInNet`. |
| Performance | **High** (V8 or optimized engine) | **Moderate** (Boa engine, new thread per worker) | **Monitor:** Ensure Boa performance is sufficient for complex PACs. |
| **Networking** | | | |
| Gateway Mode | **Supported** (Bind to 0.0.0.0 or specific IP) | **Missing** (Hardcoded to 127.0.0.1) | **Feature:** Allow binding to non-loopback interfaces. |
| Connection Pooling | **Robust** (Keep-Alive, reuse) | **Basic** (Relies on Hyper's internal pooling) | **Verify:** Ensure connection reuse across multiple client requests. |
| **Configuration** | | | |
| Persistence | **Atomic Save** (Write to temp, rename) | **Direct Overwrite** (On every keystroke) | **Fix:** Debounce saves or add explicit "Apply" button logic. |
| Hot Reload | **Supported** (File watcher) | **Manual** (Restart service via UI) | **Feature:** Watch config file for changes. |
| **Observability** | | | |
| Logging | **Structured & Rotated** (JSON, file limits) | **Simple Text** (Appends indefinitely) | **Fix:** Add log rotation to prevent disk fill-up. |

## Conclusion

FerroVela is a functional basic proxy but lacks critical enterprise features, primarily **NTLM authentication** and a **fully compliant PAC engine**. Addressing these two areas is the highest priority to match standard capabilities.
