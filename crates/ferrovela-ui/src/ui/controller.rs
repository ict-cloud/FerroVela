use iced::{window, Subscription, Task, Theme};
use log::error;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use ferrovela_lib::config::{
    default_port, load_config, save_config, Config, ExceptionsConfig, ProxyConfig, UpstreamConfig,
};
use ferrovela_lib::launchd;

use super::model::{AuthType, ConfigEditor, Message, ServiceStatus, Tab};

impl ConfigEditor {
    pub fn new_args() -> (Self, Task<Message>) {
        let config = load_config();

        let (main_window_id, open_task) = window::open(window::Settings {
            size: (800.0, 600.0).into(),
            min_size: Some((600.0, 450.0).into()),
            ..Default::default()
        });

        let upstream = config.upstream.as_ref();
        let editor = Self {
            active_tab: Tab::Proxy,
            proxy_port: config.proxy.port.to_string(),
            pac_file: config.proxy.pac_file.clone().unwrap_or_default(),
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
            upstream_use_keyring: upstream.map(|u| u.use_keyring).unwrap_or(true),
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
            status_is_error: false,
            status_timestamp: None,
            proxy_port_error: validate_port(&config.proxy.port.to_string()),
            pac_file_error: config
                .proxy
                .pac_file
                .as_deref()
                .and_then(|s| validate_pac_file(s)),
            upstream_proxy_url_error: upstream
                .and_then(|u| u.proxy_url.as_deref())
                .and_then(|s| validate_proxy_url(s)),
            service_status: if launchd::is_running() {
                ServiceStatus::Running
            } else {
                ServiceStatus::Stopped
            },
            restart_needed: false,
            appearance: if system_prefers_dark() {
                Theme::Dark
            } else {
                Theme::Light
            },
            show_logs: false,
            log_content: String::new(),
            log_search: String::new(),
            main_window_id: Some(main_window_id),
            log_window_id: None,
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
            Message::ToggleService(start) => {
                self.handle_toggle_service(start);
                Task::none()
            }
            Message::RestartService => {
                self.handle_restart_service();
                Task::none()
            }
            Message::PollStatus => {
                self.service_status = if launchd::is_running() {
                    ServiceStatus::Running
                } else {
                    ServiceStatus::Stopped
                };
                self.appearance = if system_prefers_dark() {
                    Theme::Dark
                } else {
                    Theme::Light
                };
                if let Some(ts) = self.status_timestamp {
                    if ts.elapsed() >= Duration::from_secs(3) {
                        self.status.clear();
                        self.status_timestamp = None;
                    }
                }
                Task::none()
            }
            Message::LogSearchChanged(v) => {
                self.log_search = v;
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
            Message::ProxyPortChanged(v) => {
                self.proxy_port_error = validate_port(&v);
                self.proxy_port = v;
            }
            Message::PacFileChanged(v) => {
                self.pac_file_error = validate_pac_file(&v);
                self.pac_file = v;
            }
            Message::UpstreamAuthTypeChanged(v) => {
                self.upstream_auth_type = v;
                // Clear proxy URL error when upstream is disabled
                if v == AuthType::None {
                    self.upstream_proxy_url_error = None;
                }
            }
            Message::UpstreamProxyUrlChanged(v) => {
                self.upstream_proxy_url_error = validate_proxy_url(&v);
                self.upstream_proxy_url = v;
            }
            Message::UpstreamUsernameChanged(v) => self.upstream_username = v,
            Message::UpstreamPasswordChanged(v) => self.upstream_password = v,
            Message::UpstreamUseKeyringToggled(v) => self.upstream_use_keyring = v,
            Message::UpstreamDomainChanged(v) => self.upstream_domain = v,
            Message::UpstreamWorkstationChanged(v) => self.upstream_workstation = v,
            Message::ExceptionsHostsChanged(v) => self.exceptions_hosts = v,
            _ => return,
        }

        if self.has_validation_errors() {
            self.status = "Invalid input — settings not saved.".to_string();
            self.status_is_error = true;
            self.status_timestamp = Some(std::time::Instant::now());
        } else {
            self.save_current_config();
        }
    }

    fn has_validation_errors(&self) -> bool {
        self.proxy_port_error.is_some()
            || self.pac_file_error.is_some()
            || self.upstream_proxy_url_error.is_some()
    }

    fn handle_toggle_service(&mut self, start: bool) {
        if start {
            match launchd::start() {
                Ok(()) => {
                    self.service_status = ServiceStatus::Running;
                    self.restart_needed = false;
                    self.status = "Service started.".to_string();
                    self.status_is_error = false;
                }
                Err(e) => {
                    error!("Failed to start service: {}", e);
                    self.status = format!("Failed to start: {e}");
                    self.status_is_error = true;
                }
            }
        } else {
            match launchd::stop() {
                Ok(()) => {
                    self.service_status = ServiceStatus::Stopped;
                    self.restart_needed = false;
                    self.status = "Service stopped.".to_string();
                    self.status_is_error = false;
                }
                Err(e) => {
                    error!("Failed to stop service: {}", e);
                    self.status = format!("Failed to stop: {e}");
                    self.status_is_error = true;
                }
            }
        }
        self.status_timestamp = Some(std::time::Instant::now());
    }

    fn handle_restart_service(&mut self) {
        let _ = launchd::stop();
        match launchd::start() {
            Ok(()) => {
                self.service_status = ServiceStatus::Running;
                self.restart_needed = false;
                self.status = "Service restarted.".to_string();
                self.status_is_error = false;
            }
            Err(e) => {
                error!("Failed to restart service: {}", e);
                self.status = format!("Failed to restart: {e}");
                self.status_is_error = true;
                self.service_status = ServiceStatus::Stopped;
            }
        }
        self.status_timestamp = Some(std::time::Instant::now());
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
                    return iced::widget::operation::snap_to_end(
                        iced::widget::Id::new("ferrovela_log_scroll"),
                    );
                }
            }
            Message::External => {
                if let Some(id) = self.main_window_id {
                    return window::minimize(id, false).chain(window::gain_focus(id));
                } else {
                    let (new_id, open_task) = window::open(window::Settings {
                        size: (800.0, 600.0).into(),
                        min_size: Some((600.0, 450.0).into()),
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
                    let _ = std::fs::remove_file(launchd::ui_socket_path());
                    return iced::exit();
                }
            }
            Message::WindowCloseRequested(id) => {
                if Some(id) == self.log_window_id {
                    self.log_window_id = None;
                    self.show_logs = false;
                }
                return window::close(id);
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
        match save_config(&config) {
            Ok(_) => {
                self.status = "Saved successfully!".to_string();
                self.status_is_error = false;
                if self.service_status == ServiceStatus::Running {
                    self.restart_needed = true;
                }
                self.sync_keyring();
            }
            Err(e) => {
                self.status = format!("Error saving: {}", e);
                self.status_is_error = true;
            }
        }
        self.status_timestamp = Some(std::time::Instant::now());
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
                        log::info!("Saved password to keyring");
                    }
                }
                Err(e) => error!("Failed to create keyring entry: {}", e),
            }
        } else if let Ok(entry) = keyring::Entry::new("ferrovela", username) {
            let _ = entry.delete_credential();
        }
    }

    fn open_log_window(&mut self, main_pos: Option<iced::Point>) -> Task<Message> {
        let position = main_pos
            .map(|p| window::Position::Specific(iced::Point::new(p.x + 40.0, p.y + 40.0)))
            .unwrap_or(window::Position::Default);

        let (id, open_task) = window::open(window::Settings {
            size: (800.0, 600.0).into(),
            min_size: Some((500.0, 350.0).into()),
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
        let log_path = launchd::log_path();
        if let Ok(mut file) = std::fs::File::open(&log_path) {
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

        let status_poll = iced::time::every(Duration::from_secs(3)).map(|_| Message::PollStatus);

        let ipc = Subscription::run(ui_show_stream);

        let events = iced::event::listen_with(|event, _status, id| match event {
            iced::Event::Window(window::Event::CloseRequested) => {
                Some(Message::WindowCloseRequested(id))
            }
            iced::Event::Window(window::Event::Closed) => Some(Message::WindowClosed(id)),
            iced::Event::Window(_) => Some(Message::IdCaptured(id)),
            _ => None,
        });

        Subscription::batch(vec![tick, status_poll, ipc, events])
    }
}

// ---------------------------------------------------------------------------
// Unix socket IPC stream
// ---------------------------------------------------------------------------

fn ui_show_stream() -> impl iced::futures::Stream<Item = Message> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::net::UnixListener;

    iced::futures::stream::unfold(Option::<UnixListener>::None, |state| async move {
        let listener = match state {
            Some(l) => l,
            None => {
                let path = launchd::ui_socket_path();
                let _ = std::fs::remove_file(&path);
                match UnixListener::bind(&path) {
                    Ok(l) => {
                        // Restrict the socket to owner read/write only.
                        // On macOS, AF_UNIX socket permissions ARE enforced by the
                        // kernel — 0600 prevents other users from connecting at all.
                        if let Err(e) =
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                        {
                            log::warn!("could not set socket permissions: {e}");
                        }
                        l
                    }
                    Err(e) => {
                        log::error!("failed to bind UI socket: {e}");
                        std::future::pending::<()>().await;
                        return None;
                    }
                }
            }
        };

        // Accept loop: skip connections from other users rather than broadcasting
        // the show-window signal to any local process that can reach the socket.
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    if peer_is_owner(&stream) {
                        return Some((Message::External, Some(listener)));
                    }
                    log::warn!("rejected IPC connection from unexpected peer UID");
                    // Drop `stream` and wait for the next connection.
                }
                Err(e) => {
                    log::error!("UI socket accept error: {e}");
                    return None;
                }
            }
        }
    })
}

