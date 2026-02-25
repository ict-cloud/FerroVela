use anyhow::Result;
use base64::prelude::*;

use super::{UpstreamAuthenticator, AuthSession};

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
    fn create_session(&self) -> Box<dyn AuthSession> {
        Box::new(BasicSession {
            username: self.username.clone(),
            password: self.password.clone(),
        })
    }
}

pub struct BasicSession {
    username: String,
    password: String,
}

impl AuthSession for BasicSession {
    fn step(&mut self, _challenge: Option<&str>) -> Result<Option<String>> {
        let creds = format!("{}:{}", self.username, self.password);
        let encoded = BASE64_STANDARD.encode(creds);
        Ok(Some(format!("Basic {}", encoded)))
    }
}
