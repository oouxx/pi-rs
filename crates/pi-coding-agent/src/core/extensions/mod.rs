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

/// 构造内置 Rust 扩展 registry（goal + subagent + web_search）。
///
/// 对应原版 `--no-extensions` 语义（resource-loader.ts：`noExtensions` 时
/// 只保留 CLI 显式路径，砍掉所有"发现的扩展"）：pi-rs 没有 JS 扩展运行时
/// 和安装/目录发现机制，"发现到的扩展"就是这三个内置注册的 Rust 扩展，
/// 因此 `enable=false` 时返回 `None`（无任何扩展工具进入 agent 工具列表）。
///
/// 所有 CLI 模式（print / interactive / acp / rpc）统一走此入口，不再各自
/// 手写注册块。
pub fn builtin_extension_registry(enable: bool) -> Option<ExtensionRegistry> {
    if !enable {
        return None;
    }
    let mut reg = ExtensionRegistry::new();
    reg.register(
        Box::new(pi_extensions::goal::GoalExtension::new()),
        create_builtin_source_info("goal"),
    );
    reg.register(
        Box::new(pi_extensions::subagent::SubagentExtension::new()),
        create_builtin_source_info("subagent"),
    );
    reg.register(
        Box::new(pi_extensions::web_search::WebSearchExtension::new()),
        create_builtin_source_info("web_search"),
    );
    Some(reg)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// `--no-extensions` 语义：禁用扩展发现 → registry 为 None。
    #[test]
    fn test_builtin_extension_registry_disabled() {
        assert!(builtin_extension_registry(false).is_none());
    }

    /// 默认注册全部三个内置 Rust 扩展（goal / subagent / web_search）。
    #[test]
    fn test_builtin_extension_registry_enabled() {
        let reg = builtin_extension_registry(true).expect("registry");
        let tools: Vec<&str> = reg
            .tools()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
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
}