/// Returns `true` when the peer on `stream` has the same effective UID as the
/// current process.  Uses `getpeereid(2)`, which is available on macOS/BSD.
fn peer_is_owner(stream: &tokio::net::UnixStream) -> bool {
    use std::os::unix::io::AsRawFd;
    let fd = stream.as_raw_fd();
    let mut peer_uid: libc::uid_t = libc::uid_t::MAX;
    let mut peer_gid: libc::gid_t = libc::gid_t::MAX;
    // SAFETY: fd is valid for the lifetime of this call; pointers are stack-allocated.
    if unsafe { libc::getpeereid(fd, &mut peer_uid, &mut peer_gid) } != 0 {
        log::debug!("getpeereid failed: {}", std::io::Error::last_os_error());
        return false;
    }
    // SAFETY: geteuid() is always safe to call.
    let own_uid = unsafe { libc::geteuid() };
    peer_uid == own_uid
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Returns `true` when macOS is currently in Dark appearance.
fn system_prefers_dark() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "Dark")
        .unwrap_or(false)
}

fn validate_port(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return Some("Required".to_string());
    }
    match s.parse::<u32>() {
        Ok(n) if (1..=65535).contains(&n) => None,
        Ok(_) => Some("Must be between 1 and 65535".to_string()),
        Err(_) => Some("Must be a number".to_string()),
    }
}

fn validate_pac_file(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return None;
    }
    if !std::path::Path::new(s).exists() {
        return Some("File not found".to_string());
    }
    None
}

fn validate_proxy_url(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let host_part = if let Some(rest) = s.strip_prefix("http://") {
        rest
    } else if let Some(rest) = s.strip_prefix("https://") {
        rest
    } else {
        return Some("Must start with http:// or https://".to_string());
    };
    if host_part.is_empty() || host_part.starts_with('/') {
        return Some("Enter a valid URL, e.g. http://proxy.corp.com:8080".to_string());
    }
    None
}

fn non_empty_trimmed(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
