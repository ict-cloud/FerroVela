use serde::Deserialize;
use std::fs;
use std::path::Path;
use anyhow::Result;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub upstream: Option<UpstreamConfig>,
    pub exceptions: Option<ExceptionsConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    pub port: u16,
    pub pac_file: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpstreamConfig {
    pub auth_type: String, // "ntlm", "kerberos", "basic", "none"
    pub username: Option<String>,
    pub password: Option<String>,
    pub proxy_url: Option<String>, // if no PAC, use this
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExceptionsConfig {
    pub hosts: Vec<String>,
}

pub fn load_config(path: &str) -> Result<Config> {
    let content = fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}
