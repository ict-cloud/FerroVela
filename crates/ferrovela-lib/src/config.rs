use anyhow::Result;
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_foundation::ConcreteCFType;
use core_foundation_sys::preferences::{
    CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFPreferencesSetAppValue,
};
use std::ptr;

/// Preferences domain.  Test builds (this crate's own tests, or dependants
/// enabling the `test-prefs` feature) use a separate domain so tests never
/// touch the user's real settings.
const APP_ID: &str = if cfg!(any(test, feature = "test-prefs")) {
    "com.ictcloud.ferrovela.test"
} else {
    "com.ictcloud.ferrovela"
};

// ---------------------------------------------------------------------------
// Config structs
// ---------------------------------------------------------------------------

#[derive(Default, Debug, Clone)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub upstream: Option<UpstreamConfig>,
    pub exceptions: Option<ExceptionsConfig>,
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub port: u16,
    pub pac_file: Option<String>,
    pub allow_private_ips: bool,
    pub listen_ip: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            pac_file: None,
            allow_private_ips: false,
            listen_ip: default_listen_ip(),
        }
    }
}

impl ProxyConfig {
    /// The IP the proxy should actually bind to.
    ///
    /// Falls back to loopback whenever `allow_private_ips` is disabled,
    /// regardless of what `listen_ip` contains. Exposing the proxy to the
    /// LAN is only meaningful when the SSRF guard also allows internal
    /// targets, so the two settings are enforced together here in the
    /// library — not just in the UI — to close the `defaults write` backdoor.
    pub fn effective_listen_ip(&self) -> &str {
        if self.allow_private_ips {
            &self.listen_ip
        } else {
            "127.0.0.1"
        }
    }
}

pub fn default_port() -> u16 {
    3128
}

pub fn default_listen_ip() -> String {
    "127.0.0.1".to_string()
}

#[derive(Clone)]
pub struct UpstreamConfig {
    pub auth_type: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub use_keyring: bool,
    pub domain: Option<String>,
    pub workstation: Option<String>,
    pub proxy_url: Option<String>,
}

impl std::fmt::Debug for UpstreamConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamConfig")
            .field("auth_type", &self.auth_type)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("use_keyring", &self.use_keyring)
            .field("domain", &self.domain)
            .field("workstation", &self.workstation)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            auth_type: "none".to_string(),
            username: None,
            password: None,
            use_keyring: true,
            domain: None,
            workstation: None,
            proxy_url: None,
        }
    }
}

/// Exception-list configuration, with pre-compiled lookup structures.
///
/// On construction the `hosts` list is partitioned into:
/// - `exact`    — `HashSet<String>` for O(1) exact-match lookups.
/// - `suffixes` — `Vec<String>` of `".example.com"` strings for wildcard
///   patterns like `"*.example.com"` (suffix check, O(n)).
///
/// Typical corporate configs have ≤ 50 exceptions; even in the worst case
/// exact matches (the common hot path) are now O(1) instead of O(n·m).
#[derive(Debug, Clone, Default)]
pub struct ExceptionsConfig {
    pub hosts: Vec<String>,
    exact: std::collections::HashSet<String>,
    suffixes: Vec<String>,
}

impl ExceptionsConfig {
    /// Build an `ExceptionsConfig` from a list of host patterns, pre-compiling
    /// them into the fast lookup structures.
    pub fn new(hosts: Vec<String>) -> Self {
        let mut exact = std::collections::HashSet::with_capacity(hosts.len());
        let mut suffixes = Vec::new();
        for h in &hosts {
            if h.starts_with("*.") {
                // "*.example.com" → ".example.com"
                suffixes.push(h[1..].to_owned());
            } else {
                exact.insert(h.clone());
            }
        }
        Self {
            hosts,
            exact,
            suffixes,
        }
    }

    pub fn matches(&self, host: &str) -> bool {
        if self.exact.contains(host) {
            return true;
        }
        self.suffixes
            .iter()
            .any(|suffix| host.len() > suffix.len() && host.ends_with(suffix.as_str()))
    }
}

