use anyhow::Result;
use musli::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Default, Debug, Decode, Encode, Clone, Deserialize, Serialize)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub upstream: Option<UpstreamConfig>,
    pub exceptions: Option<ExceptionsConfig>,
}

#[derive(Debug, Decode, Encode, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Decode, Encode, Clone, Deserialize, Serialize)]
pub struct UpstreamConfig {
    pub auth_type: String, // "ntlm", "kerberos", "basic", "none"
    pub username: Option<String>,
    pub password: Option<String>,
    #[musli(default)]
    #[serde(default)]
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

#[derive(Debug, Decode, Encode, Clone, Default, Deserialize, Serialize)]
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
    let content = fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

pub fn save_config(path: &str, config: &Config) -> Result<()> {
    let content = toml::to_string(config)?;
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

    file.write_all(content.as_bytes())?;
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

        // Verify the content is valid TOML and contains expected default values
        assert!(content.contains("port = 3128"));

        // Ensure it can be deserialized back into a Config object
        let loaded_config: Config =
            toml::from_str(&content).expect("Failed to deserialize saved config");
        assert_eq!(loaded_config.proxy.port, 3128);
    }
}
