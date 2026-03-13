use crate::proxy::ProxySignal;
use iced::window;
use std::fmt;
use std::sync::OnceLock;
use tokio::sync::{mpsc, Mutex};
use tokio::task::AbortHandle;

/// Global receiver for IPC commands sent by a second instance of the app.
pub static IPC_RECEIVER: OnceLock<Mutex<Option<mpsc::Receiver<ProxySignal>>>> = OnceLock::new();

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
    // Service
    ToggleService(bool),
    // Log window
    OpenLogs,
    LogsOpened(window::Id),
    Tick,
    // IPC / window management
    External,
    WindowCloseRequested(window::Id),
    IdCaptured(window::Id),
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct ConfigEditor {
    pub path: String,
    // Navigation
    pub active_tab: Tab,
    // Proxy tab
    pub proxy_port: String,
    pub pac_file: String,
    pub allow_private_ips: bool,
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
    // Service control
    pub service_status: ServiceStatus,
    pub proxy_handle: Option<AbortHandle>,
    // Log window
    pub show_logs: bool,
    pub log_content: String,
    // Window management
    pub main_window_id: Option<window::Id>,
    pub log_window_id: Option<window::Id>,
    // IPC channel to the running Proxy task
    pub signal_sender: mpsc::Sender<ProxySignal>,
}
