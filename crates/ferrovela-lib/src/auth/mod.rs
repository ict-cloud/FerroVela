use crate::config::UpstreamConfig;
use anyhow::Result;
use log::{debug, warn};

pub mod basic;
pub mod kerberos;
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

/// The upstream password: the configured one, or else the keyring entry for
/// the username when keyring storage is enabled.
pub fn resolve_password(config: &UpstreamConfig) -> Option<String> {
    if let Some(password) = &config.password {
        return Some(password.clone());
    }
    if !config.use_keyring {
        return None;
    }
    let username = config.username.as_deref()?;
    let entry = match keyring_core::Entry::new("ferrovela", username) {
        Ok(entry) => entry,
        Err(e) => {
            warn!("keyring: cannot open entry for {username}: {e}");
            return None;
        }
    };
    match entry.get_password() {
        Ok(password) => Some(password),
        Err(keyring_core::Error::NoEntry) => {
            debug!("keyring: no password stored for {username}");
            None
        }
        Err(e) => {
            warn!("keyring: cannot read password for {username}: {e}");
            None
        }
    }
}

pub fn create_authenticator(config: &UpstreamConfig) -> Option<Box<dyn UpstreamAuthenticator>> {
    let password = resolve_password(config);

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

    /// The configured password wins; the keyring is only consulted when enabled.
    #[test]
    fn resolve_password_prefers_config_and_respects_keyring_flag() {
        let mut config = UpstreamConfig {
            username: Some("user".into()),
            password: Some("secret".into()),
            use_keyring: true,
            ..Default::default()
        };
        assert_eq!(resolve_password(&config).as_deref(), Some("secret"));
        config.password = None;
        config.use_keyring = false;
        assert_eq!(resolve_password(&config), None);
    }
}
