use iced::widget::{
    button, column, container, pick_list, row, rule, scrollable, text, text_input, toggler, Column,
    Space,
};
use iced::{window, Alignment, Color, Element, Length, Theme};

use super::model::{AuthType, ConfigEditor, Message, ServiceStatus, Tab};

impl ConfigEditor {
    pub fn view(&self, window_id: window::Id) -> Element<'_, Message> {
        if Some(window_id) == self.log_window_id {
            return self.view_logs();
        }

        let logs_button = button(text("Logs").width(Length::Fill).align_x(Alignment::Center))
            .width(Length::Fill)
            .padding(10)
            .on_press(Message::OpenLogs)
            .style(if self.log_window_id.is_some() {
                iced::widget::button::primary
            } else {
                iced::widget::button::secondary
            });

        let sidebar = column![
            sidebar_button("Proxy", Tab::Proxy, self.active_tab),
            sidebar_button("Upstream", Tab::Upstream, self.active_tab),
            sidebar_button("Exceptions", Tab::Exceptions, self.active_tab),
            sidebar_button("Advanced", Tab::Advanced, self.active_tab),
            Space::new().height(Length::Fill),
            rule::horizontal(1),
            Space::new().height(5),
            logs_button,
        ]
        .spacing(5)
        .padding(10)
        .width(Length::Fixed(150.0))
        .height(Length::Fill);

        let content = match self.active_tab {
            Tab::Proxy => self.view_proxy_config(),
            Tab::Upstream => self.view_upstream_config(),
            Tab::Exceptions => self.view_exceptions_config(),
            Tab::Advanced => self.view_advanced_config(),
        };

        let (dot_color, status_label) = match self.service_status {
            ServiceStatus::Running => (Color::from_rgb(0.13, 0.62, 0.13), "Running"),
            ServiceStatus::Stopped => (Color::from_rgb(0.55, 0.55, 0.55), "Stopped"),
        };

        let service_control = row![
            text("●").color(dot_color),
            text(status_label),
            Space::new().width(10),
            toggler(self.service_status == ServiceStatus::Running)
                .on_toggle(Message::ToggleService)
                .width(Length::Shrink),
        ]
        .spacing(5)
        .align_y(Alignment::Center);

        let mut main_col = Column::new().spacing(10);
        main_col = main_col.push(service_control);

        if !self.status.is_empty() {
            let color = if self.status_is_error {
                Color::from_rgb(0.75, 0.1, 0.1)
            } else {
                Color::from_rgb(0.1, 0.55, 0.1)
            };
            main_col = main_col.push(text(&self.status).color(color).size(13));
        }

        if self.restart_needed && self.service_status == ServiceStatus::Running {
            let banner = container(
                row![
                    text("Settings changed — restart required to apply.").size(13),
                    Space::new().width(Length::Fill),
                    button("Restart Now")
                        .on_press(Message::RestartService)
                        .style(iced::widget::button::danger),
                ]
                .align_y(Alignment::Center)
                .spacing(10),
            )
            .padding(10)
            .style(warning_box)
            .width(Length::Fill);
            main_col = main_col.push(banner);
        }

        main_col = main_col.push(Space::new().height(10));
        main_col = main_col.push(content);

