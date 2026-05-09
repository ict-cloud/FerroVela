#[cfg(test)]
mod tests {
    use crate::ui::{AuthType, ConfigEditor, Message};
    use ferrovela_lib::config;

    fn reset_preferences() {
        config::save_config(&config::Config::default()).unwrap();
    }

    #[test]
    fn test_ui_initialization() {
        let _lock = config::PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let (editor, _) = ConfigEditor::new_args();

        // Assert initial state (defaults)
        assert_eq!(editor.proxy_port, config::default_port().to_string());
        assert_eq!(editor.pac_file, "");
        assert_eq!(editor.upstream_auth_type, AuthType::None);
    }

    #[test]
    fn test_ui_updates() {
        let _lock = config::PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let (mut editor, _) = ConfigEditor::new_args();

        // Test Port Change
        let _ = editor.update(Message::ProxyPortChanged("9090".to_string()));
        assert_eq!(editor.proxy_port, "9090");

        // Test Auth Type Change
        let _ = editor.update(Message::UpstreamAuthTypeChanged(AuthType::Basic));
        assert_eq!(editor.upstream_auth_type, AuthType::Basic);

        // Clean up
        reset_preferences();
    }

    #[test]
    fn test_save_config() {
        let _lock = config::PREFS_LOCK.lock().unwrap();
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

    #[test]
    fn test_advanced_fields_locked_by_default() {
        let _lock = config::PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let (editor, _) = ConfigEditor::new_args();

        assert!(!editor.advanced_unlocked);
        assert!(!editor.allow_private_ips);
        assert_eq!(editor.proxy_listen_ip, config::default_listen_ip());
    }

    #[test]
    fn test_advanced_unlock_result() {
        let _lock = config::PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let (mut editor, _) = ConfigEditor::new_args();

        // Injecting AdvancedUnlockResult(true) unlocks the tab.
        let _ = editor.update(Message::AdvancedUnlockResult(true));
        assert!(editor.advanced_unlocked);

        // Injecting AdvancedUnlockResult(false) locks it and shows a message.
        let _ = editor.update(Message::AdvancedUnlockResult(false));
        assert!(!editor.advanced_unlocked);
        assert!(editor.status.contains("Unlock cancelled"));
    }

    #[test]
    fn test_advanced_relocks_on_tab_leave() {
        use crate::ui::Tab;
        let _lock = config::PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let (mut editor, _) = ConfigEditor::new_args();

        let _ = editor.update(Message::TabSelected(Tab::Advanced));
        let _ = editor.update(Message::AdvancedUnlockResult(true));
        assert!(editor.advanced_unlocked);

        // Leaving the Advanced tab must relock.
        let _ = editor.update(Message::TabSelected(Tab::Proxy));
        assert!(!editor.advanced_unlocked);
    }

    #[test]
    fn test_listen_ip_validation() {
        let _lock = config::PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let (mut editor, _) = ConfigEditor::new_args();

        // Unlock and enable allow_private_ips so the field becomes editable.
        let _ = editor.update(Message::AdvancedUnlockResult(true));
        let _ = editor.update(Message::AllowPrivateIpsToggled(true));

        // Invalid IP
        let _ = editor.update(Message::ProxyListenIpChanged("not-an-ip".to_string()));
        assert!(editor.proxy_listen_ip_error.is_some());

        // Valid IP
        let _ = editor.update(Message::ProxyListenIpChanged("0.0.0.0".to_string()));
        assert!(editor.proxy_listen_ip_error.is_none());

        reset_preferences();
    }

    #[test]
    fn test_listen_ip_round_trip() {
        let _lock = config::PREFS_LOCK.lock().unwrap();
        reset_preferences();
        let (mut editor, _) = ConfigEditor::new_args();

        // Unlock and enable allow_private_ips, then set a custom listen IP.
        let _ = editor.update(Message::AdvancedUnlockResult(true));
        let _ = editor.update(Message::AllowPrivateIpsToggled(true));
        let _ = editor.update(Message::ProxyListenIpChanged("0.0.0.0".to_string()));

        // Verify it persisted to CFPreferences.
        let loaded = config::load_config();
        assert_eq!(loaded.proxy.listen_ip, "0.0.0.0");
        assert!(loaded.proxy.allow_private_ips);

        reset_preferences();
    }
}