// ---------------------------------------------------------------------------
// CFPreferences helpers
// ---------------------------------------------------------------------------

/// Copy the preference `key` and return it if it has the expected CF type.
fn copy<T: ConcreteCFType>(key: &str) -> Option<T> {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    // SAFETY: both refs outlive the call; a non-null result is owned by us
    // (Create rule) and is released when the wrapper drops.
    unsafe {
        let value =
            CFPreferencesCopyAppValue(cf_key.as_concrete_TypeRef(), cf_app.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        CFType::wrap_under_create_rule(value).downcast_into()
    }
}

/// Set the preference `key`, or remove it when `value` is `None`.
fn set(key: &str, value: Option<CFType>) {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    let value_ref = value.as_ref().map_or(ptr::null(), |v| v.as_CFTypeRef());
    // SAFETY: all refs are valid for the duration of the call.
    unsafe {
        CFPreferencesSetAppValue(
            cf_key.as_concrete_TypeRef(),
            value_ref,
            cf_app.as_concrete_TypeRef(),
        );
    }
}

fn read_cf_string(key: &str) -> Option<String> {
    copy::<CFString>(key).map(|s| s.to_string())
}

fn read_cf_bool(key: &str) -> Option<bool> {
    copy::<CFBoolean>(key).map(Into::into)
}

fn read_cf_number_u16(key: &str) -> Option<u16> {
    copy::<CFNumber>(key)?
        .to_i64()
        .and_then(|n| u16::try_from(n).ok())
}

fn read_cf_string_array(key: &str) -> Option<Vec<String>> {
    let array = copy::<CFArray>(key)?;
    Some(
        array
            .iter()
            .filter(|item| !item.is_null())
            // SAFETY: non-null array elements are valid CF objects borrowed from `array`.
            .filter_map(|item| unsafe { CFType::wrap_under_get_rule(*item) }.downcast::<CFString>())
            .map(|s| s.to_string())
            .collect(),
    )
}

fn write_cf_string(key: &str, value: Option<&str>) {
    set(key, value.map(|v| CFString::new(v).into_CFType()));
}

fn write_cf_bool(key: &str, value: bool) {
    set(key, Some(CFBoolean::from(value).into_CFType()));
}

fn write_cf_number_u16(key: &str, value: u16) {
    set(key, Some(CFNumber::from(value as i64).into_CFType()));
}

fn write_cf_string_array(key: &str, values: Option<&[String]>) {
    let array = values.filter(|v| !v.is_empty()).map(|vals| {
        let cf_strings: Vec<CFString> = vals.iter().map(|s| CFString::new(s)).collect();
        CFArray::from_CFTypes(&cf_strings).into_CFType()
    });
    set(key, array);
}

/// Flush pending writes to permanent storage; `false` if the system reports failure.
fn synchronize() -> bool {
    let cf_app = CFString::new(APP_ID);
    // SAFETY: `cf_app` is valid for the duration of the call.
    unsafe { CFPreferencesAppSynchronize(cf_app.as_concrete_TypeRef()) != 0 }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn load_config() -> Config {
    let port = read_cf_number_u16("proxy_port").unwrap_or(default_port());
    let pac_file = read_cf_string("proxy_pac_file");
    let allow_private_ips = read_cf_bool("proxy_allow_private_ips").unwrap_or(false);
    let listen_ip = read_cf_string("proxy_listen_ip").unwrap_or_else(default_listen_ip);

    let proxy = ProxyConfig {
        port,
        pac_file,
        allow_private_ips,
        listen_ip,
    };

    let upstream = load_upstream_config();
    let exceptions = load_exceptions_config();

    Config {
        proxy,
        upstream,
        exceptions,
    }
}

fn load_upstream_config() -> Option<UpstreamConfig> {
    let auth_type = read_cf_string("upstream_auth_type");
    let username = read_cf_string("upstream_username");
    let proxy_url = read_cf_string("upstream_proxy_url");

    // Only construct UpstreamConfig if at least one meaningful field is set
    if (auth_type.as_deref() == Some("none") || auth_type.is_none())
        && username.is_none()
        && proxy_url.is_none()
    {
        return None;
    }

    Some(UpstreamConfig {
        auth_type: auth_type.unwrap_or_else(|| "none".to_string()),
        username,
        password: read_cf_string("upstream_password"),
        use_keyring: read_cf_bool("upstream_use_keyring").unwrap_or(true),
        domain: read_cf_string("upstream_domain"),
        workstation: read_cf_string("upstream_workstation"),
        proxy_url,
    })
}

fn load_exceptions_config() -> Option<ExceptionsConfig> {
    let hosts = read_cf_string_array("exceptions_hosts")?;
    if hosts.is_empty() {
        return None;
    }
    Some(ExceptionsConfig::new(hosts))
}

pub fn save_config(config: &Config) -> Result<()> {
    write_cf_number_u16("proxy_port", config.proxy.port);
    write_cf_string("proxy_pac_file", config.proxy.pac_file.as_deref());
    write_cf_bool("proxy_allow_private_ips", config.proxy.allow_private_ips);
    write_cf_string("proxy_listen_ip", Some(&config.proxy.listen_ip));

    if let Some(ref upstream) = config.upstream {
        write_cf_string("upstream_auth_type", Some(&upstream.auth_type));
        write_cf_string("upstream_username", upstream.username.as_deref());
        write_cf_string("upstream_password", upstream.password.as_deref());
        write_cf_bool("upstream_use_keyring", upstream.use_keyring);
        write_cf_string("upstream_domain", upstream.domain.as_deref());
        write_cf_string("upstream_workstation", upstream.workstation.as_deref());
        write_cf_string("upstream_proxy_url", upstream.proxy_url.as_deref());
    } else {
        write_cf_string("upstream_auth_type", None);
        write_cf_string("upstream_username", None);
        write_cf_string("upstream_password", None);
        write_cf_bool("upstream_use_keyring", false);
        write_cf_string("upstream_domain", None);
        write_cf_string("upstream_workstation", None);
        write_cf_string("upstream_proxy_url", None);
    }

    if let Some(ref exceptions) = config.exceptions {
        write_cf_string_array("exceptions_hosts", Some(&exceptions.hosts));
    } else {
        write_cf_string_array("exceptions_hosts", None);
    }

    if !synchronize() {
        anyhow::bail!("failed to write preferences to permanent storage");
    }
    Ok(())
}

/// Mutex that serialises all tests touching CFPreferences so they cannot
/// race against each other.  Used by both `config::tests` and UI crate tests.
/// Only exists in test builds, so a dependant cannot use it without also
/// getting the test preferences domain.
#[cfg(any(test, feature = "test-prefs"))]
pub static PREFS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exceptions_exact_match() {
        let exceptions = ExceptionsConfig::new(vec!["example.com".to_string()]);
        assert!(exceptions.matches("example.com"));
        assert!(!exceptions.matches("sub.example.com"));
        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_wildcard_match() {
        let exceptions = ExceptionsConfig::new(vec!["*.example.com".to_string()]);
        assert!(exceptions.matches("sub.example.com"));
        assert!(exceptions.matches("deep.sub.example.com"));
        assert!(!exceptions.matches("example.com"));
        assert!(!exceptions.matches("myexample.com"));
        assert!(!exceptions.matches("other.com"));
        // Edge case: a bare dot-prefixed string must not match.
        assert!(!exceptions.matches(".example.com"));
    }

    #[test]
    fn test_exceptions_multiple_patterns() {
        let exceptions =
            ExceptionsConfig::new(vec!["exact.com".to_string(), "*.wild.com".to_string()]);
        assert!(exceptions.matches("exact.com"));
        assert!(!exceptions.matches("sub.exact.com"));
        assert!(exceptions.matches("sub.wild.com"));
        assert!(!exceptions.matches("wild.com"));
        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_empty() {
        let exceptions = ExceptionsConfig::new(vec![]);
        assert!(!exceptions.matches("example.com"));
    }

    fn reset_preferences() {
        save_config(&Config::default()).unwrap();
    }

    #[test]
    fn test_effective_listen_ip() {
        let loopback = ProxyConfig {
            listen_ip: "0.0.0.0".to_string(),
            allow_private_ips: false,
            ..Default::default()
        };
        assert_eq!(loopback.effective_listen_ip(), "127.0.0.1");

        let lan = ProxyConfig {
            listen_ip: "0.0.0.0".to_string(),
            allow_private_ips: true,
            ..Default::default()
        };
        assert_eq!(lan.effective_listen_ip(), "0.0.0.0");

        let loopback_explicit = ProxyConfig {
            listen_ip: "127.0.0.1".to_string(),
            allow_private_ips: true,
            ..Default::default()
        };
        assert_eq!(loopback_explicit.effective_listen_ip(), "127.0.0.1");
    }

    #[test]
    fn test_config_round_trip_via_cfpreferences() {
        let _lock = PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let config = Config {
            proxy: ProxyConfig {
                port: 9999,
                pac_file: Some("http://test.pac".to_string()),
                allow_private_ips: true,
                listen_ip: "0.0.0.0".to_string(),
            },
            upstream: Some(UpstreamConfig {
                auth_type: "basic".to_string(),
                username: Some("testuser".to_string()),
                password: Some("testpass".to_string()),
                use_keyring: false,
                domain: Some("TESTDOMAIN".to_string()),
                workstation: Some("TESTWS".to_string()),
                proxy_url: Some("http://proxy:8080".to_string()),
            }),
            exceptions: Some(ExceptionsConfig::new(vec![
                "localhost".to_string(),
                "*.internal".to_string(),
            ])),
        };

        save_config(&config).unwrap();
        let loaded = load_config();

        assert_eq!(loaded.proxy.port, 9999);
        assert_eq!(loaded.proxy.pac_file.as_deref(), Some("http://test.pac"));
        assert!(loaded.proxy.allow_private_ips);
        assert_eq!(loaded.proxy.listen_ip, "0.0.0.0");

        let upstream = loaded.upstream.unwrap();
        assert_eq!(upstream.auth_type, "basic");
        assert_eq!(upstream.username.as_deref(), Some("testuser"));
        assert_eq!(upstream.password.as_deref(), Some("testpass"));
        assert!(!upstream.use_keyring);
        assert_eq!(upstream.domain.as_deref(), Some("TESTDOMAIN"));
        assert_eq!(upstream.workstation.as_deref(), Some("TESTWS"));
        assert_eq!(upstream.proxy_url.as_deref(), Some("http://proxy:8080"));

        let exceptions = loaded.exceptions.unwrap();
        assert_eq!(exceptions.hosts, vec!["localhost", "*.internal"]);

        // Clean up test preferences
        reset_preferences();
    }

    #[test]
    fn test_wrong_type_reads_as_none() {
        let _lock = PREFS_LOCK.lock().unwrap();
        write_cf_string("proxy_port", Some("not a number"));
        write_cf_bool("exceptions_hosts", true);
        assert_eq!(read_cf_number_u16("proxy_port"), None);
        assert_eq!(read_cf_bool("proxy_port"), None);
        assert_eq!(read_cf_string_array("exceptions_hosts"), None);
        assert_eq!(load_config().proxy.port, default_port());
        reset_preferences();
    }

    #[test]
    fn test_default_config_load() {
        let _lock = PREFS_LOCK.lock().unwrap();
        reset_preferences();
        // With no preferences set, should return defaults
        let config = load_config();
        assert_eq!(config.proxy.port, default_port());
        assert!(config.proxy.pac_file.is_none());
        assert!(!config.proxy.allow_private_ips);
        assert_eq!(config.proxy.listen_ip, default_listen_ip());
        assert!(config.upstream.is_none());
        assert!(config.exceptions.is_none());
    }
}
