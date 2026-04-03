mod controller;
mod model;
mod view;

#[allow(unused_imports)] // items are part of the public API and used in tests
pub use model::{AuthType, ConfigEditor, Message, ServiceStatus, Tab};

use iced::window;

pub fn run_ui() -> iced::Result {
    iced::daemon(
        ConfigEditor::new_args,
        ConfigEditor::update,
        ConfigEditor::view,
    )
    .title(|app: &ConfigEditor, id: window::Id| {
        if Some(id) == app.log_window_id {
            "FerroVela – Logs".to_string()
        } else {
            "FerroVela".to_string()
        }
    })
    .theme(|_: &ConfigEditor, _: window::Id| iced::Theme::Light)
    .subscription(ConfigEditor::subscription)
    .run()
}
