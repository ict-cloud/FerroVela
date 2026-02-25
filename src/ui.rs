use crate::config::{
    default_port, load_config, save_config, Config, ExceptionsConfig, ProxyConfig, UpstreamConfig,
};
use crate::pac::PacEngine;
use crate::proxy::{Proxy, ProxySignal};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_input, toggler, Space,
};
use iced::{window, Alignment, Element, Length, Subscription, Task, Theme};
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
    iced::daemon(
        move || ConfigEditor::new_args(config_path.clone()),
        ConfigEditor::update,
        ConfigEditor::view,
    )
    .theme(|_: &ConfigEditor, _: window::Id| Theme::Light)
    .subscription(ConfigEditor::subscription)
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
                AuthType::None => "None",
                AuthType::Basic => "Basic",
                AuthType::Ntlm => "NTLM",
                AuthType::Kerberos => "Kerberos",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Proxy,
    Upstream,
    Exceptions,
}

pub struct ConfigEditor {
    pub path: String,
    // Tabs
    pub active_tab: Tab,
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
    // Advanced
    pub allow_private_ips: bool,
    // Status message
    pub status: String,
    // Service control
    pub service_status: ServiceStatus,
    pub proxy_handle: Option<AbortHandle>,
    // Log view
    pub show_logs: bool,
    pub log_content: String,
    // Window Management
    pub main_window_id: Option<window::Id>,
    pub log_window_id: Option<window::Id>,
    // Signal sender for Proxy
    pub signal_sender: mpsc::Sender<ProxySignal>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TabSelected(Tab),
    ProxyPortChanged(String),
    PacFileChanged(String),
    UpstreamAuthTypeChanged(AuthType),
    UpstreamUsernameChanged(String),
    UpstreamPasswordChanged(String),
    UpstreamDomainChanged(String),
    UpstreamWorkstationChanged(String),
    UpstreamProxyUrlChanged(String),
    ExceptionsHostsChanged(String),
    ToggleService(bool),
    OpenLogs,
    LogsOpened(window::Id),
    Tick,
    External,
    WindowCloseRequested(window::Id),
    IdCaptured(window::Id),
}

impl ConfigEditor {
    pub fn new_args(path: String) -> (Self, Task<Message>) {
        let config = load_config(&path).unwrap_or_default();

        let (tx, rx) = mpsc::channel(32);
        let _ = IPC_RECEIVER.set(Mutex::new(Some(rx)));

        let (main_window_id, open_task) = window::open(window::Settings {
            size: (800.0, 600.0).into(),
            ..Default::default()
        });

        (
            Self {
                path,
                active_tab: Tab::Proxy,
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
                allow_private_ips: config.proxy.allow_private_ips,
                status: String::new(),
                service_status: ServiceStatus::Stopped,
                proxy_handle: None,
                show_logs: false,
                log_content: String::new(),
                main_window_id: Some(main_window_id),
                log_window_id: None,
                signal_sender: tx,
            },
            open_task.map(Message::IdCaptured),
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
                auth_type: self.upstream_auth_type.to_string().to_lowercase(),
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
            proxy: ProxyConfig {
                port,
                pac_file,
                allow_private_ips: self.allow_private_ips,
            },
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
                let offset = if len > 10000 { len - 10000 } else { 0 };
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
            Message::TabSelected(tab) => {
                self.active_tab = tab;
            }
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
            Message::ToggleService(is_running) => {
                if is_running {
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
                } else {
                    if let Some(handle) = self.proxy_handle.take() {
                        handle.abort();
                    }
                    self.service_status = ServiceStatus::Stopped;
                    self.status = "Service Stopped".to_string();
                }
            }
            Message::OpenLogs => {
                if self.log_window_id.is_none() {
                    let (id, open_task) = window::open(window::Settings {
                        size: (800.0, 600.0).into(),
                        ..Default::default()
                    });
                    self.log_content = String::new();
                    self.load_logs();
                    self.show_logs = true;
                    return open_task.map(move |_| Message::LogsOpened(id));
                } else {
                    if let Some(id) = self.log_window_id {
                        return window::gain_focus(id);
                    }
                }
            }
            Message::LogsOpened(id) => {
                self.log_window_id = Some(id);
            }
            Message::Tick => {
                if self.show_logs || self.log_window_id.is_some() {
                    self.load_logs();
                }
            }
            Message::External => {
                if let Some(id) = self.main_window_id {
                    return window::minimize(id, false).chain(window::gain_focus(id));
                }
            }
            Message::WindowCloseRequested(id) => {
                if Some(id) == self.log_window_id {
                    self.log_window_id = None;
                    self.show_logs = false;
                    return window::close(id);
                } else {
                    if self.service_status == ServiceStatus::Running {
                        return window::minimize(id, true);
                    } else {
                        return window::close(id);
                    }
                }
            }
            Message::IdCaptured(id) => {
                if self.log_window_id != Some(id) && self.main_window_id.is_none() {
                    self.main_window_id = Some(id);
                }
            }
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let tick = if self.show_logs || self.log_window_id.is_some() {
            iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        };

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

    pub fn view(&self, window_id: window::Id) -> Element<'_, Message> {
        if Some(window_id) == self.log_window_id {
            return self.view_logs();
        }

        let sidebar = column![
            sidebar_button("Proxy", Tab::Proxy, self.active_tab),
            sidebar_button("Upstream", Tab::Upstream, self.active_tab),
            sidebar_button("Exceptions", Tab::Exceptions, self.active_tab),
        ]
        .spacing(5)
        .padding(10)
        .width(Length::Fixed(150.0));

        let content = match self.active_tab {
            Tab::Proxy => self.view_proxy_config(),
            Tab::Upstream => self.view_upstream_config(),
            Tab::Exceptions => self.view_exceptions_config(),
        };

        let status_text = match self.service_status {
            ServiceStatus::Running => "Running",
            ServiceStatus::Stopped => "Stopped",
        };

        let service_control = row![
            text(status_text),
            Space::new().width(10),
            toggler(self.service_status == ServiceStatus::Running)
                .on_toggle(Message::ToggleService)
                .width(Length::Shrink),
            Space::new().width(20),
            button("Show Logs").on_press(Message::OpenLogs),
        ]
        .align_y(Alignment::Center);

        let main_layout = row![
            sidebar,
            container(column![service_control, Space::new().height(20), content].spacing(10))
                .width(Length::Fill)
                .padding(20)
                .style(rounded_box)
        ];

        main_layout.into()
    }

    fn view_logs(&self) -> Element<'_, Message> {
        column![scrollable(
            text(&self.log_content).font(iced::font::Font::MONOSPACE)
        )]
        .padding(10)
        .into()
    }

