use anyhow::Result;
use musli::{Decode, Encode};
use std::fs::{self, OpenOptions};
use std::io::Write;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Default, Debug, Decode, Encode, Clone)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub upstream: Option<UpstreamConfig>,
    pub exceptions: Option<ExceptionsConfig>,
}

#[derive(Debug, Decode, Encode, Clone)]
pub struct ProxyConfig {
    #[musli(default = default_port)]
    pub port: u16,
    pub pac_file: Option<String>,
    #[musli(default)]
    pub allow_private_ips: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            pac_file: None,
            allow_private_ips: false,
        }
    }
}

pub fn default_port() -> u16 {
    3128
}

#[derive(Debug, Decode, Encode, Clone)]
pub struct UpstreamConfig {
    pub auth_type: String, // "ntlm", "kerberos", "basic", "none"
    pub username: Option<String>,
    pub password: Option<String>,
    #[musli(default)]
    pub use_keyring: bool,
    pub domain: Option<String>,
    pub workstation: Option<String>,
    pub proxy_url: Option<String>, // if no PAC, use this
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            auth_type: "none".to_string(),
            username: None,
            password: None,
            use_keyring: false,
            domain: None,
            workstation: None,
            proxy_url: None,
        }
    }
}

#[derive(Debug, Decode, Encode, Clone, Default)]
pub struct ExceptionsConfig {
    pub hosts: Vec<String>,
}

impl ExceptionsConfig {
    pub fn matches(&self, host: &str) -> bool {
        self.hosts
            .iter()
            .any(|pattern| Self::host_matches_pattern(pattern, host))
    }

    fn host_matches_pattern(pattern: &str, host: &str) -> bool {
        if pattern == host {
            return true;
        }
        // pattern[1..] strips the '*', leaving ".example.com", so only actual subdomains match
        if pattern.starts_with("*.") && host.ends_with(&pattern[1..]) {
            return true;
        }
        false
    }
}

pub fn load_config(path: &str) -> Result<Config> {
    let content = fs::read(path)?;
    let config: Config = musli::json::from_slice(&content)?;
    Ok(config)
}

