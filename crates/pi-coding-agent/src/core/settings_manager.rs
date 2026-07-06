use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    pub enabled: Option<bool>,
    pub reserve_tokens: Option<u64>,
    pub keep_recent_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummarySettings {
    pub reserve_tokens: Option<u64>,
    pub skip_prompt: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRetrySettings {
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RetrySettings {
    pub enabled: Option<bool>,
    pub max_retries: Option<u32>,
    pub base_delay_ms: Option<u64>,
    pub provider: Option<ProviderRetrySettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    pub show_images: Option<bool>,
    pub image_width_cells: Option<u32>,
    pub clear_on_shrink: Option<bool>,
    pub show_terminal_progress: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImageSettings {
    pub auto_resize: Option<bool>,
    pub block_images: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgetsSettings {
    pub minimal: Option<u32>,
    pub low: Option<u32>,
    pub medium: Option<u32>,
    pub high: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownSettings {
    pub code_block_indent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WarningSettings {
    pub anthropic_extra_usage: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub last_changelog_version: Option<String>,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub thinking_level: Option<String>,
    pub custom_system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub compaction: Option<CompactionSettings>,
    pub branch_summary: Option<BranchSummarySettings>,
    pub retry: Option<RetrySettings>,
    pub terminal: Option<TerminalSettings>,
    pub image: Option<ImageSettings>,
    pub thinking_budgets: Option<ThinkingBudgetsSettings>,
    pub markdown: Option<MarkdownSettings>,
    pub warnings: Option<WarningSettings>,
    pub transport: Option<String>,
    pub skills: Option<serde_json::Value>,
    pub enable_skill_commands: Option<bool>,
    pub extensions: Option<Vec<serde_json::Value>>,
    pub steering_mode: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn deep_merge_settings(base: &Settings, overlay: &Settings) -> Settings {
    let mut merged = base.clone();
    let base_json =
        serde_json::to_value(base).unwrap_or(serde_json::Value::Object(Default::default()));
    let overlay_json =
        serde_json::to_value(overlay).unwrap_or(serde_json::Value::Object(Default::default()));

    if let (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) =
        (&base_json, &overlay_json)
    {
        let mut result = base_map.clone();
        for (key, value) in overlay_map {
            if !value.is_null() {
                result.insert(key.clone(), value.clone());
            }
        }
        if let Ok(s) = serde_json::from_value(serde_json::Value::Object(result)) {
            merged = s;
        }
    }

    merged
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsScope {
    Global,
    Project,
}

pub trait SettingsStorage: Send + Sync {
    fn with_lock(
        &self,
        scope: SettingsScope,
        current: Option<String>,
        f: Box<dyn FnOnce(Option<&str>) -> Option<String> + Send>,
    ) -> Option<String>;
}

pub struct FileSettingsStorage {
    global_settings_path: PathBuf,
    project_settings_path: PathBuf,
}

impl FileSettingsStorage {
    pub fn new(cwd: &str, agent_dir: &str) -> Self {
        let resolved_agent_dir = PathBuf::from(agent_dir);
        let resolved_cwd = PathBuf::from(cwd);
        Self {
            global_settings_path: resolved_agent_dir.join("settings.json"),
            project_settings_path: resolved_cwd
                .join(config::CONFIG_DIR_NAME)
                .join("settings.json"),
        }
    }
}

impl SettingsStorage for FileSettingsStorage {
    fn with_lock(
        &self,
        scope: SettingsScope,
        _current: Option<String>,
        f: Box<dyn FnOnce(Option<&str>) -> Option<String> + Send>,
    ) -> Option<String> {
        let path = match scope {
            SettingsScope::Global => &self.global_settings_path,
            SettingsScope::Project => &self.project_settings_path,
        };

        let current = if path.exists() {
            fs::read_to_string(path).ok()
        } else {
            None
        };

        let result = f(current.as_deref());

        if let Some(ref new_content) = result {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, new_content);
        }

        result
    }
}

pub struct InMemorySettingsStorage {
    global: std::sync::Mutex<Option<String>>,
    project: std::sync::Mutex<Option<String>>,
}

impl InMemorySettingsStorage {
    pub fn new() -> Self {
        Self {
            global: std::sync::Mutex::new(None),
            project: std::sync::Mutex::new(None),
        }
    }
}

impl Default for InMemorySettingsStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsStorage for InMemorySettingsStorage {
    fn with_lock(
        &self,
        scope: SettingsScope,
        _current: Option<String>,
        f: Box<dyn FnOnce(Option<&str>) -> Option<String> + Send>,
    ) -> Option<String> {
        let mutex = match scope {
            SettingsScope::Global => &self.global,
            SettingsScope::Project => &self.project,
        };

        let mut guard = mutex.lock().unwrap();
        let current_owned = guard.clone();
        let result = f(current_owned.as_deref());

        if let Some(new_content) = result.clone() {
            *guard = Some(new_content);
        }

        result
    }
}

#[derive(Debug)]
pub struct SettingsError {
    pub scope: SettingsScope,
    pub message: String,
}

pub struct SettingsManager {
    storage: Box<dyn SettingsStorage>,
    global_settings: Settings,
    project_settings: Settings,
    settings: Settings,
    errors: Vec<SettingsError>,
}

impl SettingsManager {
    pub fn new(
        storage: Box<dyn SettingsStorage>,
        initial_global: Settings,
        initial_project: Settings,
    ) -> Self {
        let settings = deep_merge_settings(&initial_global, &initial_project);
        Self {
            storage,
            global_settings: initial_global,
            project_settings: initial_project,
            settings,
            errors: Vec::new(),
        }
    }

    pub fn create(cwd: &str, agent_dir: Option<&str>) -> Self {
        let resolved_agent_dir = agent_dir
            .map(|d| d.to_string())
            .unwrap_or_else(|| config::get_agent_dir().to_string_lossy().to_string());
        let storage = Box::new(FileSettingsStorage::new(cwd, &resolved_agent_dir));
        Self::from_storage(storage)
    }

    pub fn from_storage(storage: Box<dyn SettingsStorage>) -> Self {
        let global_settings = Self::load_from_storage(&*storage, SettingsScope::Global);
        let project_settings = Self::load_from_storage(&*storage, SettingsScope::Project);
        Self::new(storage, global_settings, project_settings)
    }

    fn load_from_storage(storage: &dyn SettingsStorage, scope: SettingsScope) -> Settings {
        let result = storage.with_lock(
            scope,
            None,
            Box::new(|current| current.map(|s| s.to_string())),
        );
        match result {
            Some(content) => serde_json::from_str(&content).unwrap_or_default(),
            None => Settings::default(),
        }
    }

    pub fn get_global_settings(&self) -> &Settings {
        &self.global_settings
    }

    pub fn get_project_settings(&self) -> &Settings {
        &self.project_settings
    }

    pub fn get_settings(&self) -> &Settings {
        &self.settings
    }

    pub fn get<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let json = serde_json::to_value(&self.settings).ok()?;
        json.get(key)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    pub fn set_global(&mut self, key: &str, value: serde_json::Value) {
        let json = serde_json::to_value(&self.global_settings).unwrap_or_default();
        if let serde_json::Value::Object(mut map) = json {
            map.insert(key.to_string(), value);
            if let Ok(s) = serde_json::from_value(serde_json::Value::Object(map)) {
                self.global_settings = s;
            }
        }
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn set_project(&mut self, key: &str, value: serde_json::Value) {
        let json = serde_json::to_value(&self.project_settings).unwrap_or_default();
        if let serde_json::Value::Object(mut map) = json {
            map.insert(key.to_string(), value);
            if let Ok(s) = serde_json::from_value(serde_json::Value::Object(map)) {
                self.project_settings = s;
            }
        }
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Project);
    }

    fn persist_scope(&self, scope: SettingsScope) {
        let settings = match scope {
            SettingsScope::Global => &self.global_settings,
            SettingsScope::Project => &self.project_settings,
        };

        let content = serde_json::to_string_pretty(settings).unwrap_or_default();
        let _ = self
            .storage
            .with_lock(scope, None, Box::new(move |_current| Some(content)));
    }

    pub fn drain_errors(&mut self) -> Vec<SettingsError> {
        std::mem::take(&mut self.errors)
    }

    pub fn apply_overrides(&mut self, overrides: &Settings) {
        self.settings = deep_merge_settings(&self.settings, overrides);
    }

    pub fn reload(&mut self) {
        self.global_settings = Self::load_from_storage(&*self.storage, SettingsScope::Global);
        self.project_settings = Self::load_from_storage(&*self.storage, SettingsScope::Project);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_default() {
        let settings = Settings::default();
        assert!(settings.default_provider.is_none());
        assert!(settings.default_model.is_none());
    }

    #[test]
    fn test_deep_merge_settings() {
        let base = Settings {
            default_provider: Some("anthropic".to_string()),
            thinking_level: Some("medium".to_string()),
            ..Default::default()
        };

        let overlay = Settings {
            thinking_level: Some("high".to_string()),
            custom_system_prompt: Some("Custom prompt".to_string()),
            ..Default::default()
        };

        let merged = deep_merge_settings(&base, &overlay);
        assert_eq!(merged.default_provider, Some("anthropic".to_string()));
        assert_eq!(merged.thinking_level, Some("high".to_string()));
        assert_eq!(
            merged.custom_system_prompt,
            Some("Custom prompt".to_string())
        );
    }

    #[test]
    fn test_in_memory_storage() {
        let storage = InMemorySettingsStorage::new();
        storage.with_lock(
            SettingsScope::Global,
            None,
            Box::new(|current| {
                assert!(current.is_none());
                Some(r#"{"defaultProvider":"anthropic"}"#.to_string())
            }),
        );

        storage.with_lock(
            SettingsScope::Global,
            None,
            Box::new(|current| {
                assert_eq!(current, Some(r#"{"defaultProvider":"anthropic"}"#));
                None
            }),
        );
    }

    #[test]
    fn test_settings_manager_in_memory() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(
            storage,
            Settings {
                default_provider: Some("anthropic".to_string()),
                ..Default::default()
            },
            Settings::default(),
        );

        assert_eq!(
            mgr.get_settings().default_provider,
            Some("anthropic".to_string())
        );

        mgr.set_global("thinkingLevel", serde_json::json!("high"));
        assert_eq!(mgr.get_settings().thinking_level, Some("high".to_string()));
    }

    #[test]
    fn test_settings_manager_project_override() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(
            storage,
            Settings {
                thinking_level: Some("medium".to_string()),
                ..Default::default()
            },
            Settings {
                thinking_level: Some("high".to_string()),
                ..Default::default()
            },
        );

        assert_eq!(mgr.get_settings().thinking_level, Some("high".to_string()));
    }

    #[test]
    fn test_settings_manager_file_storage() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("agent");
        let cwd = dir.path().join("project");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::create_dir_all(&cwd).unwrap();

        let storage = Box::new(FileSettingsStorage::new(
            cwd.to_str().unwrap(),
            agent_dir.to_str().unwrap(),
        ));

        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());
        mgr.set_global("defaultProvider", serde_json::json!("openai"));

        let mgr2 =
            SettingsManager::create(cwd.to_str().unwrap(), Some(agent_dir.to_str().unwrap()));
        assert_eq!(
            mgr2.get_settings().default_provider,
            Some("openai".to_string())
        );
    }
}
