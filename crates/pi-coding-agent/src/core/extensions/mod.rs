pub mod action_bus;
pub mod api;
pub mod dispatcher;
pub mod types;
pub mod loader;

pub use api::{
    ArgumentCompletionsFn, AutocompleteItem, CommandRegistry, CommandRegistration, EventPublisher,
    ExtensionContext, ExtensionRegistry, ExtensionUIContext, FlagRegistry, HookHandler, HookResult,
    HookRunner, RegisteredCommand, RegisteredFlag, RegisteredShortcut, RegisteredTool, RuntimeHandle,
    SendMessageOptions, SendUserMessageOptions, ShortcutRegistry, ToolCallOutput, ToolDefinition,
    ToolInfo, ToolRegistry,
};
pub use api::{create_builtin_source_info, create_source_info, create_synthetic_source_info, SourceInfo, SourceOrigin, SourceScope};
pub use api::ResourcesDiscoverResult;
pub use api::{ProjectTrustDecision, ProjectTrustResult, UserBashResult};

// Loader (runtime-agnostic discovery + cache). The factory-invocation half
// (loading TS/JS extension modules) is not implemented on main — see the
// `feat/bun-extension-compat` branch for the Bun subprocess runtime.
pub use loader::{
    discover_extension_paths, discover_extensions_in_dir, is_extension_file, read_pi_manifest,
    resolve_extension_entries, CacheToken, DiscoveredExtensions, ExtensionCache, PiManifest,
};

/// Wire payload for the extension `registerProvider` hook.
///
/// Mirrors the TS host call `runtime.registerProvider(providerId, config)`,
/// which receives the provider id separately from the config (the config's
/// `name` is only a display name).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterProviderPayload {
    provider_id: String,
    config: crate::core::model_registry::ProviderConfig,
}

/// Install the extension `registerProvider` hook on a runtime handle.
///
/// The payload is `{ "providerId": "...", "config": { ... } }`. Structurally
/// invalid registrations (bad payload shape, missing `api`/`baseUrl` for model
/// definitions) return an error instead of being silently dropped, matching TS
/// `validateExtensionProvider`, which throws.
pub fn install_register_provider_hook(
    handle: &mut RuntimeHandle,
    registry: crate::core::model_registry::ModelRegistry,
) {
    handle.register_provider = std::sync::Arc::new(move |payload| {
        let RegisterProviderPayload { provider_id, config } =
            serde_json::from_value(payload).map_err(|e| e.to_string())?;
        registry.register_provider(&provider_id, config)
    });
}

/// Try to load a JS/TS extension immediately after installation.
///
/// Main 分支不包含 JS 扩展运行时（V8 方案已移除，Bun 方案在
/// `feat/bun-extension-compat` 分支）。返回错误说明需要扩展运行时。
///
/// Used by the CLI's `pi install` command to provide immediate feedback.
pub async fn load_extension_now(
    _source: &str,
    _cwd: &str,
    _agent_dir: &str,
) -> Result<String, String> {
    Err("JS extension loading requires a JS extension runtime (see feat/bun-extension-compat branch)"
        .to_string())
}

/// Stable ids of the built-in Rust extensions, used as keys in
/// `extensionsEnabled` (settings.json).
pub const BUILTIN_EXTENSION_IDS: [&str; 3] = ["goal", "subagent", "web_search"];

