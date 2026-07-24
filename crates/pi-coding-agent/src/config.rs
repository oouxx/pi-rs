use std::path::PathBuf;

pub const PACKAGE_NAME: &str = "pi-coding-agent";
pub const APP_NAME: &str = "pi";
pub const APP_TITLE: &str = "π";
pub const CONFIG_DIR_NAME: &str = ".pi-rs";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";
pub const ENV_SESSION_DIR: &str = "PI_CODING_AGENT_SESSION_DIR";
pub const ENV_PI_RS_HOME: &str = "PI_RS_HOME";

pub const CURRENT_SESSION_VERSION: u32 = 3;

pub fn expand_tilde_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// Returns the base `.pi-rs` config directory.
///
/// Priority:
/// 1. `$PI_RS_HOME` environment variable (supports relative paths, resolved against cwd)
/// 2. `~/.pi-rs`
pub fn get_pi_rs_home() -> PathBuf {
    if let Ok(env_dir) = std::env::var(ENV_PI_RS_HOME) {
        let expanded = expand_tilde_path(&env_dir);
        if expanded.is_relative() {
            if let Ok(cwd) = std::env::current_dir() {
                return cwd.join(expanded);
            }
        }
        return expanded;
    }
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(CONFIG_DIR_NAME)
}

/// Returns the agent configuration directory.
///
/// Priority:
/// 1. `$PI_CODING_AGENT_DIR` environment variable
/// 2. `$PI_RS_HOME/agent` (or `~/.pi-rs/agent` if `PI_RS_HOME` is unset)
pub fn get_agent_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var(ENV_AGENT_DIR) {
        let expanded = expand_tilde_path(&env_dir);
        if expanded.is_relative() {
            if let Ok(cwd) = std::env::current_dir() {
                return cwd.join(expanded);
            }
        }
        return expanded;
    }
    get_pi_rs_home().join("agent")
}

pub fn get_models_path() -> PathBuf {
    get_agent_dir().join("models.json")
}

pub fn get_auth_path() -> PathBuf {
    get_agent_dir().join("auth.json")
}

pub fn get_settings_path() -> PathBuf {
    get_agent_dir().join("settings.json")
}

pub fn get_tools_dir() -> PathBuf {
    get_agent_dir().join("tools")
}

pub fn get_bin_dir() -> PathBuf {
    get_agent_dir().join("bin")
}

pub fn get_prompts_dir() -> PathBuf {
    get_agent_dir().join("prompts")
}

/// Returns the sessions directory.
///
/// Priority:
/// 1. `$PI_CODING_AGENT_SESSION_DIR` environment variable
/// 2. `{agent_dir}/sessions` (derived from `get_agent_dir()`)
pub fn get_sessions_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var(ENV_SESSION_DIR) {
        let expanded = expand_tilde_path(&env_dir);
        if expanded.is_relative() {
            if let Ok(cwd) = std::env::current_dir() {
                return cwd.join(expanded);
            }
        }
        return expanded;
    }
    get_agent_dir().join("sessions")
}

pub fn get_debug_log_path() -> PathBuf {
    get_agent_dir().join(format!("{}-debug.log", APP_NAME))
}

pub fn get_custom_themes_dir() -> PathBuf {
    get_agent_dir().join("themes")
}

/// Returns the default session directory for the given `cwd`.
///
/// The path is `{sessions_dir}/--encoded-cwd--`.
/// Sessions dir resolution follows `get_sessions_dir()` priority.
pub fn get_default_session_dir(cwd: &str, _agent_dir: Option<&str>) -> PathBuf {
    let sessions_base = get_sessions_dir();
    let safe_path = encode_cwd_to_dir_name(&resolve_path(cwd));
    let dir = sessions_base.join(safe_path);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).ok();
    }
    dir
}

fn encode_cwd_to_dir_name(cwd: &str) -> String {
    let trimmed = cwd.trim_start_matches('/').trim_start_matches('\\');
    let safe = trimmed.replace(['/', '\\', ':'], "-");
    format!("--{}--", safe)
}

pub fn resolve_path(path: &str) -> String {
    let p = expand_tilde_path(path);
    match std::fs::canonicalize(&p) {
        Ok(canonical) => canonical.to_string_lossy().to_string(),
        Err(_) => p.to_string_lossy().to_string(),
    }
}

pub fn normalize_path(path: &str) -> String {
    expand_tilde_path(path).to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_cwd_to_dir_name() {
        assert_eq!(
            encode_cwd_to_dir_name("/Users/test/projects/my-app"),
            "--Users-test-projects-my-app--"
        );
    }

    #[test]
    fn test_expand_tilde_path() {
        let result = expand_tilde_path("~/test");
        assert!(result.to_string_lossy().ends_with("/test"));
        assert!(!result.to_string_lossy().starts_with("~"));

        let result = expand_tilde_path("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_constants() {
        assert_eq!(APP_NAME, "pi");
        assert_eq!(CONFIG_DIR_NAME, ".pi-rs");
        assert_eq!(CURRENT_SESSION_VERSION, 3);
    }

    /// All env-var-dependent tests in one serial function to avoid races
    /// on global `std::env` between parallel test threads.
    #[test]
    fn test_env_var_config() {
        // --- get_pi_rs_home: default (no env var) ---
        std::env::remove_var("PI_RS_HOME");
        std::env::remove_var("PI_CODING_AGENT_DIR");
        std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
        let home = get_pi_rs_home();
        let expected = dirs::home_dir().unwrap().join(".pi-rs");
        assert_eq!(home, expected);

        // --- get_pi_rs_home: PI_RS_HOME override ---
        std::env::set_var("PI_RS_HOME", "/custom/pi-home");
        let home = get_pi_rs_home();
        assert_eq!(home, PathBuf::from("/custom/pi-home"));

        // --- get_agent_dir: uses PI_RS_HOME ---
        std::env::remove_var("PI_CODING_AGENT_DIR");
        let agent = get_agent_dir();
        assert_eq!(agent, PathBuf::from("/custom/pi-home/agent"));

        // --- get_agent_dir: PI_CODING_AGENT_DIR takes precedence ---
        std::env::set_var("PI_CODING_AGENT_DIR", "/direct/agent");
        let agent = get_agent_dir();
        assert_eq!(agent, PathBuf::from("/direct/agent"));

        // --- get_sessions_dir: PI_CODING_AGENT_SESSION_DIR override ---
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/custom/sessions");
        let sessions = get_sessions_dir();
        assert_eq!(sessions, PathBuf::from("/custom/sessions"));

        // --- get_sessions_dir: default (under agent dir) ---
        std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
        std::env::set_var("PI_RS_HOME", "/tmp/test-pi-home");
        std::env::remove_var("PI_CODING_AGENT_DIR");
        let sessions = get_sessions_dir();
        assert_eq!(sessions, PathBuf::from("/tmp/test-pi-home/agent/sessions"));

        // Cleanup
        std::env::remove_var("PI_RS_HOME");
        std::env::remove_var("PI_CODING_AGENT_DIR");
        std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
    }
}
