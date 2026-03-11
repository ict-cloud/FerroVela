# Keyring Integration Specification

## Overview
Currently, FerroVela stores upstream proxy passwords as plaintext in the `config.toml` file. This specification details the integration of the operating system's native keychain (e.g., macOS Keychain, Windows Credential Manager, Secret Service on Linux) to securely store and retrieve these passwords.

## Design Changes

### 1. Configuration Changes (`src/config.rs`)
* Add a `use_keyring` boolean flag to the `UpstreamConfig` struct to indicate whether the password should be fetched from the system keychain instead of the TOML file.

### 2. Authentication Logic (`src/auth/mod.rs`)
* When initializing authenticators (e.g., Basic, NTLM), the system will check the `use_keyring` flag.
* If `use_keyring` is true and a `username` is provided, the application will use the `keyring` crate to query the OS keychain for the password under the service name `ferrovela` and the provided `username`.
* If the password lookup fails, it can gracefully fallback to the configuration file's password or fail the authentication setup.

### 3. User Interface (`src/ui.rs`)
* Add a "Store password in system keyring" toggle in the Upstream Settings view.
* When this toggle is active, any entered password will be securely saved to the OS keychain instead of being written to `config.toml`. The `password` field in `config.toml` will be cleared or omitted to prevent plaintext storage.
* If the toggle is deactivated, the system will revert to storing the password in the TOML file, and ideally, the keychain entry will be removed to maintain hygiene.

## Dependencies
* Add the `keyring` crate to `Cargo.toml`.

## Security Considerations
* By storing passwords in the OS keychain, users gain hardware-backed encryption (where applicable) and unified credential management.
* The service name for the keyring will be set to `ferrovela`.
