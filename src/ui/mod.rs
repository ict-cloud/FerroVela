mod controller;
mod model;
mod view;

#[allow(unused_imports)] // items are part of the public API and used in tests
pub use model::{AuthType, ConfigEditor, IPC_RECEIVER, Message, ServiceStatus, Tab};

use iced::window;

pub fn run_ui(config_path: String) -> iced::Result {
    iced::daemon(
        move || ConfigEditor::new_args(config_path.clone()),
        ConfigEditor::update,
        ConfigEditor::view,
    )
    .theme(|_: &ConfigEditor, _: window::Id| iced::Theme::Light)
    .subscription(ConfigEditor::subscription)
    .run()
}
