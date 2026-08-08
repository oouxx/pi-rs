pub mod action_bus;
pub mod api;
pub mod dispatcher;
pub mod types;
pub mod loader;
#[cfg(feature = "bun-runtime")]
pub mod bun;

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
// lives in `bun/` (方案 A, Bun subprocess).
pub use loader::{
    discover_extension_paths, discover_extensions_in_dir, is_extension_file, read_pi_manifest,
    resolve_extension_entries, CacheToken, DiscoveredExtensions, ExtensionCache, PiManifest,
};

/// Try to load a JS/TS extension immediately after installation.
///
/// When the `bun-runtime` feature is enabled, this discovers and loads the
/// extension at the given path, returning a human-readable result message.
/// When the `bun-runtime` feature is not enabled, returns an error explaining
/// that JS extensions require it.
///
/// Used by the CLI's `pi install` command to provide immediate feedback.
#[cfg(feature = "bun-runtime")]
pub async fn load_extension_now(
    source: &str,
    cwd: &str,
    agent_dir: &str,
) -> Result<String, String> {
    let extension_paths = vec![source.to_string()];
    let no_flags = std::collections::HashMap::new();
    match bun::load_bun_extensions(&extension_paths, cwd, agent_dir, &no_flags).await {
        Ok(Some(loaded)) => {
            let count = loaded.adapters.len();
            drop(loaded.runner);
            Ok(format!("Loaded {count} extension(s)"))
        }
        Ok(None) => Err("No extension files found at the installed path".to_string()),
        Err(errors) => Err(format!("Failed to load extension: {}", errors.join("; "))),
    }
}

/// Fallback: JS extensions cannot be loaded without the `bun-runtime` feature.
#[cfg(not(feature = "bun-runtime"))]
pub async fn load_extension_now(
    _source: &str,
    _cwd: &str,
    _agent_dir: &str,
) -> Result<String, String> {
    Err("JS extension loading requires building with the `bun-runtime` feature".to_string())
}