        row![
            sidebar,
            container(main_col)
                .width(Length::Fill)
                .padding(20)
                .style(rounded_box)
        ]
        .into()
    }

    fn view_logs(&self) -> Element<'_, Message> {
        let search_bar = row![
            text("Search:").size(13),
            text_input("Filter log output…", &self.log_search)
                .on_input(Message::LogSearchChanged)
                .size(13),
        ]
        .spacing(8)
        .align_y(Alignment::Center);

        let is_dark = matches!(self.appearance, Theme::Dark);
        let query = self.log_search.to_lowercase();

        let mut lines_col = Column::new().spacing(1).padding([0, 4]);
        for line in self.log_content.lines() {
            if !query.is_empty() && !line.to_lowercase().contains(&query) {
                continue;
            }
            let color = log_line_color(line, is_dark);
            let mut t = text(line)
                .font(iced::font::Font::MONOSPACE)
                .size(12)
                .width(Length::Fill);
            if let Some(c) = color {
                t = t.color(c);
            }
            lines_col = lines_col.push(t);
        }

        let log_scroll = scrollable(lines_col)
            .id(iced::widget::Id::new("ferrovela_log_scroll"))
            .height(Length::Fill);

        column![search_bar, rule::horizontal(1), log_scroll]
            .spacing(8)
            .padding(10)
            .into()
    }

    fn view_proxy_config(&self) -> Element<'_, Message> {
        column![
            text("Proxy Settings").size(24),
            Space::new().height(10),
            group_box(
                column![
                    validated_field_row(
                        "Port:",
                        text_input("3128", &self.proxy_port).on_input(Message::ProxyPortChanged),
                        self.proxy_port_error.as_deref()
                    ),
                    validated_field_row(
                        "PAC File:",
                        text_input("Path or URL to PAC file", &self.pac_file)
                            .on_input(Message::PacFileChanged),
                        self.pac_file_error.as_deref()
                    )
                ]
                .spacing(10)
            )
        ]
        .spacing(10)
        .into()
    }

    fn view_upstream_config(&self) -> Element<'_, Message> {
        let mut fields = Column::new().spacing(10);

        fields = fields.push(field_row(
            "Auth Type:",
            pick_list(
                &AuthType::ALL[..],
                Some(self.upstream_auth_type),
                Message::UpstreamAuthTypeChanged,
            ),
        ));

        match self.upstream_auth_type {
            AuthType::None => {}
            AuthType::Kerberos => {
                fields = fields.push(field_row(
                    "Username (Kerberos principal):",
                    text_input("username@REALM", &self.upstream_username)
                        .on_input(Message::UpstreamUsernameChanged),
                ));
                fields = fields.push(validated_field_row(
                    "Proxy URL:",
                    text_input("http://upstream:port", &self.upstream_proxy_url)
                        .on_input(Message::UpstreamProxyUrlChanged),
                    self.upstream_proxy_url_error.as_deref(),
                ));
            }
            AuthType::Basic | AuthType::Ntlm => {
                fields = fields.push(field_row(
                    "Username:",
                    text_input("Username", &self.upstream_username)
                        .on_input(Message::UpstreamUsernameChanged),
                ));
                fields = fields.push(field_row(
                    "Password:",
                    text_input("Password", &self.upstream_password)
                        .on_input(Message::UpstreamPasswordChanged)
                        .secure(true),
                ));
                fields = fields.push(field_row(
                    "Store password in system keyring:",
                    iced::widget::Checkbox::new(self.upstream_use_keyring)
                        .on_toggle(Message::UpstreamUseKeyringToggled),
                ));
                if self.upstream_auth_type == AuthType::Ntlm {
                    fields = fields.push(field_row(
                        "Domain:",
                        text_input("Domain", &self.upstream_domain)
                            .on_input(Message::UpstreamDomainChanged),
                    ));
                    fields = fields.push(field_row(
                        "Workstation:",
                        text_input("Workstation", &self.upstream_workstation)
                            .on_input(Message::UpstreamWorkstationChanged),
                    ));
                }
                fields = fields.push(validated_field_row(
                    "Proxy URL:",
                    text_input("http://upstream:port", &self.upstream_proxy_url)
                        .on_input(Message::UpstreamProxyUrlChanged),
                    self.upstream_proxy_url_error.as_deref(),
                ));
            }
        }

        column![
            text("Upstream Settings").size(24),
            Space::new().height(10),
            group_box(fields)
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

    fn view_advanced_config(&self) -> Element<'_, Message> {
        let (lock_icon, lock_label) = if self.advanced_unlocked {
            ("🔓", "Click to lock")
        } else {
            ("🔒", "Click the lock to make changes")
        };

        let lock_row = button(
            row![
                text(lock_icon),
                text(lock_label).size(13),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .on_press(Message::AdvancedUnlockRequested)
        .style(iced::widget::button::secondary);

        // Allow-private-IPs checkbox — editable only when unlocked.
        let mut allow_private_ips_checkbox =
            iced::widget::Checkbox::new(self.allow_private_ips);
        if self.advanced_unlocked {
            allow_private_ips_checkbox =
                allow_private_ips_checkbox.on_toggle(Message::AllowPrivateIpsToggled);
        }

        // Listen IP text input — editable only when unlocked AND allow_private_ips is on.
        let listen_ip_editable = self.advanced_unlocked && self.allow_private_ips;
        let mut listen_ip_input = text_input("127.0.0.1", &self.proxy_listen_ip);
        if listen_ip_editable {
            listen_ip_input = listen_ip_input.on_input(Message::ProxyListenIpChanged);
        }

        let mut fields = column![field_row(
            "Allow private IPs (bypass SSRF guard):",
            allow_private_ips_checkbox
        )]
        .spacing(10);

        fields = fields.push(validated_field_row(
            "Listen IP:",
            listen_ip_input,
            self.proxy_listen_ip_error.as_deref(),
        ));

        // Show a hint explaining why the Listen IP field is locked when
        // allow_private_ips is off but the tab is unlocked.
        if self.advanced_unlocked && !self.allow_private_ips {
            fields = fields.push(
                text("Listen IP is ignored unless 'Allow private IPs' is enabled. The proxy will bind to 127.0.0.1.")
                    .size(12)
                    .color(Color::from_rgb(0.55, 0.55, 0.55)),
            );
        }

        let warning = container(
            text(
                "Warning: enabling 'Allow private IPs' relaxes the SSRF guard. \
                 Setting a non-loopback Listen IP exposes the proxy to the network.",
            )
            .size(13),
        )
        .padding(10)
        .style(warning_box)
        .width(Length::Fill);

        column![
            text("Advanced Settings").size(24),
            Space::new().height(10),
            lock_row,
            Space::new().height(10),
            group_box(fields),
            Space::new().height(10),
            warning,
        ]
        .spacing(10)
        .into()
    }
}

// ---------------------------------------------------------------------------
// Widget helpers
// ---------------------------------------------------------------------------

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

fn validated_field_row<'a>(
    label: &'a str,
    input: impl Into<Element<'a, Message>>,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    let mut col = column![text(label).size(14), input.into()].spacing(5);
    if let Some(err) = error {
        col = col.push(text(err).color(Color::from_rgb(0.75, 0.1, 0.1)).size(12));
    }
    col.into()
}

