use iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_input, toggler, Space,
};
use iced::{window, Alignment, Element, Length, Theme};

use super::model::{AuthType, ConfigEditor, Message, ServiceStatus, Tab};

impl ConfigEditor {
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

        row![
            sidebar,
            container(column![service_control, Space::new().height(20), content].spacing(10))
                .width(Length::Fill)
                .padding(20)
                .style(rounded_box)
        ]
        .into()
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
                        "Store password in system keyring:",
                        iced::widget::Checkbox::new(self.upstream_use_keyring)
                            .on_toggle(Message::UpstreamUseKeyringToggled)
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
