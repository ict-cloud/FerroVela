use crate::config::UpstreamConfig;
use anyhow::Result;

pub mod basic;
pub mod kerberos;
pub mod mock_kerberos;

/// Trait for upstream authentication strategies.
pub trait UpstreamAuthenticator: Send + Sync {
    /// Generates the value for the `Proxy-Authorization` header.
    /// Returns the full header value, e.g., "Basic <token>" or "Negotiate <token>".
    fn get_auth_header(&self) -> Result<String>;
}

pub fn create_authenticator(config: &UpstreamConfig) -> Option<Box<dyn UpstreamAuthenticator>> {
    match config.auth_type.as_str() {
        "basic" => {
            if let (Some(u), Some(p)) = (&config.username, &config.password) {
                Some(Box::new(basic::BasicAuthenticator::new(
                    u.clone(),
                    p.clone(),
                )))
            } else {
                None
            }
        }
        "kerberos" => {
            if let Some(proxy_url) = &config.proxy_url {
                let host = proxy_url
                    .trim_start_matches("http://")
                    .trim_start_matches("https://")
                    .split(':')
                    .next()
                    .unwrap_or("");
                Some(Box::new(kerberos::KerberosAuthenticator::new(host)))
            } else {
                None
            }
        }
        "mock_kerberos" => Some(Box::new(mock_kerberos::MockKerberosAuthenticator::new())),
        _ => None,
    }
}
