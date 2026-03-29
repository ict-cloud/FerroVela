#[cfg(test)]
mod tests {
    use crate::config;
    use crate::ui::{AuthType, ConfigEditor, Message};

    fn reset_preferences() {
        config::save_config(&config::Config::default()).unwrap();
    }

    #[test]
    fn test_ui_initialization() {
        reset_preferences();
        let (editor, _) = ConfigEditor::new_args();

        // Assert initial state (defaults)
        assert_eq!(editor.proxy_port, config::default_port().to_string());
        assert_eq!(editor.pac_file, "");
        assert_eq!(editor.upstream_auth_type, AuthType::None);
    }

    #[test]
    fn test_ui_updates() {
        reset_preferences();
        let (mut editor, _) = ConfigEditor::new_args();

        // Test Port Change
        let _ = editor.update(Message::ProxyPortChanged("9090".to_string()));
        assert_eq!(editor.proxy_port, "9090");

        // Test Auth Type Change
        let _ = editor.update(Message::UpstreamAuthTypeChanged(AuthType::Basic));
        assert_eq!(editor.upstream_auth_type, AuthType::Basic);
    }

    #[test]
    fn test_save_config() {
        reset_preferences();
        let (mut editor, _) = ConfigEditor::new_args();

        // Update some values
        let _ = editor.update(Message::ProxyPortChanged("1234".to_string()));
        let _ = editor.update(Message::UpstreamUsernameChanged("testuser".to_string()));

        // Verify config was saved to CFPreferences
        let loaded = config::load_config();
        assert_eq!(loaded.proxy.port, 1234);
        let upstream = loaded.upstream.unwrap();
        assert_eq!(upstream.username.as_deref(), Some("testuser"));
        assert!(editor.status.contains("Saved successfully"));

        // Clean up
        reset_preferences();
    }
}
