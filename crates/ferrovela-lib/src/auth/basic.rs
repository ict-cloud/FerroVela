use anyhow::Result;
use base64::prelude::*;
use std::sync::Arc;

use super::{AuthSession, UpstreamAuthenticator};

/// Stateless Basic-auth authenticator.
///
/// The `user:password` base64 encoding is computed **once at construction**
/// and stored as an `Arc<str>`.  `create_session()` is then just an atomic
/// reference-count increment — no heap allocation, no crypto.
pub struct BasicAuthenticator {
    /// Pre-encoded Base64 of `"username:password"`.
    encoded: Arc<str>,
}

impl BasicAuthenticator {
    pub fn new(username: String, password: String) -> Self {
        let creds = format!("{}:{}", username, password);
        let encoded: Arc<str> = BASE64_STANDARD.encode(creds).into();
        Self { encoded }
    }
}

impl UpstreamAuthenticator for BasicAuthenticator {
    fn create_session(&self) -> Box<dyn AuthSession> {
        Box::new(BasicSession {
            encoded: Arc::clone(&self.encoded),
        })
    }
}

pub struct BasicSession {
    encoded: Arc<str>,
}

impl AuthSession for BasicSession {
    fn step(&mut self, _challenge: Option<&str>) -> Result<Option<String>> {
        Ok(Some(format!("Basic {}", self.encoded)))
    }
}
