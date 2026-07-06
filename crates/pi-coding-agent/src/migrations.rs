//! One-time data migrations that run on startup.
//!
//! Mirrors packages/coding-agent/src/migrations.ts

use std::path::Path;

use crate::config;

/// Run all pending migrations using the default agent directory.
pub fn run_migrations() {
    let _ = migrate_auth_to_auth_json(Some(config::get_agent_dir()));
}

/// Migrate legacy `oauth.json` and `settings.json` apiKeys to `auth.json`.
///
/// Returns names of providers that were migrated.
/// Pass `agent_dir_override` to specify a custom agent dir (for tests).
pub fn migrate_auth_to_auth_json(agent_dir_override: Option<std::path::PathBuf>) -> Vec<String> {
    let agent_dir = agent_dir_override.unwrap_or_else(config::get_agent_dir);
    let auth_path = agent_dir.join("auth.json");
    let oauth_path = agent_dir.join("oauth.json");
    let settings_path = agent_dir.join("settings.json");

    // Skip if auth.json already exists
    if auth_path.exists() {
        return Vec::new();
    }

    let mut migrated: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut providers: Vec<String> = Vec::new();

    // Migrate oauth.json
    if oauth_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&oauth_path) {
            if let Ok(oauth) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(obj) = oauth.as_object() {
                    for (provider, cred) in obj {
                        let entry = serde_json::json!({
                            "type": "oauth",
                            "credential": cred
                        });
                        migrated.insert(provider.clone(), entry);
                        providers.push(provider.clone());
                    }
                }
            }
        }
        // Rename oauth.json → oauth.json.migrated
        let _ = std::fs::rename(&oauth_path, oauth_path.with_extension("json.migrated"));
    }

    // Migrate settings.json apiKeys
    if settings_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&settings_path) {
            if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(api_keys) = settings.get("apiKeys").and_then(|v| v.as_object()) {
                    for (provider, key) in api_keys {
                        if !migrated.contains_key(provider) {
                            if let Some(key_str) = key.as_str() {
                                let entry = serde_json::json!({
                                    "type": "api_key",
                                    "key": key_str
                                });
                                migrated.insert(provider.clone(), entry);
                                providers.push(provider.clone());
                            }
                        }
                    }
                    // Remove apiKeys from settings
                    if let Some(obj) = settings.as_object_mut() {
                        obj.remove("apiKeys");
                        let _ = std::fs::write(
                            &settings_path,
                            serde_json::to_string_pretty(&settings).unwrap_or(content),
                        );
                    }
                }
            }
        }
    }

    // Write migrated auth data
    if !migrated.is_empty() {
        if let Some(parent) = auth_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(&migrated).unwrap_or_default();
        let _ = std::fs::write(&auth_path, json);
    }

    providers
}

/// Show deprecation warnings for any deprecated config values.
pub fn show_deprecation_warnings() {
    // Check for deprecated settings
    let settings_path = config::get_settings_path();
    if settings_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&settings_path) {
            if let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) {
                if settings.get("apiKeys").is_some() {
                    crate::utils::deprecation::warn_deprecation(
                        "settings.json apiKeys is deprecated. Use auth.json instead.",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_no_migration_needed() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join(".pi").join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join("auth.json"), "{}").unwrap();

        let providers = migrate_auth_to_auth_json(Some(agent_dir.clone()));
        assert!(providers.is_empty());
    }

    #[test]
    fn test_migrate_oauth() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join(".pi").join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let oauth = serde_json::json!({
            "anthropic": { "access_token": "tok_123" }
        });
        fs::write(agent_dir.join("oauth.json"), serde_json::to_string(&oauth).unwrap()).unwrap();

        let providers = migrate_auth_to_auth_json(Some(agent_dir.clone()));
        assert!(providers.contains(&"anthropic".to_string()));

        let auth_content = fs::read_to_string(agent_dir.join("auth.json")).unwrap();
        assert!(auth_content.contains("anthropic"));
    }

    #[test]
    fn test_deprecation_warning_no_panic() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join(".pi").join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let settings = serde_json::json!({
            "apiKeys": { "test-provider": "key-123" }
        });
        fs::write(agent_dir.join("settings.json"), serde_json::to_string(&settings).unwrap()).unwrap();

        // Should not panic
        show_deprecation_warnings();
    }
}
