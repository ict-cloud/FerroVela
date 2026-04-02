use anyhow::Result;
use core_foundation::base::{CFGetTypeID, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanGetTypeID};
use core_foundation::number::{CFNumber, CFNumberGetTypeID};
use core_foundation::string::{CFString, CFStringGetTypeID};
use core_foundation_sys::preferences::{
    CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFPreferencesSetAppValue,
};
use std::ptr;

const APP_ID: &str = "com.ictcloud.ferrovela";

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
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            pac_file: None,
            allow_private_ips: false,
        }
    }
}

pub fn default_port() -> u16 {
    3128
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

#[derive(Debug, Clone, Default)]
pub struct ExceptionsConfig {
    pub hosts: Vec<String>,
}

impl ExceptionsConfig {
    pub fn matches(&self, host: &str) -> bool {
        self.hosts
            .iter()
            .any(|pattern| Self::host_matches_pattern(pattern, host))
    }

    fn host_matches_pattern(pattern: &str, host: &str) -> bool {
        if pattern == host {
            return true;
        }
        if pattern.starts_with("*.") {
            let suffix = &pattern[1..]; // e.g. ".example.com"
                                        // The host must end with the suffix AND have at least one character
                                        // before it (so ".example.com" does not match "*.example.com").
            if host.len() > suffix.len() && host.ends_with(suffix) {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// CFPreferences helpers
// ---------------------------------------------------------------------------

fn read_cf_string(key: &str) -> Option<String> {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    unsafe {
        let value =
            CFPreferencesCopyAppValue(cf_key.as_concrete_TypeRef(), cf_app.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        if CFGetTypeID(value) != CFStringGetTypeID() {
            core_foundation::base::CFRelease(value);
            return None;
        }
        let cf_str = CFString::wrap_under_create_rule(value as _);
        Some(cf_str.to_string())
    }
}

fn read_cf_bool(key: &str) -> Option<bool> {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    unsafe {
        let value =
            CFPreferencesCopyAppValue(cf_key.as_concrete_TypeRef(), cf_app.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        if CFGetTypeID(value) != CFBooleanGetTypeID() {
            core_foundation::base::CFRelease(value);
            return None;
        }
        let cf_bool = CFBoolean::wrap_under_create_rule(value as _);
        Some(cf_bool.into())
    }
}

fn read_cf_number_u16(key: &str) -> Option<u16> {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    unsafe {
        let value =
            CFPreferencesCopyAppValue(cf_key.as_concrete_TypeRef(), cf_app.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        if CFGetTypeID(value) != CFNumberGetTypeID() {
            core_foundation::base::CFRelease(value);
            return None;
        }
        let cf_num = CFNumber::wrap_under_create_rule(value as _);
        cf_num.to_i64().and_then(|n| u16::try_from(n).ok())
    }
}

fn read_cf_string_array(key: &str) -> Option<Vec<String>> {
    use core_foundation::array::{CFArray, CFArrayGetTypeID};
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    unsafe {
        let value =
            CFPreferencesCopyAppValue(cf_key.as_concrete_TypeRef(), cf_app.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        if CFGetTypeID(value) != CFArrayGetTypeID() {
            core_foundation::base::CFRelease(value);
            return None;
        }
        let cf_array: CFArray<CFString> = CFArray::wrap_under_create_rule(value as _);
        let strings: Vec<String> = cf_array.iter().map(|s| s.to_string()).collect();
        Some(strings)
    }
}

fn write_cf_string(key: &str, value: Option<&str>) {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    unsafe {
        match value {
            Some(v) => {
                let cf_val = CFString::new(v);
                CFPreferencesSetAppValue(
                    cf_key.as_concrete_TypeRef(),
                    cf_val.as_concrete_TypeRef() as _,
                    cf_app.as_concrete_TypeRef(),
                );
            }
            None => {
                CFPreferencesSetAppValue(
                    cf_key.as_concrete_TypeRef(),
                    ptr::null(),
                    cf_app.as_concrete_TypeRef(),
                );
            }
        }
    }
}

fn write_cf_bool(key: &str, value: bool) {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    let cf_val = CFBoolean::from(value);
    unsafe {
        CFPreferencesSetAppValue(
            cf_key.as_concrete_TypeRef(),
            cf_val.as_concrete_TypeRef() as _,
            cf_app.as_concrete_TypeRef(),
        );
    }
}

fn write_cf_number_u16(key: &str, value: u16) {
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    let cf_val = CFNumber::from(value as i64);
    unsafe {
        CFPreferencesSetAppValue(
            cf_key.as_concrete_TypeRef(),
            cf_val.as_concrete_TypeRef() as _,
            cf_app.as_concrete_TypeRef(),
        );
    }
}

fn write_cf_string_array(key: &str, values: Option<&[String]>) {
    use core_foundation::array::CFArray;
    let cf_key = CFString::new(key);
    let cf_app = CFString::new(APP_ID);
    unsafe {
        match values {
            Some(vals) if !vals.is_empty() => {
                let cf_strings: Vec<CFString> = vals.iter().map(|s| CFString::new(s)).collect();
                let cf_array = CFArray::from_CFTypes(&cf_strings);
                CFPreferencesSetAppValue(
                    cf_key.as_concrete_TypeRef(),
                    cf_array.as_concrete_TypeRef() as _,
                    cf_app.as_concrete_TypeRef(),
                );
            }
            _ => {
                CFPreferencesSetAppValue(
                    cf_key.as_concrete_TypeRef(),
                    ptr::null(),
                    cf_app.as_concrete_TypeRef(),
                );
            }
        }
    }
}

fn synchronize() {
    let cf_app = CFString::new(APP_ID);
    unsafe {
        CFPreferencesAppSynchronize(cf_app.as_concrete_TypeRef());
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn load_config() -> Config {
    let port = read_cf_number_u16("proxy_port").unwrap_or(default_port());
    let pac_file = read_cf_string("proxy_pac_file");
    let allow_private_ips = read_cf_bool("proxy_allow_private_ips").unwrap_or(false);

    let proxy = ProxyConfig {
        port,
        pac_file,
        allow_private_ips,
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
    Some(ExceptionsConfig { hosts })
}

pub fn save_config(config: &Config) -> Result<()> {
    write_cf_number_u16("proxy_port", config.proxy.port);
    write_cf_string("proxy_pac_file", config.proxy.pac_file.as_deref());
    write_cf_bool("proxy_allow_private_ips", config.proxy.allow_private_ips);

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

    synchronize();
    Ok(())
}

/// Mutex that serialises all tests touching CFPreferences so they cannot
/// race against each other.  Used by both `config::tests` and UI crate tests.
pub static PREFS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exceptions_exact_match() {
        let exceptions = ExceptionsConfig {
            hosts: vec!["example.com".to_string()],
        };
        assert!(exceptions.matches("example.com"));
        assert!(!exceptions.matches("sub.example.com"));
        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_wildcard_match() {
        let exceptions = ExceptionsConfig {
            hosts: vec!["*.example.com".to_string()],
        };
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
        let exceptions = ExceptionsConfig {
            hosts: vec!["exact.com".to_string(), "*.wild.com".to_string()],
        };
        assert!(exceptions.matches("exact.com"));
        assert!(!exceptions.matches("sub.exact.com"));
        assert!(exceptions.matches("sub.wild.com"));
        assert!(!exceptions.matches("wild.com"));
        assert!(!exceptions.matches("other.com"));
    }

    #[test]
    fn test_exceptions_empty() {
        let exceptions = ExceptionsConfig { hosts: vec![] };
        assert!(!exceptions.matches("example.com"));
    }

    fn reset_preferences() {
        save_config(&Config::default()).unwrap();
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
            exceptions: Some(ExceptionsConfig {
                hosts: vec!["localhost".to_string(), "*.internal".to_string()],
            }),
        };

        save_config(&config).unwrap();
        let loaded = load_config();

        assert_eq!(loaded.proxy.port, 9999);
        assert_eq!(loaded.proxy.pac_file.as_deref(), Some("http://test.pac"));
        assert!(loaded.proxy.allow_private_ips);

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
    fn test_default_config_load() {
        let _lock = PREFS_LOCK.lock().unwrap();
        reset_preferences();
        // With no preferences set, should return defaults
        let config = load_config();
        assert_eq!(config.proxy.port, default_port());
        assert!(config.proxy.pac_file.is_none());
        assert!(!config.proxy.allow_private_ips);
        assert!(config.upstream.is_none());
        assert!(config.exceptions.is_none());
    }
}
