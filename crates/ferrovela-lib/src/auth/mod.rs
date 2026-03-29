use crate::config::UpstreamConfig;
use anyhow::Result;

pub mod basic;
pub mod kerberos;
pub mod mock_kerberos;
pub mod ntlm;

/// Trait for upstream authentication factory.
#[allow(dead_code)]
pub trait UpstreamAuthenticator: Send + Sync {
    /// Creates a new authentication session.
    fn create_session(&self) -> Box<dyn AuthSession>;
}

/// Trait for an authentication session.
/// Handles the handshake process.
#[allow(dead_code)]
pub trait AuthSession: Send + Sync {
    /// Processes a challenge from the server (e.g., from `Proxy-Authenticate` header).
    /// If `challenge` is `None`, it's the initial step.
    /// Returns the value for the `Proxy-Authorization` header, or `None` if no header is needed (e.g. handshake complete).
    fn step(&mut self, challenge: Option<&str>) -> Result<Option<String>>;
}

#[allow(dead_code)]
pub fn create_authenticator(config: &UpstreamConfig) -> Option<Box<dyn UpstreamAuthenticator>> {
    let mut password = config.password.clone();
    if config.use_keyring && password.is_none() {
        if let Some(username) = &config.username {
            if let Ok(entry) = keyring::Entry::new("ferrovela", username) {
                if let Ok(pw) = entry.get_password() {
                    password = Some(pw);
                }
            }
        }
    }

    match config.auth_type.as_str() {
        "basic" => {
            if let (Some(u), Some(p)) = (&config.username, &password) {
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
        "ntlm" => {
            if let (Some(u), Some(p)) = (&config.username, &password) {
                Some(Box::new(ntlm::NtlmAuthenticator::new(
                    u.clone(),
                    p.clone(),
                    config.domain.clone().unwrap_or_default(),
                    config.workstation.clone().unwrap_or_default(),
                )))
            } else {
                None
            }
        }
        _ => None,
    }
}
