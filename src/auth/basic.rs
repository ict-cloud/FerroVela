use anyhow::Result;
use base64::prelude::*;

use super::UpstreamAuthenticator;

pub struct BasicAuthenticator {
    username: String,
    password: String,
}

impl BasicAuthenticator {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }
}

impl UpstreamAuthenticator for BasicAuthenticator {
    fn get_auth_header(&self) -> Result<String> {
        let creds = format!("{}:{}", self.username, self.password);
        let encoded = BASE64_STANDARD.encode(creds);
        Ok(format!("Basic {}", encoded))
    }
}
