use crate::config::{
    default_port, load_config, save_config, Config, ExceptionsConfig, ProxyConfig, UpstreamConfig,
};
use crate::pac::PacEngine;
use crate::proxy::{Proxy, ProxySignal};
use iced::widget::{button, column, pick_list, row, scrollable, text, text_input};
use iced::{window, Alignment, Color, Element, Length, Subscription, Task};
use log::{error, info};
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tokio::task::AbortHandle;

// Global receiver for IPC commands
pub static IPC_RECEIVER: OnceLock<Mutex<Option<mpsc::Receiver<ProxySignal>>>> = OnceLock::new();

pub fn run_ui(config_path: String) -> iced::Result {
    iced::application(
        move || ConfigEditor::new_args(config_path.clone()),
        ConfigEditor::update,
        ConfigEditor::view,
    )
    .title("Ferrovela Configuration")
    .subscription(ConfigEditor::subscription)
    .exit_on_close_request(false)
    .run()
}

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
        write!(
            f,
            "{}",
            match self {
                AuthType::None => "none",
                AuthType::Basic => "basic",
                AuthType::Ntlm => "ntlm",
                AuthType::Kerberos => "kerberos",
            }
        )
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

pub struct ConfigEditor {
    pub path: String,
    // Form fields
    pub proxy_port: String,
    pub pac_file: String,
    pub upstream_auth_type: AuthType,
    pub upstream_username: String,
    pub upstream_password: String,
    pub upstream_domain: String,
    pub upstream_workstation: String,
    pub upstream_proxy_url: String,
    pub exceptions_hosts: String,
    // Status message
    pub status: String,
    // Service control
    pub service_status: ServiceStatus,
    pub proxy_handle: Option<AbortHandle>,
    // Log view
    pub show_logs: bool,
    pub log_content: String,
    // State
    pub window_id: Option<window::Id>,
    // Signal sender for Proxy
    pub signal_sender: mpsc::Sender<ProxySignal>,
}

#[derive(Debug, Clone)]
pub enum Message {
    ProxyPortChanged(String),
    PacFileChanged(String),
    UpstreamAuthTypeChanged(AuthType),
    UpstreamUsernameChanged(String),
    UpstreamPasswordChanged(String),
    UpstreamDomainChanged(String),
    UpstreamWorkstationChanged(String),
    UpstreamProxyUrlChanged(String),
    ExceptionsHostsChanged(String),
    SavePressed,
    ToggleService,
    ToggleLogs,
    Tick,
    External,
    WindowCloseRequested(window::Id),
    IdCaptured(window::Id),
}

impl ConfigEditor {
    pub fn new_args(path: String) -> (Self, Task<Message>) {
        let config = load_config(&path).unwrap_or_default();

        // Initialize IPC channel
        let (tx, rx) = mpsc::channel(32);
        // We only set the global receiver once. If it's already set (re-run?), we might lose the new rx?
        // But application::run usually runs once per process.
        // If we restart UI? application::run blocks.
        let _ = IPC_RECEIVER.set(Mutex::new(Some(rx)));

        (
            Self {
                path,
                proxy_port: config.proxy.port.to_string(),
                pac_file: config.proxy.pac_file.unwrap_or_default(),
                upstream_auth_type: config
                    .upstream
                    .as_ref()
                    .map(|u| AuthType::from(u.auth_type.as_str()))
                    .unwrap_or_default(),
                upstream_username: config
                    .upstream
                    .as_ref()
                    .and_then(|u| u.username.clone())
                    .unwrap_or_default(),
                upstream_password: config
                    .upstream
                    .as_ref()
                    .and_then(|u| u.password.clone())
                    .unwrap_or_default(),
                upstream_domain: config
                    .upstream
                    .as_ref()
                    .and_then(|u| u.domain.clone())
                    .unwrap_or_default(),
                upstream_workstation: config
                    .upstream
                    .as_ref()
                    .and_then(|u| u.workstation.clone())
                    .unwrap_or_default(),
                upstream_proxy_url: config
                    .upstream
                    .as_ref()
                    .and_then(|u| u.proxy_url.clone())
                    .unwrap_or_default(),
                exceptions_hosts: config
                    .exceptions
                    .as_ref()
                    .map(|e| e.hosts.join(", "))
                    .unwrap_or_default(),
                status: String::new(),
                service_status: ServiceStatus::Stopped,
                proxy_handle: None,
                show_logs: false,
                log_content: String::new(),
                window_id: None,
                signal_sender: tx,
            },
            Task::none(),
        )
    }

