use anyhow::Result;
use base64::prelude::*;

use super::{AuthSession, UpstreamAuthenticator};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_authenticator_creates_session() {
        let auth = BasicAuthenticator::new("user".to_string(), "pass".to_string());
        let mut session = auth.create_session();

        let result = session.step(None).unwrap();
        assert_eq!(result, Some("Basic dXNlcjpwYXNz".to_string()));
    }

    #[test]
    fn test_basic_session_ignores_challenge() {
        let mut session = BasicSession {
            username: "admin".to_string(),
            password: "password123".to_string(),
        };

        let result = session.step(Some("Basic realm=\"Some Realm\"")).unwrap();
        assert_eq!(result, Some("Basic YWRtaW46cGFzc3dvcmQxMjM=".to_string()));
    }

    #[test]
    fn test_basic_session_empty_credentials() {
        let mut session = BasicSession {
            username: "".to_string(),
            password: "".to_string(),
        };

        let result = session.step(None).unwrap();
        assert_eq!(result, Some("Basic Og==".to_string()));
    }
}
