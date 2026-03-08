use anyhow::{anyhow, Result};
use base64::prelude::*;
use log::debug;
use ntlmclient::{
    get_ntlm_time, respond_challenge_ntlm_v2, Credentials, Flags, Message, NegotiateMessage,
};

use super::{AuthSession, UpstreamAuthenticator};

pub struct NtlmAuthenticator {
    username: String,
    password: String,
    domain: String,
    workstation: String,
}

impl NtlmAuthenticator {
    pub fn new(username: String, password: String, domain: String, workstation: String) -> Self {
        Self {
            username,
            password,
            domain,
            workstation,
        }
    }
}

impl UpstreamAuthenticator for NtlmAuthenticator {
    fn create_session(&self) -> Box<dyn AuthSession> {
        Box::new(NtlmSession {
            username: self.username.clone(),
            password: self.password.clone(),
            domain: self.domain.clone(),
            workstation: self.workstation.clone(),
            state: NtlmState::Initial,
        })
    }
}

enum NtlmState {
    Initial,
    Challenge,
    Complete,
}

struct NtlmSession {
    username: String,
    password: String,
    domain: String,
    workstation: String,
    state: NtlmState,
}

impl AuthSession for NtlmSession {
    fn step(&mut self, challenge: Option<&str>) -> Result<Option<String>> {
        match self.state {
            NtlmState::Initial => {
                debug!("NTLM: Generating Type 1 (Negotiate) message");
                let flags = Flags::NEGOTIATE_UNICODE
                    | Flags::REQUEST_TARGET
                    | Flags::NEGOTIATE_NTLM
                    | Flags::NEGOTIATE_WORKSTATION_SUPPLIED;

                let msg = Message::Negotiate(NegotiateMessage {
                    flags,
                    supplied_domain: self.domain.clone(),
                    supplied_workstation: self.workstation.clone(),
                    os_version: Default::default(),
                });

                let bytes = msg
                    .to_bytes()
                    .map_err(|e| anyhow!("Failed to encode NTLM Type 1: {:?}", e))?;
                let encoded = BASE64_STANDARD.encode(&bytes);

                self.state = NtlmState::Challenge;
                Ok(Some(format!("NTLM {}", encoded)))
            }
            NtlmState::Challenge => {
                let challenge_str = challenge.ok_or_else(|| anyhow!("NTLM expected challenge"))?;
                if !challenge_str.starts_with("NTLM ") {
                    // It might be just "NTLM" if something is wrong or different stage?
                    // But here we expect Type 2 message.
                    return Err(anyhow!("Invalid NTLM challenge header: {}", challenge_str));
                }
                let b64 = challenge_str[5..].trim();
                let bytes = BASE64_STANDARD.decode(b64)?;

                let message = Message::try_from(bytes.as_slice())
                    .map_err(|e| anyhow!("Failed to parse NTLM challenge: {:?}", e))?;

                let challenge_msg = match message {
                    Message::Challenge(c) => c,
                    _ => {
                        return Err(anyhow!(
                            "Expected NTLM Challenge message, got {:?}",
                            message
                        ))
                    }
                };

                debug!("NTLM: Generating Type 3 (Authenticate) message");

                // Collect target info
                let target_info_bytes: Vec<u8> = challenge_msg
                    .target_information
                    .iter()
                    .flat_map(|ie| ie.to_bytes())
                    .collect();

                let creds = Credentials {
                    username: self.username.clone(),
                    password: self.password.clone(),
                    domain: self.domain.clone(),
                };

                let response = respond_challenge_ntlm_v2(
                    challenge_msg.challenge,
                    &target_info_bytes,
                    get_ntlm_time(),
                    &creds,
                );

                let auth_flags = Flags::NEGOTIATE_UNICODE | Flags::NEGOTIATE_NTLM;

                let auth_msg = response.to_message(&creds, &self.workstation, auth_flags);
                let auth_bytes = auth_msg
                    .to_bytes()
                    .map_err(|e| anyhow!("Failed to encode NTLM Type 3: {:?}", e))?;
                let encoded = BASE64_STANDARD.encode(&auth_bytes);

                self.state = NtlmState::Complete;
                Ok(Some(format!("NTLM {}", encoded)))
            }
            NtlmState::Complete => {
                // Connection authenticated.
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ntlm_invalid_challenge() {
        let auth = NtlmAuthenticator::new(
            "user".into(),
            "pass".into(),
            "DOMAIN".into(),
            "WORKSTATION".into(),
        );
        let mut session = auth.create_session();

        // Step 1: Negotiate - this advances state to Challenge
        let res1 = session.step(None);
        assert!(res1.is_ok());

        // Step 2: Provide an invalid challenge (e.g., Basic auth instead of NTLM)
        let res2 = session.step(Some("Basic dXNlcjpwYXNz"));
        assert!(res2.is_err());
        let err_msg = res2.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid NTLM challenge header"));
    }
}
