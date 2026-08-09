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
