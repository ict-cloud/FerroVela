use base64::Engine;
use ferrovela::auth::basic::BasicAuthenticator;
use ferrovela::auth::kerberos::KerberosAuthenticator;
use ferrovela::auth::mock_kerberos::MockKerberosAuthenticator;
use ferrovela::auth::ntlm::NtlmAuthenticator;
use ferrovela::auth::UpstreamAuthenticator;

#[test]
fn test_basic_auth() {
    let auth = BasicAuthenticator::new("user".into(), "pass".into());
    let mut session = auth.create_session();
    let header = session.step(None).unwrap().unwrap();
    assert_eq!(header, "Basic dXNlcjpwYXNz"); // "dXNlcjpwYXNz" is "user:pass" base64 encoded
}

#[test]
fn test_mock_kerberos_auth() {
    let auth = MockKerberosAuthenticator::new();
    let mut session = auth.create_session();
    let header = session.step(None).unwrap().unwrap();
    assert_eq!(header, "Negotiate MockKerberosToken");
}

#[test]
fn test_kerberos_initialization() {
    // We can't fully test Kerberos without a KDC, but we can test that initialization attempts to proceed.
    // In this environment, it should fail gracefully (e.g., due to missing credentials or configuration).
    let auth = KerberosAuthenticator::new("proxy.example.com");
    let mut session = auth.create_session();
    let res = session.step(None);

    // We expect an error because GSSAPI won't work in the sandbox without kinit
    assert!(res.is_err());

    let err = res.unwrap_err();
    // Verify it failed in GSSAPI step or Name creation, not panic.
    // The error message will depend on the system state, but it confirms the code ran.
    println!("Expected Kerberos error: {}", err);
}

#[test]
fn test_ntlm_initialization() {
    let auth = NtlmAuthenticator::new(
        "user".into(),
        "pass".into(),
        "DOMAIN".into(),
        "WORKSTATION".into(),
    );
    let mut session = auth.create_session();

    // Step 1: Negotiate
    let header = session.step(None).unwrap().unwrap();
    assert!(header.starts_with("NTLM "));
    let b64 = header.trim_start_matches("NTLM ");
    let bytes = base64::prelude::BASE64_STANDARD.decode(b64).unwrap();
    // Verify minimal length or content?
    assert!(bytes.len() > 0);
}
