use crate::config::{
    load_config, save_config, Config, ExceptionsConfig, ProxyConfig, UpstreamConfig,
};
use iced::widget::{button, column, row, text, text_input};
use iced::{executor, Application, Command, Element, Settings, Theme};

pub fn run_ui(config_path: String) -> iced::Result {
    ConfigEditor::run(Settings::with_flags(config_path))
}

struct ConfigEditor {
    path: String,
    // Form fields
    proxy_port: String,
    pac_file: String,
    upstream_auth_type: String,
    upstream_username: String,
    upstream_password: String,
    upstream_proxy_url: String,
    exceptions_hosts: String,
    // Status message
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    ProxyPortChanged(String),
    PacFileChanged(String),
    UpstreamAuthTypeChanged(String),
    UpstreamUsernameChanged(String),
    UpstreamPasswordChanged(String),
    UpstreamProxyUrlChanged(String),
    ExceptionsHostsChanged(String),
    SavePressed,
}

impl Application for ConfigEditor {
    type Executor = executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = String;

    fn new(path: String) -> (Self, Command<Message>) {
        let config = load_config(&path).unwrap_or_default();

        (
            Self {
                path,
                proxy_port: config.proxy.port.to_string(),
                pac_file: config.proxy.pac_file.unwrap_or_default(),
                upstream_auth_type: config
                    .upstream
                    .as_ref()
                    .map(|u| u.auth_type.clone())
                    .unwrap_or_else(|| "none".to_string()),
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
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Ferrovela Configuration")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::ProxyPortChanged(value) => self.proxy_port = value,
            Message::PacFileChanged(value) => self.pac_file = value,
            Message::UpstreamAuthTypeChanged(value) => self.upstream_auth_type = value,
            Message::UpstreamUsernameChanged(value) => self.upstream_username = value,
            Message::UpstreamPasswordChanged(value) => self.upstream_password = value,
            Message::UpstreamProxyUrlChanged(value) => self.upstream_proxy_url = value,
            Message::ExceptionsHostsChanged(value) => self.exceptions_hosts = value,
            Message::SavePressed => {
                // Construct config
                let port = self.proxy_port.parse().unwrap_or(8080);
                let pac_file = if self.pac_file.trim().is_empty() {
                    None
                } else {
                    Some(self.pac_file.trim().to_string())
                };

                let upstream = if self.upstream_auth_type == "none"
                    && self.upstream_username.is_empty()
                    && self.upstream_proxy_url.is_empty()
                {
                    None
                } else {
                    Some(UpstreamConfig {
                        auth_type: self.upstream_auth_type.clone(),
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

                let config = Config {
                    proxy: ProxyConfig { port, pac_file },
                    upstream,
                    exceptions,
                };

                match save_config(&self.path, &config) {
                    Ok(_) => self.status = "Saved successfully!".to_string(),
                    Err(e) => self.status = format!("Error saving: {}", e),
                }
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let content = column![
            text("Proxy Configuration").size(20),
            row![
                text("Port:"),
                text_input("8080", &self.proxy_port).on_input(Message::ProxyPortChanged)
            ]
            .spacing(10),
            row![
                text("PAC File:"),
                text_input("Path to PAC file", &self.pac_file).on_input(Message::PacFileChanged)
            ]
            .spacing(10),
            text("Upstream Configuration").size(20),
            row![
                text("Auth Type:"),
                text_input("basic/ntlm/none", &self.upstream_auth_type)
                    .on_input(Message::UpstreamAuthTypeChanged)
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
                text("Proxy URL:"),
                text_input("http://upstream:port", &self.upstream_proxy_url)
                    .on_input(Message::UpstreamProxyUrlChanged)
            ]
            .spacing(10),
            text("Exceptions").size(20),
            row![
                text("Hosts (comma separated):"),
                text_input("localhost, 127.0.0.1", &self.exceptions_hosts)
                    .on_input(Message::ExceptionsHostsChanged)
            ]
            .spacing(10),
            button("Save").on_press(Message::SavePressed),
            text(&self.status)
        ]
        .spacing(20)
        .padding(20);

        content.into()
    }
}