    fn build_config(&self) -> Config {
        let port = self.proxy_port.parse().unwrap_or(default_port());
        let pac_file = if self.pac_file.trim().is_empty() {
            None
        } else {
            Some(self.pac_file.trim().to_string())
        };

        let upstream = if self.upstream_auth_type == AuthType::None
            && self.upstream_username.is_empty()
            && self.upstream_proxy_url.is_empty()
        {
            None
        } else {
            Some(UpstreamConfig {
                auth_type: self.upstream_auth_type.to_string(),
                username: if self.upstream_username.trim().is_empty() {
                    None
                } else {
                    Some(self.upstream_username.trim().to_string())
                },
                password: if self.upstream_password.trim().is_empty() {
                    None
                } else {
                    Some(self.upstream_password.trim().to_string())
                },
                domain: if self.upstream_domain.trim().is_empty() {
                    None
                } else {
                    Some(self.upstream_domain.trim().to_string())
                },
                workstation: if self.upstream_workstation.trim().is_empty() {
                    None
                } else {
                    Some(self.upstream_workstation.trim().to_string())
                },
                proxy_url: if self.upstream_proxy_url.trim().is_empty() {
                    None
                } else {
                    Some(self.upstream_proxy_url.trim().to_string())
                },
            })
        };

        let exceptions = if self.exceptions_hosts.trim().is_empty() {
            None
        } else {
            let hosts: Vec<String> = self
                .exceptions_hosts
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            Some(ExceptionsConfig { hosts })
        };

        Config {
            proxy: ProxyConfig { port, pac_file },
            upstream,
            exceptions,
        }
    }

    fn save_current_config(&mut self) {
        let config = self.build_config();
        match save_config(&self.path, &config) {
            Ok(_) => self.status = "Saved successfully!".to_string(),
            Err(e) => self.status = format!("Error saving: {}", e),
        }
    }

    fn load_logs(&mut self) {
        if let Ok(mut file) = std::fs::File::open("service.log") {
            if let Ok(metadata) = file.metadata() {
                let len = metadata.len();
                let offset = len.saturating_sub(10000);
                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    let mut buffer = String::new();
                    if file.read_to_string(&mut buffer).is_ok() {
                        self.log_content = buffer;
                    }
                }
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ProxyPortChanged(value) => {
                self.proxy_port = value;
                self.save_current_config();
            }
            Message::PacFileChanged(value) => {
                self.pac_file = value;
                self.save_current_config();
            }
            Message::UpstreamAuthTypeChanged(value) => {
                self.upstream_auth_type = value;
                self.save_current_config();
            }
            Message::UpstreamUsernameChanged(value) => {
                self.upstream_username = value;
                self.save_current_config();
            }
            Message::UpstreamPasswordChanged(value) => {
                self.upstream_password = value;
                self.save_current_config();
            }
            Message::UpstreamDomainChanged(value) => {
                self.upstream_domain = value;
                self.save_current_config();
            }
            Message::UpstreamWorkstationChanged(value) => {
                self.upstream_workstation = value;
                self.save_current_config();
            }
            Message::UpstreamProxyUrlChanged(value) => {
                self.upstream_proxy_url = value;
                self.save_current_config();
            }
            Message::ExceptionsHostsChanged(value) => {
                self.exceptions_hosts = value;
                self.save_current_config();
            }
            Message::SavePressed => {
                self.save_current_config();
            }
            Message::ToggleService => match self.service_status {
                ServiceStatus::Stopped => {
                    let config = Arc::new(self.build_config());
                    let pac_path = config.proxy.pac_file.clone();
                    let sender = self.signal_sender.clone();

                    let handle = tokio::spawn(async move {
                        let pac_engine = if let Some(path) = pac_path {
                            info!("Loading PAC file from {}", path);
                            match PacEngine::new(&path).await {
                                Ok(engine) => Some(engine),
                                Err(e) => {
                                    error!("Failed to load PAC file: {}", e);
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        let proxy = Proxy::new(config.clone(), pac_engine, Some(sender));
                        if let Err(e) = proxy.run().await {
                            error!("Proxy error: {}", e);
                        }
                    });

                    self.proxy_handle = Some(handle.abort_handle());
                    self.service_status = ServiceStatus::Running;
                    self.status = "Service Started".to_string();
                }
                ServiceStatus::Running => {
                    if let Some(handle) = self.proxy_handle.take() {
                        handle.abort();
                    }
                    self.service_status = ServiceStatus::Stopped;
                    self.status = "Service Stopped".to_string();
                }
            },
            Message::ToggleLogs => {
                self.show_logs = !self.show_logs;
                if self.show_logs {
                    self.load_logs();
                }
            }
            Message::Tick => {
                if self.show_logs {
                    self.load_logs();
                }
            }
            Message::External => {
                if let Some(id) = self.window_id {
                    // Minimize(false) usually restores it
                    return window::minimize(id, false).chain(window::gain_focus(id));
                }
            }
            Message::WindowCloseRequested(id) => {
                if self.service_status == ServiceStatus::Running {
                    return window::minimize(id, true);
                } else {
                    return window::close(id);
                }
            }
            Message::IdCaptured(id) => {
                self.window_id = Some(id);
            }
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let tick = if self.show_logs {
            iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        };

        // IPC Subscription
        let ipc = Subscription::run(ipc_stream);

        let events = iced::event::listen_with(|event, _status, id| match event {
            iced::Event::Window(window::Event::CloseRequested) => {
                Some(Message::WindowCloseRequested(id))
            }
            iced::Event::Window(_) => Some(Message::IdCaptured(id)),
            _ => None,
        });

        Subscription::batch(vec![tick, ipc, events])
    }

    pub fn view(&self) -> Element<'_, Message> {
        if self.show_logs {
            return self.view_logs();
        }

        column![
            self.view_service_control(),
            self.view_proxy_config(),
            self.view_upstream_config(),
            self.view_exceptions_config(),
            button("Save").on_press(Message::SavePressed),
            text(&self.status)
        ]
        .spacing(20)
        .padding(20)
        .into()
    }

    fn view_logs(&self) -> Element<'_, Message> {
        column![
            button("Back").on_press(Message::ToggleLogs),
            scrollable(text(&self.log_content))
                .height(Length::Fill)
                .width(Length::Fill)
        ]
        .padding(20)
        .spacing(10)
        .height(Length::Fill)
        .width(Length::Fill)
        .into()
    }

    fn view_service_control(&self) -> Element<'_, Message> {
        let status_color = match self.service_status {
            ServiceStatus::Running => Color::from_rgb(0.0, 0.8, 0.0),
            ServiceStatus::Stopped => Color::from_rgb(0.5, 0.5, 0.5),
        };

        let status_text = match self.service_status {
            ServiceStatus::Running => "Running",
            ServiceStatus::Stopped => "Stopped",
        };
        let toggle_text = match self.service_status {
            ServiceStatus::Running => "Stop",
            ServiceStatus::Stopped => "Start",
        };

        column![
            text("Service Control").size(20),
            row![
                text("●").size(20).color(status_color),
                text(status_text),
                button(toggle_text).on_press(Message::ToggleService),
                button("Show Logs").on_press(Message::ToggleLogs),
            ]
            .spacing(20)
            .align_y(Alignment::Center)
        ]
        .spacing(20)
        .into()
    }

    fn view_proxy_config(&self) -> Element<'_, Message> {
        column![
            text("Proxy Configuration").size(20),
            row![
                text("Port:"),
                text_input("3128", &self.proxy_port).on_input(Message::ProxyPortChanged)
            ]
            .spacing(10),
            row![
                text("PAC File:"),
                text_input("Path to PAC file", &self.pac_file).on_input(Message::PacFileChanged)
            ]
            .spacing(10)
        ]
        .spacing(20)
        .into()
    }

