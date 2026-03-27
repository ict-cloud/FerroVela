use iced::{window, Subscription, Task};
use log::{error, info};
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::config::{
    default_port, load_config, save_config, Config, ExceptionsConfig, ProxyConfig, UpstreamConfig,
};
use crate::pac::PacEngine;
use crate::proxy::{Proxy, ProxySignal};

use super::model::{AuthType, ConfigEditor, Message, ServiceStatus, Tab, IPC_RECEIVER};

impl ConfigEditor {
    pub fn new_args(path: String) -> (Self, Task<Message>) {
        let config = load_config(&path).unwrap_or_default();

        let (tx, rx) = mpsc::channel(32);
        let _ = IPC_RECEIVER.set(tokio::sync::Mutex::new(Some(rx)));

        let (main_window_id, open_task) = window::open(window::Settings {
            size: (800.0, 600.0).into(),
            ..Default::default()
        });

        let upstream = config.upstream.as_ref();
        let editor = Self {
            path,
            active_tab: Tab::Proxy,
            proxy_port: config.proxy.port.to_string(),
            pac_file: config.proxy.pac_file.unwrap_or_default(),
            allow_private_ips: config.proxy.allow_private_ips,
            upstream_auth_type: upstream
                .map(|u| AuthType::from(u.auth_type.as_str()))
                .unwrap_or_default(),
            upstream_username: upstream
                .and_then(|u| u.username.clone())
                .unwrap_or_default(),
            upstream_password: upstream
                .and_then(|u| u.password.clone())
                .unwrap_or_default(),
            upstream_use_keyring: upstream.map(|u| u.use_keyring).unwrap_or(false),
            upstream_domain: upstream.and_then(|u| u.domain.clone()).unwrap_or_default(),
            upstream_workstation: upstream
                .and_then(|u| u.workstation.clone())
                .unwrap_or_default(),
            upstream_proxy_url: upstream
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
            main_window_id: Some(main_window_id),
            log_window_id: None,
            signal_sender: tx,
        };

        (editor, open_task.map(Message::IdCaptured))
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabSelected(tab) => {
                self.active_tab = tab;
                Task::none()
            }
            Message::ProxyPortChanged(_)
            | Message::PacFileChanged(_)
            | Message::UpstreamAuthTypeChanged(_)
            | Message::UpstreamUsernameChanged(_)
            | Message::UpstreamPasswordChanged(_)
            | Message::UpstreamUseKeyringToggled(_)
            | Message::UpstreamDomainChanged(_)
            | Message::UpstreamWorkstationChanged(_)
            | Message::UpstreamProxyUrlChanged(_)
            | Message::ExceptionsHostsChanged(_) => {
                self.handle_config_message(message);
                Task::none()
            }
            Message::ToggleService(is_running) => {
                self.handle_toggle_service(is_running);
                Task::none()
            }
            Message::OpenLogs
            | Message::OpenLogsAt(_)
            | Message::Tick
            | Message::External
            | Message::WindowCloseRequested(_)
            | Message::WindowClosed(_)
            | Message::IdCaptured(_) => self.handle_window_message(message),
        }
    }

    fn handle_config_message(&mut self, message: Message) {
        match message {
            Message::ProxyPortChanged(v) => self.proxy_port = v,
            Message::PacFileChanged(v) => self.pac_file = v,
            Message::UpstreamAuthTypeChanged(v) => self.upstream_auth_type = v,
            Message::UpstreamUsernameChanged(v) => self.upstream_username = v,
            Message::UpstreamPasswordChanged(v) => self.upstream_password = v,
            Message::UpstreamUseKeyringToggled(v) => self.upstream_use_keyring = v,
            Message::UpstreamDomainChanged(v) => self.upstream_domain = v,
            Message::UpstreamWorkstationChanged(v) => self.upstream_workstation = v,
            Message::UpstreamProxyUrlChanged(v) => self.upstream_proxy_url = v,
            Message::ExceptionsHostsChanged(v) => self.exceptions_hosts = v,
            _ => return,
        }
        self.save_current_config();
    }

    fn handle_toggle_service(&mut self, is_running: bool) {
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

    fn handle_window_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenLogs => {
                if self.log_window_id.is_none() {
                    if let Some(main_id) = self.main_window_id {
                        return window::position(main_id).map(Message::OpenLogsAt);
                    }
                    return self.open_log_window(None);
                } else if let Some(id) = self.log_window_id {
                    return window::gain_focus(id);
                }
            }
            Message::OpenLogsAt(pos) => {
                if self.log_window_id.is_none() {
                    return self.open_log_window(pos);
                }
            }
            Message::Tick => {
                if self.show_logs || self.log_window_id.is_some() {
                    self.load_logs();
                }
            }
            Message::External => {
                if let Some(id) = self.main_window_id {
                    return window::minimize(id, false).chain(window::gain_focus(id));
                } else {
                    let (new_id, open_task) = window::open(window::Settings {
                        size: (800.0, 600.0).into(),
                        ..Default::default()
                    });
                    self.main_window_id = Some(new_id);
                    return open_task.map(Message::IdCaptured);
                }
            }
            Message::WindowClosed(id) => {
                if Some(id) == self.log_window_id {
                    self.log_window_id = None;
                    self.show_logs = false;
                } else if Some(id) == self.main_window_id {
                    self.main_window_id = None;
                    if self.service_status == ServiceStatus::Stopped {
                        return iced::exit();
                    }
                }
            }
            Message::WindowCloseRequested(id) => {
                if Some(id) == self.log_window_id {
                    self.log_window_id = None;
                    self.show_logs = false;
                    return window::close(id);
                } else if self.service_status == ServiceStatus::Running {
                    return window::minimize(id, true);
                } else {
                    return window::close(id);
                }
            }
            Message::IdCaptured(id) => {
                if self.log_window_id != Some(id) && self.main_window_id != Some(id) {
                    self.main_window_id = Some(id);
                }
            }
            _ => {}
        }
        Task::none()
    }

    // -----------------------------------------------------------------------
    // Config helpers
    // -----------------------------------------------------------------------

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
                username: non_empty_trimmed(&self.upstream_username),
                password: if self.upstream_use_keyring {
                    None
                } else {
                    non_empty_trimmed(&self.upstream_password)
                },
                use_keyring: self.upstream_use_keyring,
                domain: non_empty_trimmed(&self.upstream_domain),
                workstation: non_empty_trimmed(&self.upstream_workstation),
                proxy_url: non_empty_trimmed(&self.upstream_proxy_url),
            })
        };

        let exceptions = if self.exceptions_hosts.trim().is_empty() {
            None
        } else {
            let hosts = self
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
            Ok(_) => {
                self.status = "Saved successfully!".to_string();
                self.sync_keyring();
            }
            Err(e) => self.status = format!("Error saving: {}", e),
        }
    }

    /// Writes or removes the password from the system keyring depending on
    /// whether keyring storage is enabled in the current form state.
    fn sync_keyring(&self) {
        let username = self.upstream_username.trim();
        if username.is_empty() {
            return;
        }

        if self.upstream_use_keyring {
            let password = self.upstream_password.trim();
            if password.is_empty() {
                return;
            }
            match keyring::Entry::new("ferrovela", username) {
                Ok(entry) => {
                    if let Err(e) = entry.set_password(password) {
                        error!("Failed to save password to keyring: {}", e);
                    } else {
                        info!("Saved password to keyring for '{}'", username);
                    }
                }
                Err(_) => error!("Failed to create keyring entry for '{}'", username),
            }
        } else {
            // Keyring disabled — clear any previously stored credential.
            if let Ok(entry) = keyring::Entry::new("ferrovela", username) {
                let _ = entry.delete_credential();
            }
        }
    }

    fn open_log_window(&mut self, main_pos: Option<iced::Point>) -> Task<Message> {
        let position = main_pos
            .map(|p| window::Position::Specific(iced::Point::new(p.x + 40.0, p.y + 40.0)))
            .unwrap_or(window::Position::Default);

        let (id, open_task) = window::open(window::Settings {
            size: (800.0, 600.0).into(),
            position,
            ..Default::default()
        });
        self.log_window_id = Some(id);
        self.log_content = String::new();
        self.load_logs();
        self.show_logs = true;
        open_task.map(|_| Message::Tick)
    }

    pub fn load_logs(&mut self) {
        if let Ok(mut file) = std::fs::File::open("service.log") {
            if let Ok(metadata) = file.metadata() {
                let offset = metadata.len().saturating_sub(10_000);
                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    let mut buffer = String::new();
                    if file.read_to_string(&mut buffer).is_ok() {
                        self.log_content = buffer;
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Subscription
    // -----------------------------------------------------------------------

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
            iced::Event::Window(window::Event::Closed) => Some(Message::WindowClosed(id)),
            iced::Event::Window(_) => Some(Message::IdCaptured(id)),
            _ => None,
        });

        Subscription::batch(vec![tick, ipc, events])
    }
}

// ---------------------------------------------------------------------------
// IPC stream
// ---------------------------------------------------------------------------

fn ipc_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::futures::stream::unfold((), |_| async {
        if let Some(lock) = IPC_RECEIVER.get() {
            let mut guard = lock.lock().await;
            if let Some(rx) = guard.as_mut() {
                if let Some(cmd) = rx.recv().await {
                    match cmd {
                        ProxySignal::Show => return Some((Message::External, ())),
                    }
                }
            }
        }
        // Receiver is missing or the channel was closed — suspend forever.
        std::future::pending::<()>().await;
        None
    })
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Returns `Some(trimmed)` if `s` is non-empty after trimming, else `None`.
fn non_empty_trimmed(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
