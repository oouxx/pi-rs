use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::config;

// ============================================================================
// Settings types (matching TS interfaces)
// ============================================================================

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

/// Package source matching TS `PackageSource` type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageSource {
    String(String),
    Object {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        autoload: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        extensions: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        skills: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompts: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        themes: Option<Vec<String>>,
    },
}

/// Default project trust matching TS `DefaultProjectTrust`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DefaultProjectTrust {
    #[serde(rename = "ask")]
    Ask,
    #[serde(rename = "always")]
    Always,
    #[serde(rename = "never")]
    Never,
}

/// Steering mode matching TS `"all" | "one-at-a-time"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SteeringMode {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "one-at-a-time")]
    OneAtATime,
}

impl Default for SteeringMode {
    fn default() -> Self {
        SteeringMode::OneAtATime
    }
}

/// Follow-up mode matching TS `"all" | "one-at-a-time"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FollowUpMode {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "one-at-a-time")]
    OneAtATime,
}

impl Default for FollowUpMode {
    fn default() -> Self {
        FollowUpMode::OneAtATime
    }
}

/// Double escape action matching TS `"fork" | "tree" | "none"`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DoubleEscapeAction {
    #[serde(rename = "fork")]
    Fork,
    #[serde(rename = "tree")]
    Tree,
    #[serde(rename = "none")]
    None,
}

impl Default for DoubleEscapeAction {
    fn default() -> Self {
        DoubleEscapeAction::Tree
    }
}

/// Tree filter mode matching TS.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TreeFilterMode {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "no-tools")]
    NoTools,
    #[serde(rename = "user-only")]
    UserOnly,
    #[serde(rename = "labeled-only")]
    LabeledOnly,
    #[serde(rename = "all")]
    All,
}

impl Default for TreeFilterMode {
    fn default() -> Self {
        TreeFilterMode::Default
    }
}

/// Output pad matching TS `0 | 1`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OutputPad {
    #[serde(rename = "0")]
    Zero,
    #[serde(rename = "1")]
    One,
}

impl Default for OutputPad {
    fn default() -> Self {
        OutputPad::One
    }
}

/// Transport setting matching TS `TransportSetting`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransportSetting {
    #[serde(rename = "sse")]
    Sse,
    #[serde(rename = "websocket")]
    Websocket,
    #[serde(rename = "websocket-cached")]
    WebsocketCached,
    #[serde(rename = "auto")]
    Auto,
}

impl Default for TransportSetting {
    fn default() -> Self {
        TransportSetting::Auto
    }
}

/// Main Settings struct matching TS `Settings` interface.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    // Core settings
    pub last_changelog_version: Option<String>,
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub default_thinking_level: Option<String>,
    pub transport: Option<TransportSetting>,
    pub steering_mode: Option<SteeringMode>,
    pub follow_up_mode: Option<FollowUpMode>,
    pub theme: Option<String>,

    // Feature settings
    pub compaction: Option<CompactionSettings>,
    pub branch_summary: Option<BranchSummarySettings>,
    pub retry: Option<RetrySettings>,
    pub hide_thinking_block: Option<bool>,
    pub show_cache_miss_notices: Option<bool>,

    // Editor / shell settings
    pub external_editor: Option<String>,
    pub shell_path: Option<String>,
    pub quiet_startup: Option<bool>,
    pub default_project_trust: Option<DefaultProjectTrust>,
    pub shell_command_prefix: Option<String>,
    pub npm_command: Option<Vec<String>>,

    // UI / display settings
    pub collapse_changelog: Option<bool>,
    pub enable_install_telemetry: Option<bool>,
    pub enable_analytics: Option<bool>,
    pub tracking_id: Option<String>,

    // Packages, extensions, skills, prompts, themes
    pub packages: Option<Vec<PackageSource>>,
    pub extensions: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub themes: Option<Vec<String>>,
    pub enable_skill_commands: Option<bool>,

    // Terminal / image settings
    pub terminal: Option<TerminalSettings>,
    pub images: Option<ImageSettings>,

    // Model / thinking settings
    pub enabled_models: Option<Vec<String>>,
    pub thinking_budgets: Option<ThinkingBudgetsSettings>,

    // Editor / display settings
    pub double_escape_action: Option<DoubleEscapeAction>,
    pub tree_filter_mode: Option<TreeFilterMode>,
    pub editor_padding_x: Option<u32>,
    pub output_pad: Option<OutputPad>,
    pub autocomplete_max_visible: Option<u32>,
    pub show_hardware_cursor: Option<bool>,

    // Markdown / warnings
    pub markdown: Option<MarkdownSettings>,
    pub warnings: Option<WarningSettings>,

    // Session / network settings
    pub session_dir: Option<String>,
    pub http_proxy: Option<String>,
    pub http_idle_timeout_ms: Option<u64>,
    pub websocket_connect_timeout_ms: Option<u64>,

    // Catch-all for unknown fields
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ============================================================================
// Deep merge (matching TS deepMergeSettings)
// ============================================================================

/// Deep merge settings: project/overrides take precedence, nested objects merge shallowly.
/// Matches TS `deepMergeSettings` behavior exactly.
fn deep_merge_settings(base: &Settings, overrides: &Settings) -> Settings {
    let base_json = serde_json::to_value(base).unwrap_or(serde_json::Value::Object(Default::default()));
    let overrides_json = serde_json::to_value(overrides).unwrap_or(serde_json::Value::Object(Default::default()));

    if let (serde_json::Value::Object(base_map), serde_json::Value::Object(overrides_map)) =
        (&base_json, &overrides_json)
    {
        let mut result = base_map.clone();
        for (key, value) in overrides_map {
            if value.is_null() {
                // TS: undefined values are skipped (null in JSON from Option::None)
                continue;
            }
            // For nested objects, merge shallowly (TS: { ...baseValue, ...overrideValue })
            if let Some(base_value) = base_map.get(key) {
                if value.is_object() && base_value.is_object() {
                    if let (serde_json::Value::Object(base_obj), serde_json::Value::Object(overlay_obj)) =
                        (base_value, &value)
                    {
                        let mut merged_obj = base_obj.clone();
                        for (k, v) in overlay_obj {
                            if !v.is_null() {
                                merged_obj.insert(k.clone(), v.clone());
                            }
                        }
                        result.insert(key.clone(), serde_json::Value::Object(merged_obj));
                        continue;
                    }
                }
            }
            // For primitives and arrays, override wins
            result.insert(key.clone(), value.clone());
        }
        if let Ok(s) = serde_json::from_value(serde_json::Value::Object(result)) {
            return s;
        }
    }

    base.clone()
}

