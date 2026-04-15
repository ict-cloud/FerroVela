use iced::{window, Theme};
use std::fmt;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthType {
    #[default]
    None,
    Basic,
    Ntlm,
    Kerberos,
}

impl AuthType {
    pub const ALL: [AuthType; 4] = [
        AuthType::None,
        AuthType::Basic,
        AuthType::Ntlm,
        AuthType::Kerberos,
    ];
}

impl fmt::Display for AuthType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            AuthType::None => "None",
            AuthType::Basic => "Basic",
            AuthType::Ntlm => "NTLM",
            AuthType::Kerberos => "Kerberos",
        };
        write!(f, "{}", label)
    }
}

impl From<&str> for AuthType {
    fn from(s: &str) -> Self {
        match s {
            "basic" => AuthType::Basic,
            "ntlm" => AuthType::Ntlm,
            "kerberos" => AuthType::Kerberos,
            _ => AuthType::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServiceStatus {
    #[default]
    Stopped,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Proxy,
    Upstream,
    Exceptions,
    Advanced,
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    // Config form
    TabSelected(Tab),
    ProxyPortChanged(String),
    PacFileChanged(String),
    UpstreamAuthTypeChanged(AuthType),
    UpstreamUsernameChanged(String),
    UpstreamPasswordChanged(String),
    UpstreamDomainChanged(String),
    UpstreamWorkstationChanged(String),
    UpstreamProxyUrlChanged(String),
    UpstreamUseKeyringToggled(bool),
    ExceptionsHostsChanged(String),
    AllowPrivateIpsToggled(bool),
    ProxyListenIpChanged(String),
    AdvancedUnlockRequested,
    AdvancedUnlockResult(bool),
    // Service
    ToggleService(bool),
    RestartService,
    PollStatus,
    // Log window
    OpenLogs,
    OpenLogsAt(Option<iced::Point>),
    Tick,
    LogSearchChanged(String),
    // IPC / window management
    External,
    WindowCloseRequested(window::Id),
    WindowClosed(window::Id),
    IdCaptured(window::Id),
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct ConfigEditor {
    // Navigation
    pub active_tab: Tab,
    // Proxy tab
    pub proxy_port: String,
    pub pac_file: String,
    pub allow_private_ips: bool,
    // Advanced tab
    pub proxy_listen_ip: String,
    pub proxy_listen_ip_error: Option<String>,
    pub advanced_unlocked: bool,
    // Upstream tab
    pub upstream_auth_type: AuthType,
    pub upstream_username: String,
    pub upstream_password: String,
    pub upstream_use_keyring: bool,
    pub upstream_domain: String,
    pub upstream_workstation: String,
    pub upstream_proxy_url: String,
    // Exceptions tab
    pub exceptions_hosts: String,
    // Status bar
    pub status: String,
    pub status_is_error: bool,
    pub status_timestamp: Option<std::time::Instant>,
    // Validation errors
    pub proxy_port_error: Option<String>,
    pub pac_file_error: Option<String>,
    pub upstream_proxy_url_error: Option<String>,
    // Service control
    pub service_status: ServiceStatus,
    pub restart_needed: bool,
    // Appearance
    pub appearance: Theme,
    // Log window
    pub show_logs: bool,
    pub log_content: String,
    pub log_search: String,
    // Window management
    pub main_window_id: Option<window::Id>,
    pub log_window_id: Option<window::Id>,
}
