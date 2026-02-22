use anyhow::Result;

pub mod basic;
pub mod kerberos;
pub mod mock_kerberos;

/// Trait for upstream authentication strategies.
pub trait UpstreamAuthenticator {
    /// Generates the value for the `Proxy-Authorization` header.
    /// Returns the full header value, e.g., "Basic <token>" or "Negotiate <token>".
    fn get_auth_header(&self) -> Result<String>;
}