/// Returns a coloured foreground for a log line based on its level keyword,
/// or `None` to let the line inherit the default theme text colour.
fn log_line_color(line: &str, is_dark: bool) -> Option<Color> {
    if line.contains("ERROR") {
        Some(Color::from_rgb(0.85, 0.2, 0.2))
    } else if line.contains("WARN") {
        Some(if is_dark {
            Color::from_rgb(0.95, 0.75, 0.25)
        } else {
            Color::from_rgb(0.70, 0.45, 0.0)
        })
    } else if line.contains("DEBUG") || line.contains("TRACE") {
        Some(Color::from_rgb(0.55, 0.55, 0.55))
    } else {
        None
    }
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

fn warning_box(theme: &Theme) -> container::Style {
    let (bg, border) = if matches!(theme, Theme::Dark) {
        (
            Color::from_rgb(0.30, 0.22, 0.02),
            Color::from_rgb(0.55, 0.40, 0.05),
        )
    } else {
        (
            Color::from_rgb(1.0, 0.95, 0.7),
            Color::from_rgb(0.85, 0.65, 0.1),
        )
    };
    container::Style {
        background: Some(bg.into()),
        border: iced::Border {
            radius: 6.0.into(),
            width: 1.0,
            color: border,
        },
        ..Default::default()
    }
}
