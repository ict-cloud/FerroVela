use anyhow::{Context, Result};
use base64::prelude::*;
use libgssapi::context::{ClientCtx, CtxFlags};
use libgssapi::name::Name;
use libgssapi::oid::{GSS_MECH_KRB5, GSS_NT_HOSTBASED_SERVICE};
use log::debug;

use super::UpstreamAuthenticator;

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
    fn get_auth_header(&self) -> Result<String> {
        debug!("Initializing GSSAPI context for: {}", self.service_name);

        let name = Name::new(self.service_name.as_bytes(), Some(&GSS_NT_HOSTBASED_SERVICE))
            .context("Failed to create GSS Name")?;

        let mut ctx = ClientCtx::new(
            None,
            name,
            CtxFlags::GSS_C_MUTUAL_FLAG | CtxFlags::GSS_C_REPLAY_FLAG,
            Some(&GSS_MECH_KRB5),
        );

        let token = match ctx.step(None, None) {
            Ok(Some(token)) => token,
            Ok(None) => return Err(anyhow::anyhow!("GSSAPI context established without token")),
            Err(e) => return Err(anyhow::anyhow!("GSSAPI step failed: {}", e)),
        };

        let encoded = BASE64_STANDARD.encode(&*token);
        Ok(format!("Negotiate {}", encoded))
    }
}
