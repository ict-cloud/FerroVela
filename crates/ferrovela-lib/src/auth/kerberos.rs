#![allow(dead_code)]
use anyhow::{Context, Result};
use base64::prelude::*;
use libgssapi::context::{ClientCtx, CtxFlags};
use libgssapi::name::Name;
use libgssapi::oid::{GSS_MECH_KRB5, GSS_NT_HOSTBASED_SERVICE};
use log::debug;

use super::{AuthSession, UpstreamAuthenticator};

pub struct KerberosAuthenticator {
    service_name: String,
}

impl KerberosAuthenticator {
    pub fn new(proxy_host: &str) -> Self {
        // RFC 4559: "The service name for the GSS_Init_sec_context call is 'HTTP@<hostname>'."
        // We assume proxy_host is just the hostname (no port).
        let service_name = format!("HTTP@{}", proxy_host);
        Self { service_name }
    }
}

impl UpstreamAuthenticator for KerberosAuthenticator {
    fn create_session(&self) -> Box<dyn AuthSession> {
        Box::new(KerberosSession {
            service_name: self.service_name.clone(),
            ctx: None,
        })
    }
}

pub struct KerberosSession {
    service_name: String,
    ctx: Option<ClientCtx>,
}

impl AuthSession for KerberosSession {
    fn step(&mut self, challenge: Option<&str>) -> Result<Option<String>> {
        let input_token = if let Some(c) = challenge {
            if c.trim().is_empty() {
                None
            } else if let Some(stripped) = c.strip_prefix("Negotiate ") {
                Some(BASE64_STANDARD.decode(stripped.trim())?)
            } else if c.trim() == "Negotiate" {
                None
            } else {
                return Err(anyhow::anyhow!("Invalid challenge format: {}", c));
            }
        } else {
            None
        };

        if self.ctx.is_none() {
            debug!("Initializing GSSAPI context for: {}", self.service_name);
            let name = Name::new(
                self.service_name.as_bytes(),
                Some(&GSS_NT_HOSTBASED_SERVICE),
            )
            .context("Failed to create GSS Name")?;

            let ctx = ClientCtx::new(
                None,
                name,
                CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_REPLAY_FLAG,
                Some(&GSS_MECH_KRB5),
            );
            self.ctx = Some(ctx);
        }

        let ctx = self
            .ctx
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Kerberos context not initialized"))?;
        match ctx.step(input_token.as_deref(), None) {
            Ok(Some(token)) => {
                let encoded = BASE64_STANDARD.encode(&*token);
                Ok(Some(format!("Negotiate {}", encoded)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("GSSAPI step failed: {}", e)),
        }
    }
}
