#![allow(dead_code)]
use anyhow::Result;

use super::{AuthSession, UpstreamAuthenticator};

pub struct MockKerberosAuthenticator;

impl Default for MockKerberosAuthenticator {
    fn default() -> Self {
        Self::new()
    }
}

impl MockKerberosAuthenticator {
    pub fn new() -> Self {
        Self
    }
}

impl UpstreamAuthenticator for MockKerberosAuthenticator {
    fn create_session(&self) -> Box<dyn AuthSession> {
        Box::new(MockKerberosSession)
    }
}

pub struct MockKerberosSession;

impl AuthSession for MockKerberosSession {
    fn step(&mut self, _challenge: Option<&str>) -> Result<Option<String>> {
        Ok(Some("Negotiate MockKerberosToken".to_string()))
    }
}
