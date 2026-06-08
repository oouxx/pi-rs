use std::path::PathBuf;

pub const PACKAGE_NAME: &str = "pi-coding-agent";
pub const APP_NAME: &str = "pi";
pub const APP_TITLE: &str = "π";
pub const CONFIG_DIR_NAME: &str = ".pi";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";
pub const ENV_SESSION_DIR: &str = "PI_CODING_AGENT_SESSION_DIR";

pub const CURRENT_SESSION_VERSION: u32 = 3;

pub fn expand_tilde_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

pub fn get_agent_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var(ENV_AGENT_DIR) {
        return expand_tilde_path(&env_dir);
    }
    dirs::home_dir()
        .expect("Could not determine home directory")
        .join(CONFIG_DIR_NAME)
        .join("agent")
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

pub fn get_sessions_dir() -> PathBuf {
    get_agent_dir().join("sessions")
}

pub fn get_debug_log_path() -> PathBuf {
    get_agent_dir().join(format!("{}-debug.log", APP_NAME))
}

pub fn get_custom_themes_dir() -> PathBuf {
    get_agent_dir().join("themes")
}

pub fn get_default_session_dir(cwd: &str, agent_dir: Option<&str>) -> PathBuf {
    let resolved_agent_dir = match agent_dir {
        Some(d) => PathBuf::from(d),
        None => get_agent_dir(),
    };
    let dir = get_default_session_dir_path(cwd, &resolved_agent_dir);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).ok();
    }
    dir
}

fn get_default_session_dir_path(cwd: &str, agent_dir: &std::path::Path) -> PathBuf {
    let resolved_cwd = resolve_path(cwd);
    let safe_path = encode_cwd_to_dir_name(&resolved_cwd);
    agent_dir.join("sessions").join(safe_path)
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
        assert_eq!(CONFIG_DIR_NAME, ".pi");
        assert_eq!(CURRENT_SESSION_VERSION, 3);
    }
}
