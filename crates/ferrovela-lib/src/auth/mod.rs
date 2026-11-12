use crate::config::UpstreamConfig;
use anyhow::Result;

pub mod basic;
pub mod kerberos;
pub mod mock_kerberos;
pub mod ntlm;

/// Trait for upstream authentication factory.
pub trait UpstreamAuthenticator: Send + Sync {
    /// Creates a new authentication session for the upstream proxy
    /// `proxy_host` (bare hostname; Kerberos derives its SPN from it).
    fn create_session(&self, proxy_host: &str) -> Box<dyn AuthSession>;
}

/// Trait for an authentication session.
/// Handles the handshake process.
pub trait AuthSession: Send {
    /// Processes a challenge from the server (e.g., from `Proxy-Authenticate` header).
    /// If `challenge` is `None`, it's the initial step.
    /// Returns the value for the `Proxy-Authorization` header, or `None` if no header is needed (e.g. handshake complete).
    fn step(&mut self, challenge: Option<&str>) -> Result<Option<String>>;
}

pub fn create_authenticator(config: &UpstreamConfig) -> Option<Box<dyn UpstreamAuthenticator>> {
    let mut password = config.password.clone();
    if config.use_keyring && password.is_none() {
        if let Some(username) = &config.username {
            if let Ok(entry) = keyring_core::Entry::new("ferrovela", username) {
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
        // No proxy_url needed: with a PAC file the proxy is chosen per request
        // and the SPN is derived from it in `create_session`.
        "kerberos" => Some(Box::new(kerberos::KerberosAuthenticator)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UpstreamConfig;

    /// PAC-only setups have no static `proxy_url`; Kerberos must still be
    /// enabled, otherwise CONNECTs go upstream unauthenticated.
    #[test]
    fn kerberos_does_not_require_proxy_url() {
        let config = UpstreamConfig {
            auth_type: "kerberos".to_string(),
            ..Default::default()
        };
        assert!(create_authenticator(&config).is_some());
    }
}
