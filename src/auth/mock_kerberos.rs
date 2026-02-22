use anyhow::Result;

use super::UpstreamAuthenticator;

pub struct MockKerberosAuthenticator;

impl MockKerberosAuthenticator {
    pub fn new() -> Self {
        Self
    }
}

impl UpstreamAuthenticator for MockKerberosAuthenticator {
    fn get_auth_header(&self) -> Result<String> {
        Ok("Negotiate MockKerberosToken".to_string())
    }
}
