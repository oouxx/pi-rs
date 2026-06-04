pub fn is_truthy_env_flag(value: Option<&str>) -> bool {
    match value {
        Some(v) => {
            let lower = v.trim().to_lowercase();
            lower == "1" || lower == "true" || lower == "yes"
        }
        None => false,
    }
}

pub trait HasTelemetrySetting {
    fn get_enable_install_telemetry(&self) -> bool;
}

pub fn is_install_telemetry_enabled(
    settings_manager: &impl HasTelemetrySetting,
    telemetry_env: Option<&str>,
) -> bool {
    match telemetry_env {
        Some(val) => is_truthy_env_flag(Some(val)),
        None => settings_manager.get_enable_install_telemetry(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSettings {
        telemetry: bool,
    }

    impl HasTelemetrySetting for MockSettings {
        fn get_enable_install_telemetry(&self) -> bool {
            self.telemetry
        }
    }

    #[test]
    fn test_truthy_values() {
        assert!(is_truthy_env_flag(Some("1")));
        assert!(is_truthy_env_flag(Some("true")));
        assert!(is_truthy_env_flag(Some("TRUE")));
        assert!(is_truthy_env_flag(Some("yes")));
        assert!(is_truthy_env_flag(Some("YES")));
    }

    #[test]
    fn test_falsy_values() {
        assert!(!is_truthy_env_flag(Some("0")));
        assert!(!is_truthy_env_flag(Some("false")));
        assert!(!is_truthy_env_flag(Some("no")));
        assert!(!is_truthy_env_flag(None));
    }

    #[test]
    fn test_env_var_overrides_settings() {
        let settings = MockSettings { telemetry: false };
        assert!(is_install_telemetry_enabled(&settings, Some("1")));
        assert!(!is_install_telemetry_enabled(&settings, Some("0")));
    }

    #[test]
    fn test_settings_used_when_no_env() {
        let enabled = MockSettings { telemetry: true };
        let disabled = MockSettings { telemetry: false };
        assert!(is_install_telemetry_enabled(&enabled, None));
        assert!(!is_install_telemetry_enabled(&disabled, None));
    }
}