pub fn save_config(path: &str, config: &Config) -> Result<()> {
    let content = musli::json::to_vec(config)?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options.open(path)?;

    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(Permissions::from_mode(0o600))?;
    }

    file.write_all(&content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exceptions_exact_match() {
        let exceptions = ExceptionsConfig {
            hosts: vec!["example.com".to_string()],
        };
        assert!(exceptions.matches("example.com"));
        assert!(!exceptions.matches("sub.example.com"));
        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_wildcard_match() {
        let exceptions = ExceptionsConfig {
            hosts: vec!["*.example.com".to_string()],
        };
        assert!(exceptions.matches("sub.example.com"));
        assert!(exceptions.matches("deep.sub.example.com"));

        // Bare domain and suffix-only hosts must NOT match the wildcard
        assert!(!exceptions.matches("example.com"));
        assert!(!exceptions.matches("myexample.com"));

        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_multiple_patterns() {
        let exceptions = ExceptionsConfig {
            hosts: vec!["exact.com".to_string(), "*.wild.com".to_string()],
        };
        assert!(exceptions.matches("exact.com"));
        assert!(!exceptions.matches("sub.exact.com"));

        assert!(exceptions.matches("sub.wild.com"));
        assert!(!exceptions.matches("wild.com")); // bare domain does not match wildcard

        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_empty() {
        let exceptions = ExceptionsConfig { hosts: vec![] };
        assert!(!exceptions.matches("example.com"));
    }

    #[test]
    fn test_save_config_serialization() {
        use tempfile::NamedTempFile;

        // Create a temporary file
        let temp_file = NamedTempFile::new().expect("Failed to create temp file");
        let path = temp_file
            .path()
            .to_str()
            .expect("Failed to get temp file path");

        // Create a default config
        let config = Config::default();

        // Save the config
        save_config(path, &config).expect("Failed to save config");

        // Read the content back
        let content = fs::read_to_string(path).expect("Failed to read back config file");

        // Verify the content is valid JSON and contains expected default values
        assert!(content.contains("3128"));

        // Ensure it can be deserialized back into a Config object
        let loaded_config = load_config(path).expect("Failed to deserialize saved config");
        assert_eq!(loaded_config.proxy.port, 3128);
    }

    // ── ProxyConfig round-trip ────────────────────────────────────────────────

    #[test]
    fn test_proxy_config_round_trip() {
        use tempfile::NamedTempFile;

        let config = Config {
            proxy: ProxyConfig {
                port: 8080,
                pac_file: Some("http://wpad.corp/proxy.pac".to_string()),
                allow_private_ips: true,
            },
            upstream: None,
            exceptions: None,
        };

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();

        save_config(path, &config).unwrap();
        let loaded = load_config(path).unwrap();

        assert_eq!(loaded.proxy.port, 8080);
        assert_eq!(
            loaded.proxy.pac_file.as_deref(),
            Some("http://wpad.corp/proxy.pac")
        );
        assert!(loaded.proxy.allow_private_ips);
        assert!(loaded.upstream.is_none());
        assert!(loaded.exceptions.is_none());
    }

    // ── UpstreamConfig round-trip ─────────────────────────────────────────────

    #[test]
    fn test_upstream_config_basic_round_trip() {
        use tempfile::NamedTempFile;

        let config = Config {
            proxy: ProxyConfig::default(),
            upstream: Some(UpstreamConfig {
                auth_type: "basic".to_string(),
                username: Some("alice".to_string()),
                password: Some("s3cret".to_string()),
                use_keyring: false,
                domain: None,
                workstation: None,
                proxy_url: Some("http://proxy.corp:8080".to_string()),
            }),
            exceptions: None,
        };

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();

        save_config(path, &config).unwrap();
        let loaded = load_config(path).unwrap();

        let upstream = loaded.upstream.unwrap();
        assert_eq!(upstream.auth_type, "basic");
        assert_eq!(upstream.username.as_deref(), Some("alice"));
        assert_eq!(upstream.password.as_deref(), Some("s3cret"));
        assert!(!upstream.use_keyring);
        assert_eq!(upstream.proxy_url.as_deref(), Some("http://proxy.corp:8080"));
    }

    #[test]
    fn test_upstream_config_ntlm_round_trip() {
        use tempfile::NamedTempFile;

        let config = Config {
            proxy: ProxyConfig::default(),
            upstream: Some(UpstreamConfig {
                auth_type: "ntlm".to_string(),
                username: Some("bob".to_string()),
                password: Some("hunter2".to_string()),
                use_keyring: false,
                domain: Some("CORP".to_string()),
                workstation: Some("LAPTOP01".to_string()),
                proxy_url: Some("http://ntlm-proxy:3128".to_string()),
            }),
            exceptions: None,
        };

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();

        save_config(path, &config).unwrap();
        let loaded = load_config(path).unwrap();

        let upstream = loaded.upstream.unwrap();
        assert_eq!(upstream.auth_type, "ntlm");
        assert_eq!(upstream.domain.as_deref(), Some("CORP"));
        assert_eq!(upstream.workstation.as_deref(), Some("LAPTOP01"));
    }

    #[test]
    fn test_upstream_config_keyring_flag_round_trip() {
        use tempfile::NamedTempFile;

        let config = Config {
            proxy: ProxyConfig::default(),
            upstream: Some(UpstreamConfig {
                auth_type: "basic".to_string(),
                username: Some("carol".to_string()),
                password: None,
                use_keyring: true,
                domain: None,
                workstation: None,
                proxy_url: None,
            }),
            exceptions: None,
        };

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();

        save_config(path, &config).unwrap();
        let loaded = load_config(path).unwrap();

        let upstream = loaded.upstream.unwrap();
        assert!(upstream.use_keyring);
        assert!(upstream.password.is_none());
    }

    // ── ExceptionsConfig round-trip ───────────────────────────────────────────

    #[test]
    fn test_exceptions_config_round_trip() {
        use tempfile::NamedTempFile;

        let config = Config {
            proxy: ProxyConfig::default(),
            upstream: None,
            exceptions: Some(ExceptionsConfig {
                hosts: vec![
                    "localhost".to_string(),
                    "*.internal.corp".to_string(),
                    "10.0.0.1".to_string(),
                ],
            }),
        };

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();

        save_config(path, &config).unwrap();
        let loaded = load_config(path).unwrap();

        let exceptions = loaded.exceptions.unwrap();
        assert_eq!(exceptions.hosts.len(), 3);
        assert!(exceptions.matches("localhost"));
        assert!(exceptions.matches("host.internal.corp"));
        assert!(exceptions.matches("10.0.0.1"));
        assert!(!exceptions.matches("external.com"));
    }

    // ── load_config error handling ────────────────────────────────────────────

    #[test]
    fn test_load_config_missing_file() {
        let result = load_config("/nonexistent/path/config.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_invalid_json() {
        use tempfile::NamedTempFile;

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"this is not valid json {{{").unwrap();

        let result = load_config(tmp.path().to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_empty_file() {
        use tempfile::NamedTempFile;

        let tmp = NamedTempFile::new().unwrap();
        fs::write(tmp.path(), b"").unwrap();

        let result = load_config(tmp.path().to_str().unwrap());
        assert!(result.is_err());
    }
}