/// 构造内置 Rust 扩展 registry（goal + subagent + web_search）。
///
/// `enable` 是 discovery 总开关（`--no-extensions`）：为 `false` 时返回
/// `None`，一个内置扩展都不注册。
///
/// `enabled` 是来自 settings `extensionsEnabled` 的逐扩展覆盖：
/// - id 缺失 → 启用（opt-out，新增内置扩展默认生效，不需改用户配置）
/// - `false` → 不注册该扩展
/// - `true` → 强制注册
///
/// 全关时返回 `Some(空 registry)` 而非 `None`：discovery 仍然开着，只是没有
/// 任何内置扩展。
pub fn builtin_extension_registry(
    enable: bool,
    enabled: &std::collections::HashMap<String, bool>,
) -> Option<ExtensionRegistry> {
    if !enable {
        return None;
    }
    let is_enabled = |id: &str| enabled.get(id).copied().unwrap_or(true);
    let mut reg = ExtensionRegistry::new();
    if is_enabled("goal") {
        reg.register(
            Box::new(pi_extensions::goal::GoalExtension::new()),
            create_builtin_source_info("goal"),
        );
    }
    if is_enabled("subagent") {
        reg.register(
            Box::new(pi_extensions::subagent::SubagentExtension::new()),
            create_builtin_source_info("subagent"),
        );
    }
    if is_enabled("web_search") {
        reg.register(
            Box::new(pi_extensions::web_search::WebSearchExtension::new()),
            create_builtin_source_info("web_search"),
        );
    }
    Some(reg)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `--no-extensions` 语义：禁用扩展发现 → registry 为 None。
    #[test]
    fn test_builtin_extension_registry_disabled() {
        assert!(builtin_extension_registry(false, &std::collections::HashMap::new()).is_none());
    }

    /// 默认注册全部三个内置 Rust 扩展（goal / subagent / web_search）。
    #[test]
    fn test_builtin_extension_registry_enabled() {
        let reg = builtin_extension_registry(true, &std::collections::HashMap::new())
            .expect("registry");
        let tools: Vec<&str> =
            reg.tools().iter().map(|t| t.name.as_str()).collect();
        for name in [
            "goal_complete",
            "goal_blocked",
            "goal_wait",
            "subagent",
            "web_search",
            "web_fetch",
        ] {
            assert!(tools.contains(&name), "missing tool {name}: {tools:?}");
        }
        // 来源标注正确（create_builtin_source_info -> `<builtin:{name}>`）
        for t in reg.tools() {
            let expected = match t.name.as_str() {
                "goal_complete" | "goal_blocked" | "goal_wait" => "<builtin:goal>",
                "subagent" => "<builtin:subagent>",
                "web_search" | "web_fetch" => "<builtin:web_search>",
                other => panic!("unexpected tool {other}"),
            };
            assert_eq!(t.source_info.path, expected);
        }
    }

    /// `extensionsEnabled` 可以单独关掉某个内置扩展，其余仍注册（opt-out）。
    #[test]
    fn test_builtin_extension_registry_respects_enabled_map() {
        let mut enabled = std::collections::HashMap::new();
        enabled.insert("goal".to_string(), false);
        let reg = builtin_extension_registry(true, &enabled).expect("registry");
        let tools: Vec<&str> = reg.tools().iter().map(|t| t.name.as_str()).collect();
        assert!(!tools.iter().any(|t| t.starts_with("goal_")), "goal must be off: {tools:?}");
        assert!(tools.contains(&"subagent"));
        assert!(tools.contains(&"web_search"));

        // 显式 true 覆盖默认（例如 project 想重新打开被 global 关掉的扩展）。
        let mut re_enabled = std::collections::HashMap::new();
        re_enabled.insert("goal".to_string(), true);
        let reg = builtin_extension_registry(true, &re_enabled).expect("registry");
        assert!(reg.tools().iter().any(|t| t.name == "goal_complete"));
    }

    /// 全部关闭时返回 `Some(空 registry)` 而不是 `None`（discovery 仍开着）。
    #[test]
    fn test_builtin_extension_registry_all_disabled_is_empty_not_none() {
        let enabled: std::collections::HashMap<String, bool> = BUILTIN_EXTENSION_IDS
            .iter()
            .map(|id| ((*id).to_string(), false))
            .collect();
        let reg = builtin_extension_registry(true, &enabled)
            .expect("registry must still exist");
        assert!(reg.tools().is_empty());
        assert_eq!(reg.handler_count(), 0);
    }

    /// The extension `registerProvider` hook keys the registration by
    /// `providerId` (TS `pi.registerProvider(name, config)`) and installs the
    /// supplied models — previously the payload's `models` were dropped and
    /// `config.name` was misused as the provider id.
    #[test]
    fn test_install_register_provider_hook_registers_models() {
        let mut handle = RuntimeHandle::noop();
        let registry = crate::core::model_registry::ModelRegistry::new_with_models_path(
            vec![],
            std::path::Path::new("/nonexistent/models.json"),
        );
        install_register_provider_hook(&mut handle, registry.clone());

        (handle.register_provider)(serde_json::json!({
            "providerId": "corp",
            "config": {
                "name": "Corporate AI",
                "apiKey": "k",
                "api": "openai-responses",
                "baseUrl": "https://ai.corp.example",
                "models": [{
                    "id": "corp-1",
                    "name": "Corp 1",
                    "reasoning": false,
                    "input": ["text"],
                    "cost": { "input": 0.0, "output": 0.0 },
                    "contextWindow": 1000,
                    "maxTokens": 100
                }]
            }
        }))
        .expect("hook must register the provider");

        assert!(registry.find("corp", "corp-1").is_some());
        assert!(registry.get_provider_config("corp").is_some());
        assert_eq!(
            registry
                .get_provider_config("corp")
                .and_then(|c| c.name)
                .as_deref(),
            Some("Corporate AI"),
            "config.name is a display name, not the provider id"
        );

        // Malformed payloads surface an error instead of being dropped.
        let error = (handle.register_provider)(serde_json::json!({"name": "legacy-shape"}))
            .expect_err("legacy payload must be rejected");
        assert!(error.contains("providerId"), "unexpected error: {error}");
    }
}
