# Preproxy Specifications (Derived from Analysis)

## Overview
Preproxy is a MacOS application designed to act as a local proxy server that forwards requests to an upstream corporate proxy, handling complex authentication mechanisms like NTLM and Kerberos automatically.

## Core Features

1.  **Local Proxy Server**:
    -   Listens on `localhost` (IPv4 and IPv6).
    -   Configurable port (default typically 3128 or random).
    -   Supports HTTP and HTTPS (CONNECT method) tunneling.

2.  **Upstream Authentication**:
    -   **NTLM Authentication**: Supports NTLM authentication with the upstream proxy.
    -   **Kerberos Authentication**: Supports Kerberos authentication using system credentials (TGT).
    -   **Basic Authentication**: Supports standard username/password authentication.

3.  **Proxy Auto-Configuration (PAC)**:
    -   Supports parsing and executing PAC files (`.pac` or `wpad.dat`).
    -   Dynamically determines the upstream proxy based on the request URL.
    -   **Fallback**: Attempts direct connection if the PAC file is unresolvable or fails.

4.  **Credential Management**:
    -   Integrates with **macOS Keychain** to securely store and retrieve proxy credentials.
    -   Does not require storing passwords in plain text configuration files.

5.  **Traffic Control / Exceptions**:
    -   **Bypass List**: Configurable list of domains, IP addresses, or glob patterns to bypass the upstream proxy (direct connection).
    -   **Smart Logic**: Automatically bypasses localhost/loopback addresses.

6.  **Logging & Diagnostics**:
    -   Logs failed requests for troubleshooting.
    -   Displays recent request history in the UI.

7.  **Configuration**:
    -   GUI for easy configuration.
    -   Import/Export configuration settings.
    -   Uniform configuration for command-line tools (via environment variables pointing to localhost).

8.  **Compatibility**:
    -   Handles large Kerberos tokens (often required in complex AD environments).
    -   Works with both GUI and CLI applications by setting system proxy settings or environment variables.
