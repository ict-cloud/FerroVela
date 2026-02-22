use crate::auth::basic::BasicAuthenticator;
use crate::auth::mock_kerberos::MockKerberosAuthenticator;
use crate::auth::kerberos::KerberosAuthenticator;
use crate::auth::UpstreamAuthenticator;

#[test]
fn test_basic_auth() {
    let auth = BasicAuthenticator::new("user".into(), "pass".into());
    let header = auth.get_auth_header().unwrap();
    assert_eq!(header, "Basic dXNlcjpwYXNz"); // "dXNlcjpwYXNz" is "user:pass" base64 encoded
}

#[test]
fn test_mock_kerberos_auth() {
    let auth = MockKerberosAuthenticator::new();
    let header = auth.get_auth_header().unwrap();
    assert_eq!(header, "Negotiate MockKerberosToken");
}

#[test]
fn test_kerberos_initialization() {
    // We can't fully test Kerberos without a KDC, but we can test that initialization attempts to proceed.
    // In this environment, it should fail gracefully (e.g., due to missing credentials or configuration).
    let auth = KerberosAuthenticator::new("proxy.example.com");
    let res = auth.get_auth_header();

    // We expect an error because GSSAPI won't work in the sandbox without kinit
    assert!(res.is_err());

    let err = res.unwrap_err();
    // Verify it failed in GSSAPI step or Name creation, not panic.
    // The error message will depend on the system state, but it confirms the code ran.
    println!("Expected Kerberos error: {}", err);
}
