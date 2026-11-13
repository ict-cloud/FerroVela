use anyhow::{Context, Result};
use base64::prelude::*;
use libgssapi::context::{ClientCtx, CtxFlags};
use libgssapi::name::Name;
use libgssapi::oid::{GSS_MECH_KRB5, GSS_NT_HOSTBASED_SERVICE};
use log::debug;

use super::{AuthSession, UpstreamAuthenticator};

pub struct KerberosAuthenticator;

impl UpstreamAuthenticator for KerberosAuthenticator {
    fn create_session(&self, proxy_host: &str) -> Box<dyn AuthSession> {
        // RFC 4559: "The service name for the GSS_Init_sec_context call is 'HTTP@<hostname>'."
        // The SPN is built per session because a PAC file may select a
        // different upstream proxy for every request.
        Box::new(KerberosSession {
            service_name: format!("HTTP@{}", proxy_host),
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
            } else {
                // Bare "Negotiate", or another scheme listed first when the
                // proxy offers several (NTLM, Basic): start without a token.
                None
            }
        } else {
            None
        };

        if self.ctx.is_none() {
            debug!("Initializing GSSAPI context for: {}", self.service_name);
            let name = Name::new(self.service_name.as_bytes(), Some(GSS_NT_HOSTBASED_SERVICE))
                .context("Failed to create GSS Name")?;

            let ctx = ClientCtx::new(
                None,
                name,
                CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_REPLAY_FLAG,
                Some(GSS_MECH_KRB5),
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
