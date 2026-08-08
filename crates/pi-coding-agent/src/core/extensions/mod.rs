pub mod action_bus;
pub mod api;
pub mod dispatcher;
pub mod types;
pub mod loader;
#[cfg(feature = "js-runtime")]
pub mod js_runtime;

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

// Loader (V8-agnostic discovery + cache). The factory-invocation half is
// deferred to a future JS-runtime chunk; see EXTENSION_LOADING_FEASIBILITY.md.
pub use loader::{
    discover_extension_paths, discover_extensions_in_dir, is_extension_file, read_pi_manifest,
    resolve_extension_entries, CacheToken, DiscoveredExtensions, ExtensionCache, PiManifest,
};
#[cfg(feature = "js-runtime")]
pub mod js_shims;
#[cfg(feature = "js-runtime")]
pub mod js_adapter;

/// Try to load a JS/TS extension immediately after installation.
///
/// When the `js-runtime` feature is enabled, this discovers and loads the
/// extension at the given path, returning a human-readable result message.
/// When `js-runtime` is not enabled, returns an error explaining that JS
/// extensions require the js-runtime feature.
///
/// Used by the CLI's `pi install` command to provide immediate feedback.
#[cfg(feature = "js-runtime")]
pub async fn load_extension_now(
    source: &str,
    cwd: &str,
    agent_dir: &str,
) -> Result<String, String> {
    let extension_paths = vec![source.to_string()];
    match js_adapter::load_js_extensions(&extension_paths, cwd, agent_dir).await {
        Ok(Some(loaded)) => {
            let count = loaded.adapters.len();
            // Keep the manager alive briefly to ensure V8 shutdown is clean
            drop(loaded.manager);
            Ok(format!("Loaded {count} extension(s)"))
        }
        Ok(None) => {
            Err("No extension files found at the installed path".to_string())
        }
        Err(errors) => {
            Err(format!("Failed to load extension: {}", errors.join("; ")))
        }
    }
}

/// Non-V8 fallback: JS extensions cannot be loaded without the js-runtime feature.
#[cfg(not(feature = "js-runtime"))]
pub async fn load_extension_now(
    _source: &str,
    _cwd: &str,
    _agent_dir: &str,
) -> Result<String, String> {
    Err("JS extension loading requires building with the `js-runtime` feature".to_string())
}