    fn view_proxy_config(&self) -> Element<'_, Message> {
        column![
            text("Proxy Settings").size(24),
            Space::new().height(10),
            group_box(
                column![
                    field_row(
                        "Port:",
                        text_input("3128", &self.proxy_port).on_input(Message::ProxyPortChanged)
                    ),
                    field_row(
                        "PAC File:",
                        text_input("Path to PAC file", &self.pac_file)
                            .on_input(Message::PacFileChanged)
                    )
                ]
                .spacing(10)
            )
        ]
        .spacing(10)
        .into()
    }

    fn view_upstream_config(&self) -> Element<'_, Message> {
        column![
            text("Upstream Settings").size(24),
            Space::new().height(10),
            group_box(
                column![
                    field_row(
                        "Auth Type:",
                        pick_list(
                            &AuthType::ALL[..],
                            Some(self.upstream_auth_type),
                            Message::UpstreamAuthTypeChanged
                        )
                    ),
                    field_row(
                        "Username:",
                        text_input("Username", &self.upstream_username)
                            .on_input(Message::UpstreamUsernameChanged)
                    ),
                    field_row(
                        "Password:",
                        text_input("Password", &self.upstream_password)
                            .on_input(Message::UpstreamPasswordChanged)
                            .secure(true)
                    ),
                    field_row(
                        "Domain:",
                        text_input("Domain (NTLM)", &self.upstream_domain)
                            .on_input(Message::UpstreamDomainChanged)
                    ),
                    field_row(
                        "Workstation:",
                        text_input("Workstation (NTLM)", &self.upstream_workstation)
                            .on_input(Message::UpstreamWorkstationChanged)
                    ),
                    field_row(
                        "Proxy URL:",
                        text_input("http://upstream:port", &self.upstream_proxy_url)
                            .on_input(Message::UpstreamProxyUrlChanged)
                    ),
                ]
                .spacing(10)
            )
        ]
        .into()
    }

    fn view_exceptions_config(&self) -> Element<'_, Message> {
        column![
            text("Exceptions").size(24),
            Space::new().height(10),
            group_box(
                column![
                    text("Hosts to bypass proxy (comma separated):").size(14),
                    text_input("localhost, 127.0.0.1", &self.exceptions_hosts)
                        .on_input(Message::ExceptionsHostsChanged)
                ]
                .spacing(5)
            )
        ]
        .into()
    }
}

// Helpers
fn sidebar_button(label: &str, tab: Tab, active_tab: Tab) -> Element<'_, Message> {
    let is_active = tab == active_tab;

    button(text(label).width(Length::Fill).align_x(Alignment::Center))
        .width(Length::Fill)
        .padding(10)
        .on_press(Message::TabSelected(tab))
        .style(if is_active {
            iced::widget::button::primary
        } else {
            iced::widget::button::secondary
        })
        .into()
}

fn group_box<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).padding(15).style(rounded_box).into()
}

fn field_row<'a>(label: &'a str, input: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![text(label).size(14), input.into()]
        .spacing(5)
        .into()
}

fn rounded_box(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.into()),
        border: iced::Border {
            radius: 10.0.into(),
            width: 1.0,
            color: palette.background.strong.color,
        },
        ..Default::default()
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
