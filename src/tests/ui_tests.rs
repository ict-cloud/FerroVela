#[cfg(test)]
mod tests {
    use crate::ui::{AuthType, ConfigEditor, Message};
    use std::fs;
    use tempfile::NamedTempFile;

    #[test]
    fn test_ui_initialization() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap().to_string();

        let (editor, _) = ConfigEditor::new_args(path.clone());

        // Assert initial state (defaults)
        assert_eq!(editor.proxy_port, "3128"); // Default port
        assert_eq!(editor.pac_file, "");
        assert_eq!(editor.upstream_auth_type, AuthType::None);
    }

    #[test]
    fn test_ui_updates() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap().to_string();
        let (mut editor, _) = ConfigEditor::new_args(path.clone());

        // Test Port Change
        let _ = editor.update(Message::ProxyPortChanged("9090".to_string()));
        assert_eq!(editor.proxy_port, "9090");

        // Test Auth Type Change
        let _ = editor.update(Message::UpstreamAuthTypeChanged(AuthType::Basic));
        assert_eq!(editor.upstream_auth_type, AuthType::Basic);
    }

    #[test]
    fn test_save_config() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_str().unwrap().to_string();
        let (mut editor, _) = ConfigEditor::new_args(path.clone());

        // Update some values
        let _ = editor.update(Message::ProxyPortChanged("1234".to_string()));
        let _ = editor.update(Message::UpstreamUsernameChanged("testuser".to_string()));

        // Verify file content
        let content = fs::read_to_string(&path).expect("Failed to read config file");
        assert!(content.contains("port = 1234"));
        assert!(content.contains("username = \"testuser\""));
        assert!(editor.status.contains("Saved successfully"));
    }
}
