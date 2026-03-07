use anyhow::Result;
use musli::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::fs;

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
        if pattern.starts_with("*.") && host.ends_with(&pattern[2..]) {
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
    fs::write(path, content)?;
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
        // Matches because suffix matches
        assert!(exceptions.matches("sub.example.com"));
        assert!(exceptions.matches("deep.sub.example.com"));

        // Edge case behavior: matches if ends with "example.com"
        assert!(exceptions.matches("myexample.com"));
        assert!(exceptions.matches("example.com"));

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
        assert!(exceptions.matches("wild.com")); // matches suffix

        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_empty() {
        let exceptions = ExceptionsConfig { hosts: vec![] };
        assert!(!exceptions.matches("example.com"));
    }
}