    fn view_upstream_config(&self) -> Element<'_, Message> {
        column![
            text("Upstream Configuration").size(20),
            row![
                text("Auth Type:"),
                pick_list(
                    &AuthType::ALL[..],
                    Some(self.upstream_auth_type),
                    Message::UpstreamAuthTypeChanged
                )
            ]
            .spacing(10),
            row![
                text("Username:"),
                text_input("Username", &self.upstream_username)
                    .on_input(Message::UpstreamUsernameChanged)
            ]
            .spacing(10),
            row![
                text("Password:"),
                text_input("Password", &self.upstream_password)
                    .on_input(Message::UpstreamPasswordChanged)
                    .secure(true)
            ]
            .spacing(10),
            row![
                text("Domain (NTLM):"),
                text_input("Domain", &self.upstream_domain)
                    .on_input(Message::UpstreamDomainChanged)
            ]
            .spacing(10),
            row![
                text("Workstation (NTLM):"),
                text_input("Workstation", &self.upstream_workstation)
                    .on_input(Message::UpstreamWorkstationChanged)
            ]
            .spacing(10),
            row![
                text("Proxy URL:"),
                text_input("http://upstream:port", &self.upstream_proxy_url)
                    .on_input(Message::UpstreamProxyUrlChanged)
            ]
            .spacing(10)
        ]
        .spacing(20)
        .into()
    }

    fn view_exceptions_config(&self) -> Element<'_, Message> {
        column![
            text("Exceptions").size(20),
            row![
                text("Hosts (comma separated):"),
                text_input("localhost, 127.0.0.1", &self.exceptions_hosts)
                    .on_input(Message::ExceptionsHostsChanged)
            ]
            .spacing(10)
        ]
        .spacing(20)
        .into()
    }
}

// Helper for Subscription::run
fn ipc_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::futures::stream::unfold((), move |_| async move {
        if let Some(guard_lock) = IPC_RECEIVER.get() {
            // Lock the mutex. This is async mutex.
            let mut guard = guard_lock.lock().await;
            if let Some(rx) = guard.as_mut() {
                if let Some(cmd) = rx.recv().await {
                    match cmd {
                        ProxySignal::Show => return Some((Message::External, ())),
                    }
                }
            }
        }
        // If receiver missing or closed, wait forever
        std::future::pending::<()>().await;
        None
    })
}
