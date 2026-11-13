#[cfg(test)]
mod auth_tests;
#[cfg(test)]
mod connect_tests;
#[cfg(test)]
mod performance_tests;

use crate::auth::{AuthSession, UpstreamAuthenticator};

/// Test double: answers every challenge with a fixed Negotiate token.
struct MockKerberosAuthenticator;

impl UpstreamAuthenticator for MockKerberosAuthenticator {
    fn create_session(&self, _proxy_host: &str) -> Box<dyn AuthSession> {
        Box::new(MockKerberosSession)
    }
}

struct MockKerberosSession;

impl AuthSession for MockKerberosSession {
    fn step(&mut self, _challenge: Option<&str>) -> anyhow::Result<Option<String>> {
        Ok(Some("Negotiate MockKerberosToken".to_string()))
    }
}