// ============================================================================
// Settings migration (matching TS migrateSettings)
// ============================================================================

/// Migrate old settings format to new format.
/// Matches TS `SettingsManager.migrateSettings()`.
fn migrate_settings(settings: &mut serde_json::Value) {
    if let serde_json::Value::Object(ref mut map) = settings {
        // Migrate queueMode -> steeringMode
        if let Some(queue_mode) = map.remove("queueMode") {
            if !map.contains_key("steeringMode") {
                map.insert("steeringMode".to_string(), queue_mode);
            }
        }

        // Migrate legacy websockets boolean -> transport enum
        if !map.contains_key("transport") {
            if let Some(websockets) = map.get("websockets").and_then(|v| v.as_bool()) {
                map.insert(
                    "transport".to_string(),
                    serde_json::Value::String(if websockets {
                        "websocket".to_string()
                    } else {
                        "sse".to_string()
                    }),
                );
            }
        }
        map.remove("websockets");

        // Migrate old skills object format to new array format
        let skills_clone = map.get("skills").cloned();
        if let Some(skills_val) = skills_clone {
            if let Some(skills_obj) = skills_val.as_object() {
                if let Some(enable_skill_commands) = skills_obj.get("enableSkillCommands").and_then(|v| v.as_bool()) {
                    if !map.contains_key("enableSkillCommands") {
                        map.insert("enableSkillCommands".to_string(), serde_json::Value::Bool(enable_skill_commands));
                    }
                }
                if let Some(custom_dirs) = skills_obj.get("customDirectories").and_then(|v| v.as_array()) {
                    if !custom_dirs.is_empty() {
                        map.insert("skills".to_string(), serde_json::Value::Array(custom_dirs.clone()));
                    } else {
                        map.remove("skills");
                    }
                } else {
                    map.remove("skills");
                }
            }
        }

        // Migrate retry.maxDelayMs -> retry.provider.maxRetryDelayMs
        if let Some(retry_val) = map.get_mut("retry") {
            if let Some(retry_obj) = retry_val.as_object_mut() {
                if let Some(max_delay_val) = retry_obj.remove("maxDelayMs") {
                    let max_delay_ms: u64 = serde_json::from_value(max_delay_val).unwrap_or(60000);
                    let provider = retry_obj
                        .entry("provider".to_string())
                        .or_insert(serde_json::Value::Object(serde_json::Map::new()));
                    if let Some(provider_obj) = provider.as_object_mut() {
                        if !provider_obj.contains_key("maxRetryDelayMs") {
                            provider_obj.insert(
                                "maxRetryDelayMs".to_string(),
                                serde_json::Value::Number(serde_json::Number::from(max_delay_ms)),
                            );
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// Settings storage trait and implementations
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsScope {
    Global,
    Project,
}

pub trait SettingsStorage: Send + Sync {
    fn with_lock(
        &self,
        scope: SettingsScope,
        f: Box<dyn FnOnce(Option<&str>) -> Option<String> + Send>,
    );
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

    fn path_for_scope(&self, scope: &SettingsScope) -> PathBuf {
        match scope {
            SettingsScope::Global => self.global_settings_path.clone(),
            SettingsScope::Project => self.project_settings_path.clone(),
        }
    }
}

impl SettingsStorage for FileSettingsStorage {
    fn with_lock(
        &self,
        scope: SettingsScope,
        f: Box<dyn FnOnce(Option<&str>) -> Option<String> + Send>,
    ) {
        let path = self.path_for_scope(&scope);
        let dir = path.parent().unwrap();

        // Check if file exists before trying to read
        let current = if path.exists() {
            match fs::read_to_string(&path) {
                Ok(content) => Some(content),
                Err(_) => None,
            }
        } else {
            None
        };

        let next = f(current.as_deref());

        if let Some(new_content) = next {
            // Only create directory when we actually need to write
            if !dir.exists() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&path, &new_content);
        }
    }
}

pub struct InMemorySettingsStorage {
    global: Mutex<Option<String>>,
    project: Mutex<Option<String>>,
}

impl InMemorySettingsStorage {
    pub fn new() -> Self {
        Self {
            global: Mutex::new(None),
            project: Mutex::new(None),
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
        f: Box<dyn FnOnce(Option<&str>) -> Option<String> + Send>,
    ) {
        let mutex = match scope {
            SettingsScope::Global => &self.global,
            SettingsScope::Project => &self.project,
        };

        let mut guard = mutex.lock().unwrap();
        let current_owned = guard.clone();
        let result = f(current_owned.as_deref());

        if let Some(new_content) = result {
            *guard = Some(new_content);
        }
    }
}

// ============================================================================
// SettingsError
// ============================================================================

#[derive(Debug, Clone)]
pub struct SettingsError {
    pub scope: SettingsScope,
    pub message: String,
}

// ============================================================================
// SettingsManager
// ============================================================================

pub struct SettingsManager {
    storage: Box<dyn SettingsStorage>,
    global_settings: Settings,
    project_settings: Settings,
    settings: Settings,
    project_trusted: bool,
    /// Track global fields modified during session (matching TS modifiedFields)
    modified_fields: HashSet<String>,
    /// Track global nested field modifications (matching TS modifiedNestedFields)
    modified_nested_fields: HashMap<String, HashSet<String>>,
    /// Track project fields modified during session
    modified_project_fields: HashSet<String>,
    /// Track project nested field modifications
    modified_project_nested_fields: HashMap<String, HashSet<String>>,
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
            project_trusted: true,
            modified_fields: HashSet::new(),
            modified_nested_fields: HashMap::new(),
            modified_project_fields: HashSet::new(),
            modified_project_nested_fields: HashMap::new(),
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
        let (global_settings, global_error) = Self::load_from_storage(&*storage, SettingsScope::Global);
        let (project_settings, project_error) = Self::load_from_storage(&*storage, SettingsScope::Project);

        let mut errors = Vec::new();
        if let Some(err) = global_error {
            errors.push(SettingsError {
                scope: SettingsScope::Global,
                message: err,
            });
        }
        if let Some(err) = project_error {
            errors.push(SettingsError {
                scope: SettingsScope::Project,
                message: err,
            });
        }

        let settings = deep_merge_settings(&global_settings, &project_settings);
        Self {
            storage,
            global_settings,
            project_settings,
            settings,
            project_trusted: true,
            modified_fields: HashSet::new(),
            modified_nested_fields: HashMap::new(),
            modified_project_fields: HashSet::new(),
            modified_project_nested_fields: HashMap::new(),
            errors,
        }
    }

    fn load_from_storage(storage: &dyn SettingsStorage, scope: SettingsScope) -> (Settings, Option<String>) {
        // Use a two-step approach: first get the raw content, then parse it.
        // This avoids lifetime issues with capturing local variables in the closure.
        let raw_content: Option<String> = {
            let result = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
            let result_for_closure = result.clone();
            storage.with_lock(
                scope,
                Box::new(move |current| {
                    *result_for_closure.lock().unwrap() = current.map(|s| s.to_string());
                    None // don't write
                }),
            );
            // After with_lock returns, the closure (and result_for_closure) has been dropped.
            // result is now the only reference. Use lock().unwrap().take() to get the value.
            let x = result.lock().unwrap().take(); x
        };

        match raw_content {
            Some(content) => {
                // Parse JSON and migrate
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(mut json) => {
                        migrate_settings(&mut json);
                        match serde_json::from_value::<Settings>(json) {
                            Ok(settings) => (settings, None),
                            Err(e) => (Settings::default(), Some(format!("Parse error: {}", e))),
                        }
                    }
                    Err(e) => (Settings::default(), Some(format!("JSON parse error: {}", e))),
                }
            }
            None => (Settings::default(), None),
        }
    }

    // ========================================================================
    // Public getters
    // ========================================================================

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

    // ========================================================================
    // Project trust
    // ========================================================================

    pub fn is_project_trusted(&self) -> bool {
        self.project_trusted
    }

    pub fn set_project_trusted(&mut self, trusted: bool) {
        if self.project_trusted == trusted {
            return;
        }

        self.project_trusted = trusted;
        self.modified_project_fields.clear();
        self.modified_project_nested_fields.clear();

        if !trusted {
            self.project_settings = Settings::default();
            self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
            return;
        }

        let (project_settings, _error) = Self::load_from_storage(&*self.storage, SettingsScope::Project);
        self.project_settings = project_settings;
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
    }

    // ========================================================================
    // Mark modified fields (matching TS markModified / markProjectModified)
    // ========================================================================

    fn mark_modified(&mut self, field: &str, nested_key: Option<&str>) {
        self.modified_fields.insert(field.to_string());
        if let Some(nk) = nested_key {
            self.modified_nested_fields
                .entry(field.to_string())
                .or_default()
                .insert(nk.to_string());
        }
    }

    fn mark_project_modified(&mut self, field: &str, nested_key: Option<&str>) {
        self.modified_project_fields.insert(field.to_string());
        if let Some(nk) = nested_key {
            self.modified_project_nested_fields
                .entry(field.to_string())
                .or_default()
                .insert(nk.to_string());
        }
    }

    // ========================================================================
    // Persist (matching TS persistScopedSettings - only writes modified fields)
    // ========================================================================

    fn persist_scope(&self, scope: SettingsScope) {
        let (settings, modified_fields, modified_nested_fields) = match scope {
            SettingsScope::Global => (
                &self.global_settings,
                &self.modified_fields,
                &self.modified_nested_fields,
            ),
            SettingsScope::Project => (
                &self.project_settings,
                &self.modified_project_fields,
                &self.modified_project_nested_fields,
            ),
        };

        let settings_json = serde_json::to_value(settings).unwrap_or(serde_json::Value::Object(Default::default()));

        let storage = &self.storage;
        let scope_clone = scope.clone();
        let modified_fields_clone: HashSet<String> = modified_fields.clone();
        let modified_nested_fields_clone: HashMap<String, HashSet<String>> = modified_nested_fields.clone();

        storage.with_lock(
            scope_clone,
            Box::new(move |current| {
                // Parse current file content
                let current_file_settings: serde_json::Value = current
                    .and_then(|c| serde_json::from_str(c).ok())
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let mut merged = if let serde_json::Value::Object(map) = current_file_settings {
                    map
                } else {
                    serde_json::Map::new()
                };

                // Only overwrite modified fields
                if let serde_json::Value::Object(ref settings_map) = settings_json {
                    for field in &modified_fields_clone {
                        if let Some(value) = settings_map.get(field) {
                            // Check if this field has nested modifications
                            if let Some(nested_modified) = modified_nested_fields_clone.get(field) {
                                // Nested merge: only overwrite modified sub-keys
                                if let Some(existing) = merged.get(field) {
                                    if let serde_json::Value::Object(existing_obj) = existing {
                                        if let serde_json::Value::Object(settings_obj) = value {
                                            let mut merged_obj = existing_obj.clone();
                                            for nested_key in nested_modified {
                                                if let Some(nv) = settings_obj.get(nested_key) {
                                                    merged_obj.insert(nested_key.clone(), nv.clone());
                                                }
                                            }
                                            merged.insert(field.clone(), serde_json::Value::Object(merged_obj));
                                            continue;
                                        }
                                    }
                                }
                            }
                            merged.insert(field.clone(), value.clone());
                        }
                    }
                }

                Some(serde_json::to_string_pretty(&serde_json::Value::Object(merged)).unwrap_or_default())
            }),
        );
    }

    // ========================================================================
    // Set methods (matching TS set_global / set_project)
    // ========================================================================

    pub fn set_global(&mut self, key: &str, value: serde_json::Value) {
        let json = serde_json::to_value(&self.global_settings).unwrap_or(serde_json::Value::Object(Default::default()));
        if let serde_json::Value::Object(mut map) = json {
            map.insert(key.to_string(), value);
            if let Ok(s) = serde_json::from_value(serde_json::Value::Object(map)) {
                self.global_settings = s;
            }
        }
        self.mark_modified(key, None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn set_project(&mut self, key: &str, value: serde_json::Value) {
        let json = serde_json::to_value(&self.project_settings).unwrap_or(serde_json::Value::Object(Default::default()));
        if let serde_json::Value::Object(mut map) = json {
            map.insert(key.to_string(), value);
            if let Ok(s) = serde_json::from_value(serde_json::Value::Object(map)) {
                self.project_settings = s;
            }
        }
        self.mark_project_modified(key, None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Project);
    }

    // ========================================================================
    // Error handling
    // ========================================================================

    pub fn drain_errors(&mut self) -> Vec<SettingsError> {
        std::mem::take(&mut self.errors)
    }

    // ========================================================================
    // Overrides
    // ========================================================================

    pub fn apply_overrides(&mut self, overrides: &Settings) {
        self.settings = deep_merge_settings(&self.settings, overrides);
    }

    // ========================================================================
    // Reload
    // ========================================================================

    pub fn reload(&mut self) {
        let (global_settings, _) = Self::load_from_storage(&*self.storage, SettingsScope::Global);
        self.global_settings = global_settings;
        self.modified_fields.clear();
        self.modified_nested_fields.clear();

        let (project_settings, _) = Self::load_from_storage(&*self.storage, SettingsScope::Project);
        self.project_settings = project_settings;
        self.modified_project_fields.clear();
        self.modified_project_nested_fields.clear();

        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
    }

    // ========================================================================
    // Getter/setter methods matching TS SettingsManager API
    // ========================================================================

    // --- lastChangelogVersion ---

    pub fn get_last_changelog_version(&self) -> Option<&str> {
        self.settings.last_changelog_version.as_deref()
    }

    pub fn set_last_changelog_version(&mut self, version: &str) {
        self.global_settings.last_changelog_version = Some(version.to_string());
        self.mark_modified("lastChangelogVersion", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- sessionDir ---

    pub fn get_session_dir(&self) -> Option<String> {
        self.settings.session_dir.clone()
    }

    // --- defaultProvider / defaultModel ---

    pub fn get_default_provider(&self) -> Option<&str> {
        self.settings.default_provider.as_deref()
    }

    pub fn get_default_model(&self) -> Option<&str> {
        self.settings.default_model.as_deref()
    }

    pub fn set_default_provider(&mut self, provider: &str) {
        self.global_settings.default_provider = Some(provider.to_string());
        self.mark_modified("defaultProvider", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn set_default_model(&mut self, model_id: &str) {
        self.global_settings.default_model = Some(model_id.to_string());
        self.mark_modified("defaultModel", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn set_default_model_and_provider(&mut self, provider: &str, model_id: &str) {
        self.global_settings.default_provider = Some(provider.to_string());
        self.global_settings.default_model = Some(model_id.to_string());
        self.mark_modified("defaultProvider", None);
        self.mark_modified("defaultModel", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- steeringMode ---

    pub fn get_steering_mode(&self) -> SteeringMode {
        self.settings.steering_mode.clone().unwrap_or_default()
    }

    pub fn set_steering_mode(&mut self, mode: SteeringMode) {
        self.global_settings.steering_mode = Some(mode);
        self.mark_modified("steeringMode", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- followUpMode ---

    pub fn get_follow_up_mode(&self) -> FollowUpMode {
        self.settings.follow_up_mode.clone().unwrap_or_default()
    }

    pub fn set_follow_up_mode(&mut self, mode: FollowUpMode) {
        self.global_settings.follow_up_mode = Some(mode);
        self.mark_modified("followUpMode", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- theme ---

    pub fn get_theme_setting(&self) -> Option<&str> {
        self.settings.theme.as_deref()
    }

    pub fn get_theme(&self) -> Option<&str> {
        let theme = self.settings.theme.as_deref()?;
        if theme.contains('/') {
            None
        } else {
            Some(theme)
        }
    }

    pub fn set_theme(&mut self, theme: &str) {
        self.global_settings.theme = Some(theme.to_string());
        self.mark_modified("theme", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- defaultThinkingLevel ---

    pub fn get_default_thinking_level(&self) -> Option<&str> {
        self.settings.default_thinking_level.as_deref()
    }

    pub fn set_default_thinking_level(&mut self, level: &str) {
        self.global_settings.default_thinking_level = Some(level.to_string());
        self.mark_modified("defaultThinkingLevel", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- transport ---

    pub fn get_transport(&self) -> TransportSetting {
        self.settings.transport.clone().unwrap_or_default()
    }

    pub fn set_transport(&mut self, transport: TransportSetting) {
        self.global_settings.transport = Some(transport);
        self.mark_modified("transport", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- compaction ---

    pub fn get_compaction_enabled(&self) -> bool {
        self.settings.compaction.as_ref().and_then(|c| c.enabled).unwrap_or(true)
    }

    pub fn set_compaction_enabled(&mut self, enabled: bool) {
        let mut compaction = self.global_settings.compaction.clone().unwrap_or_default();
        compaction.enabled = Some(enabled);
        self.global_settings.compaction = Some(compaction);
        self.mark_modified("compaction", Some("enabled"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn get_compaction_reserve_tokens(&self) -> u64 {
        self.settings.compaction.as_ref().and_then(|c| c.reserve_tokens).unwrap_or(16384)
    }

    pub fn get_compaction_keep_recent_tokens(&self) -> u64 {
        self.settings.compaction.as_ref().and_then(|c| c.keep_recent_tokens).unwrap_or(20000)
    }

    pub fn get_compaction_settings(&self) -> CompactionSettings {
        self.settings.compaction.clone().unwrap_or_default()
    }

    // --- branchSummary ---

    pub fn get_branch_summary_settings(&self) -> BranchSummarySettings {
        self.settings.branch_summary.clone().unwrap_or_default()
    }

    pub fn get_branch_summary_skip_prompt(&self) -> bool {
        self.settings.branch_summary.as_ref().and_then(|b| b.skip_prompt).unwrap_or(false)
    }

    // --- retry ---

    pub fn get_retry_enabled(&self) -> bool {
        self.settings.retry.as_ref().and_then(|r| r.enabled).unwrap_or(true)
    }

    pub fn set_retry_enabled(&mut self, enabled: bool) {
        let mut retry = self.global_settings.retry.clone().unwrap_or_default();
        retry.enabled = Some(enabled);
        self.global_settings.retry = Some(retry);
        self.mark_modified("retry", Some("enabled"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn get_retry_settings(&self) -> RetrySettings {
        self.settings.retry.clone().unwrap_or_default()
    }

    // --- hideThinkingBlock ---

    pub fn get_hide_thinking_block(&self) -> bool {
        self.settings.hide_thinking_block.unwrap_or(false)
    }

    pub fn set_hide_thinking_block(&mut self, hide: bool) {
        self.global_settings.hide_thinking_block = Some(hide);
        self.mark_modified("hideThinkingBlock", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- showCacheMissNotices ---

    pub fn get_show_cache_miss_notices(&self) -> bool {
        self.settings.show_cache_miss_notices.unwrap_or(false)
    }

    pub fn set_show_cache_miss_notices(&mut self, show: bool) {
        self.global_settings.show_cache_miss_notices = Some(show);
        self.mark_modified("showCacheMissNotices", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- externalEditor ---

    pub fn get_external_editor_command(&self) -> Option<&str> {
        self.settings.external_editor.as_deref()
    }

    // --- shellPath ---

    pub fn get_shell_path(&self) -> Option<&str> {
        self.settings.shell_path.as_deref()
    }

    pub fn set_shell_path(&mut self, path: Option<String>) {
        self.global_settings.shell_path = path;
        self.mark_modified("shellPath", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- quietStartup ---

    pub fn get_quiet_startup(&self) -> bool {
        self.settings.quiet_startup.unwrap_or(false)
    }

    pub fn set_quiet_startup(&mut self, quiet: bool) {
        self.global_settings.quiet_startup = Some(quiet);
        self.mark_modified("quietStartup", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- defaultProjectTrust ---

    pub fn get_default_project_trust(&self) -> DefaultProjectTrust {
        self.global_settings.default_project_trust.clone().unwrap_or(DefaultProjectTrust::Ask)
    }

    pub fn set_default_project_trust(&mut self, trust: DefaultProjectTrust) {
        self.global_settings.default_project_trust = Some(trust);
        self.mark_modified("defaultProjectTrust", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- shellCommandPrefix ---

    pub fn get_shell_command_prefix(&self) -> Option<&str> {
        self.settings.shell_command_prefix.as_deref()
    }

    pub fn set_shell_command_prefix(&mut self, prefix: Option<String>) {
        self.global_settings.shell_command_prefix = prefix;
        self.mark_modified("shellCommandPrefix", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- npmCommand ---

    pub fn get_npm_command(&self) -> Option<&[String]> {
        self.settings.npm_command.as_deref()
    }

    pub fn set_npm_command(&mut self, command: Option<Vec<String>>) {
        self.global_settings.npm_command = command;
        self.mark_modified("npmCommand", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- collapseChangelog ---

    pub fn get_collapse_changelog(&self) -> bool {
        self.settings.collapse_changelog.unwrap_or(false)
    }

    pub fn set_collapse_changelog(&mut self, collapse: bool) {
        self.global_settings.collapse_changelog = Some(collapse);
        self.mark_modified("collapseChangelog", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- enableInstallTelemetry ---

    pub fn get_enable_install_telemetry(&self) -> bool {
        self.settings.enable_install_telemetry.unwrap_or(true)
    }

    pub fn set_enable_install_telemetry(&mut self, enabled: bool) {
        self.global_settings.enable_install_telemetry = Some(enabled);
        self.mark_modified("enableInstallTelemetry", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- enableAnalytics / trackingId ---

    pub fn get_enable_analytics(&self) -> bool {
        self.settings.enable_analytics.unwrap_or(false)
    }

    pub fn get_tracking_id(&self) -> Option<&str> {
        self.settings.tracking_id.as_deref()
    }

    pub fn set_enable_analytics(&mut self, enabled: bool) {
        self.global_settings.enable_analytics = Some(enabled);
        self.mark_modified("enableAnalytics", None);
        if enabled && self.global_settings.tracking_id.is_none() {
            self.global_settings.tracking_id = Some(uuid::Uuid::new_v4().to_string());
            self.mark_modified("trackingId", None);
        }
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- packages ---

    pub fn get_packages(&self) -> Vec<PackageSource> {
        self.settings.packages.clone().unwrap_or_default()
    }

    pub fn set_packages(&mut self, packages: Vec<PackageSource>) {
        self.global_settings.packages = Some(packages);
        self.mark_modified("packages", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- extensions ---

    pub fn get_extensions(&self) -> Vec<String> {
        self.settings.extensions.clone().unwrap_or_default()
    }

    pub fn set_extensions(&mut self, extensions: Vec<String>) {
        self.global_settings.extensions = Some(extensions);
        self.mark_modified("extensions", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn set_project_extensions(&mut self, extensions: Vec<String>) {
        self.project_settings.extensions = Some(extensions);
        self.mark_project_modified("extensions", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Project);
    }

    // --- skills ---

    pub fn get_skills(&self) -> Vec<String> {
        self.settings.skills.clone().unwrap_or_default()
    }

    pub fn set_skills(&mut self, skills: Vec<String>) {
        self.global_settings.skills = Some(skills);
        self.mark_modified("skills", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- prompts ---

    pub fn get_prompts(&self) -> Vec<String> {
        self.settings.prompts.clone().unwrap_or_default()
    }

    pub fn set_prompts(&mut self, prompts: Vec<String>) {
        self.global_settings.prompts = Some(prompts);
        self.mark_modified("prompts", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- themes ---

    pub fn get_themes(&self) -> Vec<String> {
        self.settings.themes.clone().unwrap_or_default()
    }

    pub fn set_themes(&mut self, themes: Vec<String>) {
        self.global_settings.themes = Some(themes);
        self.mark_modified("themes", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- enableSkillCommands ---

    pub fn get_enable_skill_commands(&self) -> bool {
        self.settings.enable_skill_commands.unwrap_or(true)
    }

    pub fn set_enable_skill_commands(&mut self, enabled: bool) {
        self.global_settings.enable_skill_commands = Some(enabled);
        self.mark_modified("enableSkillCommands", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- thinkingBudgets ---

    pub fn get_thinking_budgets(&self) -> Option<&ThinkingBudgetsSettings> {
        self.settings.thinking_budgets.as_ref()
    }

    // --- terminal settings ---

    pub fn get_show_images(&self) -> bool {
        self.settings.terminal.as_ref().and_then(|t| t.show_images).unwrap_or(true)
    }

    pub fn set_show_images(&mut self, show: bool) {
        let mut terminal = self.global_settings.terminal.clone().unwrap_or_default();
        terminal.show_images = Some(show);
        self.global_settings.terminal = Some(terminal);
        self.mark_modified("terminal", Some("showImages"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn get_image_width_cells(&self) -> u32 {
        let width = self.settings.terminal.as_ref().and_then(|t| t.image_width_cells);
        match width {
            Some(w) if w >= 1 => w,
            _ => 60,
        }
    }

    pub fn set_image_width_cells(&mut self, width: u32) {
        let w = std::cmp::max(1, width);
        let mut terminal = self.global_settings.terminal.clone().unwrap_or_default();
        terminal.image_width_cells = Some(w);
        self.global_settings.terminal = Some(terminal);
        self.mark_modified("terminal", Some("imageWidthCells"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn get_clear_on_shrink(&self) -> bool {
        if let Some(val) = self.settings.terminal.as_ref().and_then(|t| t.clear_on_shrink) {
            return val;
        }
        // Fall back to env var (matching TS behavior)
        std::env::var("PI_CLEAR_ON_SHRINK").as_deref() == Ok("1")
    }

    pub fn set_clear_on_shrink(&mut self, enabled: bool) {
        let mut terminal = self.global_settings.terminal.clone().unwrap_or_default();
        terminal.clear_on_shrink = Some(enabled);
        self.global_settings.terminal = Some(terminal);
        self.mark_modified("terminal", Some("clearOnShrink"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn get_show_terminal_progress(&self) -> bool {
        self.settings.terminal.as_ref().and_then(|t| t.show_terminal_progress).unwrap_or(false)
    }

    pub fn set_show_terminal_progress(&mut self, enabled: bool) {
        let mut terminal = self.global_settings.terminal.clone().unwrap_or_default();
        terminal.show_terminal_progress = Some(enabled);
        self.global_settings.terminal = Some(terminal);
        self.mark_modified("terminal", Some("showTerminalProgress"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- image settings ---

    pub fn get_image_auto_resize(&self) -> bool {
        self.settings.images.as_ref().and_then(|i| i.auto_resize).unwrap_or(true)
    }

    pub fn set_image_auto_resize(&mut self, enabled: bool) {
        let mut images = self.global_settings.images.clone().unwrap_or_default();
        images.auto_resize = Some(enabled);
        self.global_settings.images = Some(images);
        self.mark_modified("images", Some("autoResize"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    pub fn get_block_images(&self) -> bool {
        self.settings.images.as_ref().and_then(|i| i.block_images).unwrap_or(false)
    }

    pub fn set_block_images(&mut self, blocked: bool) {
        let mut images = self.global_settings.images.clone().unwrap_or_default();
        images.block_images = Some(blocked);
        self.global_settings.images = Some(images);
        self.mark_modified("images", Some("blockImages"));
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- enabledModels ---

    pub fn get_enabled_models(&self) -> Option<&[String]> {
        self.settings.enabled_models.as_deref()
    }

    pub fn set_enabled_models(&mut self, patterns: Option<Vec<String>>) {
        self.global_settings.enabled_models = patterns;
        self.mark_modified("enabledModels", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- doubleEscapeAction ---

    pub fn get_double_escape_action(&self) -> DoubleEscapeAction {
        self.settings.double_escape_action.clone().unwrap_or_default()
    }

    pub fn set_double_escape_action(&mut self, action: DoubleEscapeAction) {
        self.global_settings.double_escape_action = Some(action);
        self.mark_modified("doubleEscapeAction", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- treeFilterMode ---

    pub fn get_tree_filter_mode(&self) -> TreeFilterMode {
        let mode = self.settings.tree_filter_mode.clone();
        match mode {
            Some(m) => m,
            None => TreeFilterMode::Default,
        }
    }

    pub fn set_tree_filter_mode(&mut self, mode: TreeFilterMode) {
        self.global_settings.tree_filter_mode = Some(mode);
        self.mark_modified("treeFilterMode", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- showHardwareCursor ---

    pub fn get_show_hardware_cursor(&self) -> bool {
        if let Some(val) = self.settings.show_hardware_cursor {
            return val;
        }
        std::env::var("PI_HARDWARE_CURSOR").as_deref() == Ok("1")
    }

    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        self.global_settings.show_hardware_cursor = Some(enabled);
        self.mark_modified("showHardwareCursor", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- editorPaddingX ---

    pub fn get_editor_padding_x(&self) -> u32 {
        self.settings.editor_padding_x.unwrap_or(0)
    }

    pub fn set_editor_padding_x(&mut self, padding: u32) {
        let p = std::cmp::max(0, std::cmp::min(3, padding));
        self.global_settings.editor_padding_x = Some(p);
        self.mark_modified("editorPaddingX", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- outputPad ---

    pub fn get_output_pad(&self) -> OutputPad {
        match self.settings.output_pad {
            Some(OutputPad::Zero) => OutputPad::Zero,
            _ => OutputPad::One,
        }
    }

    pub fn set_output_pad(&mut self, pad: OutputPad) {
        self.global_settings.output_pad = Some(pad);
        self.mark_modified("outputPad", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- autocompleteMaxVisible ---

    pub fn get_autocomplete_max_visible(&self) -> u32 {
        self.settings.autocomplete_max_visible.unwrap_or(5)
    }

    pub fn set_autocomplete_max_visible(&mut self, max_visible: u32) {
        let v = std::cmp::max(3, std::cmp::min(20, max_visible));
        self.global_settings.autocomplete_max_visible = Some(v);
        self.mark_modified("autocompleteMaxVisible", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }

    // --- codeBlockIndent ---

    pub fn get_code_block_indent(&self) -> &str {
        self.settings.markdown.as_ref()
            .and_then(|m| m.code_block_indent.as_deref())
            .unwrap_or("  ")
    }

    // --- warnings ---

    pub fn get_warnings(&self) -> WarningSettings {
        self.settings.warnings.clone().unwrap_or_default()
    }

    pub fn set_warnings(&mut self, warnings: WarningSettings) {
        self.global_settings.warnings = Some(warnings);
        self.mark_modified("warnings", None);
        self.settings = deep_merge_settings(&self.global_settings, &self.project_settings);
        self.persist_scope(SettingsScope::Global);
    }
}

// ============================================================================
// Tests
// ============================================================================

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
            default_thinking_level: Some("medium".to_string()),
            ..Default::default()
        };

        let overlay = Settings {
            default_thinking_level: Some("high".to_string()),
            ..Default::default()
        };

        let merged = deep_merge_settings(&base, &overlay);
        assert_eq!(merged.default_provider, Some("anthropic".to_string()));
        assert_eq!(merged.default_thinking_level, Some("high".to_string()));
    }

    #[test]
    fn test_deep_merge_nested_objects() {
        let base = Settings {
            compaction: Some(CompactionSettings {
                enabled: Some(true),
                reserve_tokens: Some(1000),
                keep_recent_tokens: Some(500),
            }),
            ..Default::default()
        };

        let overlay = Settings {
            compaction: Some(CompactionSettings {
                enabled: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };

        let merged = deep_merge_settings(&base, &overlay);
        let c = merged.compaction.unwrap();
        assert!(!c.enabled.unwrap());
        assert_eq!(c.reserve_tokens, Some(1000)); // preserved from base
        assert_eq!(c.keep_recent_tokens, Some(500)); // preserved from base
    }

    #[test]
    fn test_in_memory_storage() {
        let storage = InMemorySettingsStorage::new();
        storage.with_lock(
            SettingsScope::Global,
            Box::new(|current| {
                assert!(current.is_none());
                Some(r#"{"defaultProvider":"anthropic"}"#.to_string())
            }),
        );

        storage.with_lock(
            SettingsScope::Global,
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
            mgr.get_settings().default_provider.as_deref(),
            Some("anthropic")
        );

        mgr.set_default_thinking_level("high");
        assert_eq!(mgr.get_default_thinking_level(), Some("high"));
    }

    #[test]
    fn test_settings_manager_project_override() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(
            storage,
            Settings {
                default_thinking_level: Some("medium".to_string()),
                ..Default::default()
            },
            Settings {
                default_thinking_level: Some("high".to_string()),
                ..Default::default()
            },
        );

        assert_eq!(mgr.get_default_thinking_level(), Some("high"));
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
        mgr.set_default_provider("openai");

        let mgr2 =
            SettingsManager::create(cwd.to_str().unwrap(), Some(agent_dir.to_str().unwrap()));
        assert_eq!(
            mgr2.get_settings().default_provider.as_deref(),
            Some("openai")
        );
    }

    #[test]
    fn test_migrate_queue_mode() {
        let mut json = serde_json::json!({
            "queueMode": "all"
        });
        migrate_settings(&mut json);
        assert_eq!(json.get("steeringMode").and_then(|v| v.as_str()), Some("all"));
        assert!(json.get("queueMode").is_none());
    }

    #[test]
    fn test_migrate_websockets() {
        let mut json = serde_json::json!({
            "websockets": true
        });
        migrate_settings(&mut json);
        assert_eq!(json.get("transport").and_then(|v| v.as_str()), Some("websocket"));
        assert!(json.get("websockets").is_none());
    }

    #[test]
    fn test_migrate_skills_object() {
        let mut json = serde_json::json!({
            "skills": {
                "enableSkillCommands": false,
                "customDirectories": ["/path/to/skill"]
            }
        });
        migrate_settings(&mut json);
        assert_eq!(json.get("enableSkillCommands").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            json.get("skills").and_then(|v| v.as_array()).map(|a| a.len()),
            Some(1)
        );
    }

    #[test]
    fn test_migrate_retry_max_delay() {
        let mut json = serde_json::json!({
            "retry": {
                "maxDelayMs": 30000
            }
        });
        migrate_settings(&mut json);
        let retry = json.get("retry").and_then(|v| v.as_object()).unwrap();
        assert!(retry.get("maxDelayMs").is_none());
        let provider = retry.get("provider").and_then(|v| v.as_object()).unwrap();
        assert_eq!(
            provider.get("maxRetryDelayMs").and_then(|v| v.as_u64()),
            Some(30000)
        );
    }

    #[test]
    fn test_preserve_external_changes() {
        let storage = Box::new(InMemorySettingsStorage::new());

        // Pre-populate storage with settings
        storage.with_lock(
            SettingsScope::Global,
            Box::new(|_| {
                Some(r#"{"theme":"dark","defaultModel":"claude-sonnet"}"#.to_string())
            }),
        );

        let mut mgr = SettingsManager::from_storage(storage);

        // Simulate external edit: add enabledModels
        mgr.storage.with_lock(
            SettingsScope::Global,
            Box::new(|current| {
                let mut settings: serde_json::Value = current
                    .and_then(|c| serde_json::from_str(c).ok())
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                if let serde_json::Value::Object(ref mut map) = settings {
                    map.insert(
                        "enabledModels".to_string(),
                        serde_json::Value::Array(vec![
                            serde_json::Value::String("claude-opus-4-5".to_string()),
                            serde_json::Value::String("gpt-5.2-codex".to_string()),
                        ]),
                    );
                }
                Some(serde_json::to_string_pretty(&settings).unwrap_or_default())
            }),
        );

        // Change an unrelated setting
        mgr.set_default_thinking_level("high");

        // Verify enabledModels is preserved
        mgr.storage.with_lock(
            SettingsScope::Global,
            Box::new(|current| {
                let settings: serde_json::Value = current
                    .and_then(|c| serde_json::from_str(c).ok())
                    .unwrap_or(serde_json::Value::Null);
                let enabled_models = settings
                    .get("enabledModels")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len());
                assert_eq!(enabled_models, Some(2));
                assert_eq!(
                    settings.get("defaultThinkingLevel").and_then(|v| v.as_str()),
                    Some("high")
                );
                assert_eq!(
                    settings.get("theme").and_then(|v| v.as_str()),
                    Some("dark")
                );
                assert_eq!(
                    settings.get("defaultModel").and_then(|v| v.as_str()),
                    Some("claude-sonnet")
                );
                None
            }),
        );
    }

    #[test]
    fn test_project_trust() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert!(mgr.is_project_trusted());

        mgr.set_project_trusted(false);
        assert!(!mgr.is_project_trusted());

        mgr.set_project_trusted(true);
        assert!(mgr.is_project_trusted());
    }

    #[test]
    fn test_steering_mode() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_steering_mode(), SteeringMode::OneAtATime);

        mgr.set_steering_mode(SteeringMode::All);
        assert_eq!(mgr.get_steering_mode(), SteeringMode::All);
    }

    #[test]
    fn test_compaction_settings() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert!(mgr.get_compaction_enabled());
        assert_eq!(mgr.get_compaction_reserve_tokens(), 16384);
        assert_eq!(mgr.get_compaction_keep_recent_tokens(), 20000);

        mgr.set_compaction_enabled(false);
        assert!(!mgr.get_compaction_enabled());
    }

    #[test]
    fn test_retry_settings() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert!(mgr.get_retry_enabled());

        mgr.set_retry_enabled(false);
        assert!(!mgr.get_retry_enabled());
    }

    #[test]
    fn test_terminal_settings() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert!(mgr.get_show_images());
        assert_eq!(mgr.get_image_width_cells(), 60);
        assert!(!mgr.get_clear_on_shrink());
        assert!(!mgr.get_show_terminal_progress());

        mgr.set_show_images(false);
        assert!(!mgr.get_show_images());

        mgr.set_image_width_cells(80);
        assert_eq!(mgr.get_image_width_cells(), 80);

        mgr.set_clear_on_shrink(true);
        assert!(mgr.get_clear_on_shrink());

        mgr.set_show_terminal_progress(true);
        assert!(mgr.get_show_terminal_progress());
    }

    #[test]
    fn test_image_settings() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert!(mgr.get_image_auto_resize());
        assert!(!mgr.get_block_images());

        mgr.set_image_auto_resize(false);
        assert!(!mgr.get_image_auto_resize());

        mgr.set_block_images(true);
        assert!(mgr.get_block_images());
    }

    #[test]
    fn test_double_escape_action() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_double_escape_action(), DoubleEscapeAction::Tree);

        mgr.set_double_escape_action(DoubleEscapeAction::Fork);
        assert_eq!(mgr.get_double_escape_action(), DoubleEscapeAction::Fork);
    }

    #[test]
    fn test_tree_filter_mode() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_tree_filter_mode(), TreeFilterMode::Default);

        mgr.set_tree_filter_mode(TreeFilterMode::All);
        assert_eq!(mgr.get_tree_filter_mode(), TreeFilterMode::All);
    }

    #[test]
    fn test_editor_padding_x() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_editor_padding_x(), 0);

        mgr.set_editor_padding_x(2);
        assert_eq!(mgr.get_editor_padding_x(), 2);

        // Clamp to [0, 3]
        mgr.set_editor_padding_x(5);
        assert_eq!(mgr.get_editor_padding_x(), 3);
    }

    #[test]
    fn test_output_pad() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_output_pad(), OutputPad::One);

        mgr.set_output_pad(OutputPad::Zero);
        assert_eq!(mgr.get_output_pad(), OutputPad::Zero);
    }

    #[test]
    fn test_autocomplete_max_visible() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_autocomplete_max_visible(), 5);

        mgr.set_autocomplete_max_visible(10);
        assert_eq!(mgr.get_autocomplete_max_visible(), 10);

        // Clamp to [3, 20]
        mgr.set_autocomplete_max_visible(1);
        assert_eq!(mgr.get_autocomplete_max_visible(), 3);

        mgr.set_autocomplete_max_visible(30);
        assert_eq!(mgr.get_autocomplete_max_visible(), 20);
    }

    #[test]
    fn test_code_block_indent() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_code_block_indent(), "  ");
    }

    #[test]
    fn test_warnings() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        let warnings = mgr.get_warnings();
        assert!(warnings.anthropic_extra_usage.is_none());

        mgr.set_warnings(WarningSettings {
            anthropic_extra_usage: Some(false),
        });
        let warnings = mgr.get_warnings();
        assert_eq!(warnings.anthropic_extra_usage, Some(false));
    }

    #[test]
    fn test_enable_analytics_generates_tracking_id() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert!(!mgr.get_enable_analytics());
        assert!(mgr.get_tracking_id().is_none());

        mgr.set_enable_analytics(true);
        assert!(mgr.get_enable_analytics());
        assert!(mgr.get_tracking_id().is_some());
    }

    #[test]
    fn test_theme() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert!(mgr.get_theme_setting().is_none());

        mgr.set_theme("dark");
        assert_eq!(mgr.get_theme_setting(), Some("dark"));
        assert_eq!(mgr.get_theme(), Some("dark"));

        // Theme with "/" should return None from get_theme()
        mgr.set_theme("some/path/theme");
        assert_eq!(mgr.get_theme_setting(), Some("some/path/theme"));
        assert_eq!(mgr.get_theme(), None);
    }

    #[test]
    fn test_follow_up_mode() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_follow_up_mode(), FollowUpMode::OneAtATime);

        mgr.set_follow_up_mode(FollowUpMode::All);
        assert_eq!(mgr.get_follow_up_mode(), FollowUpMode::All);
    }

    #[test]
    fn test_transport() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        assert_eq!(mgr.get_transport(), TransportSetting::Auto);

        mgr.set_transport(TransportSetting::Websocket);
        assert_eq!(mgr.get_transport(), TransportSetting::Websocket);
    }

    #[test]
    fn test_reload() {
        let storage = Box::new(InMemorySettingsStorage::new());

        // Pre-populate storage
        storage.with_lock(
            SettingsScope::Global,
            Box::new(|_| Some(r#"{"defaultProvider":"anthropic"}"#.to_string())),
        );

        let mut mgr = SettingsManager::from_storage(storage);
        assert_eq!(mgr.get_default_provider(), Some("anthropic"));

        // Change in memory
        mgr.set_default_provider("openai");
        assert_eq!(mgr.get_default_provider(), Some("openai"));

        // Simulate external change to storage
        mgr.storage.with_lock(
            SettingsScope::Global,
            Box::new(|_| Some(r#"{"defaultProvider":"anthropic"}"#.to_string())),
        );

        // Reload should restore from storage
        mgr.reload();
        assert_eq!(mgr.get_default_provider(), Some("anthropic"));
    }

    #[test]
    fn test_drain_errors() {
        let storage = Box::new(InMemorySettingsStorage::new());
        let mut mgr = SettingsManager::new(storage, Settings::default(), Settings::default());

        let errors = mgr.drain_errors();
        assert!(errors.is_empty());
    }
}
