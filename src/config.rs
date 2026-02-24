use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Default, Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub upstream: Option<UpstreamConfig>,
    pub exceptions: Option<ExceptionsConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProxyConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    pub pac_file: Option<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            pac_file: None,
        }
    }
}

pub fn default_port() -> u16 {
    3128
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ExceptionsConfig {
    pub hosts: Vec<String>,
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
