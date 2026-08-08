//! V8-backed runtime for loading TS/JS pi extensions at runtime.
//!
//! Feature-gated behind `js-runtime` (deno_core + deno_ast). This is the
//! factory-invocation half of TS `core/extensions/loader.ts`; the
//! V8-agnostic discovery/cache half lives in `loader.rs`.
//!
//! ## Architecture
//!
//! The TS `createExtensionAPI` surface is split into two halves:
//!
//! 1. **Registration methods** (`on`, `registerTool`, `registerCommand`, …):
//!    The bootstrap JS maintains JS-side `Map`s of callback functions
//!    (handlers, tool execute fns, command handlers, …) **and** calls a
//!    Rust op to record the metadata (name, description, parameter schema,
//!    source info) so the existing `ExtensionRegistry` / `CommandRegistry`
//!    can query it from Rust.
//!
//! 2. **Action methods** (`sendMessage`, `exec`, `setModel`, …): These
//!    delegate to a shared `RuntimeActions` struct stored in `OpState`.
//!    Before `bind_core()` the struct is `None` and ops throw
//!    "Extension runtime not initialized" — mirroring TS
//!    `createExtensionRuntime()`'s `notInitialized` placeholder. After
//!    `bind_core()` the struct is `Some(..)` and ops delegate.
//!
//! Callbacks (tool execute, event handlers, command handlers) stay in JS
//! land; Rust calls back into JS via `__pi.__invoke*` shim functions when
//! it needs to execute them.

#![cfg(feature = "js-runtime")]

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use deno_ast::MediaType;
use deno_ast::ParseParams;
use deno_ast::SourceMapOption;
use deno_core::anyhow;
use deno_core::error::ModuleLoaderError;
use deno_core::op2;
use deno_core::resolve_import;
use deno_core::resolve_path;
use deno_core::Extension;
use deno_core::JsRuntime;
use deno_core::ModuleLoadOptions;
use deno_core::ModuleLoadReferrer;
use deno_core::ModuleLoadResponse;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceCode;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::OpDecl;
use deno_core::OpState;
use deno_core::ResolutionKind;
use deno_core::RuntimeOptions;
use deno_error::JsErrorBox;
use crate::core::extensions::js_shims;
use serde::{Deserialize, Serialize};

// ============================================================================
// Load result — what a factory invocation leaves in Rust-side state
// ============================================================================

/// A tool that a loaded extension registered via `pi.registerTool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedToolRecord {
    pub name: String,
    pub description: String,
    /// JSON-serialized parameter schema (typebox `TSchema`).
    #[serde(default)]
    pub parameters: Option<String>,
}

/// A command registered via `pi.registerCommand`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedCommandRecord {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Names of sub-commands (TS `subcommands`).
    #[serde(default)]
    pub subcommands: Vec<String>,
}

/// A shortcut registered via `pi.registerShortcut`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedShortcutRecord {
    pub shortcut: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A flag registered via `pi.registerFlag`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedFlagRecord {
    pub name: String,
    pub flag_type: String, // "boolean" | "string"
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_value: Option<String>,
}

/// An event handler registered via `pi.on`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LoadedHandlerRecord {
    pub event: String,
}

/// A provider registration queued via `pi.registerProvider` (pre-bind).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PendingProviderRegistration {
    pub name: String,
    /// JSON-serialized `ProviderConfig`.
    pub config_json: String,
    pub extension_path: String,
}

/// Everything a loaded extension factory registered into Rust state.
#[derive(Debug, Clone, Default)]
pub struct ExtensionLoadResult {
    pub tools: Vec<LoadedToolRecord>,
    pub commands: Vec<LoadedCommandRecord>,
    pub shortcuts: Vec<LoadedShortcutRecord>,
    pub flags: Vec<LoadedFlagRecord>,
    pub handlers: Vec<LoadedHandlerRecord>,
    pub message_renderers: Vec<String>,
    pub entry_renderers: Vec<String>,
    pub pending_providers: Vec<PendingProviderRegistration>,
    pub logs: Vec<String>,
}

// ============================================================================
// Two-phase lifecycle — mirrors TS createExtensionRuntime / bindCore
// ============================================================================

/// The phase of the shared runtime. Before `bind_core()` action ops throw
/// "not initialized"; after, they delegate to the bound actions.
#[derive(Default)]
enum RuntimePhase {
    /// Placeholder: all action methods throw (TS `notInitialized`).
    #[default]
    Uninitialized,
    /// Bound: action methods delegate to the closures.
    Bound(RuntimeActions),
}

/// Action-method closures, set by `bind_core()`. Mirrors TS
/// `ExtensionActions` — each field is an `Arc<dyn Fn>` that the
/// corresponding op calls into.
#[derive(Default, Clone)]
pub struct RuntimeActions {
    pub send_message: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub send_user_message: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub append_entry: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub set_session_name: Option<Arc<dyn Fn(String) + Send + Sync>>,
    pub get_session_name: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    pub set_label: Option<Arc<dyn Fn(String, Option<String>) + Send + Sync>>,
    pub get_active_tools: Option<Arc<dyn Fn() -> Vec<String> + Send + Sync>>,
    /// Serialized `ToolInfo` objects, matching TS `getAllTools()`.
    pub get_all_tools: Option<Arc<dyn Fn() -> Vec<serde_json::Value> + Send + Sync>>,
    pub set_active_tools: Option<Arc<dyn Fn(Vec<String>) + Send + Sync>>,
    /// Serialized `SlashCommandInfo` objects, matching TS `getCommands()`.
    pub get_commands: Option<Arc<dyn Fn() -> Vec<serde_json::Value> + Send + Sync>>,
    pub get_thinking_level: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    pub set_thinking_level: Option<Arc<dyn Fn(String) + Send + Sync>>,
    /// `provider/model` string, resolved + applied at the next drain point.
    pub set_model: Option<Arc<dyn Fn(String) + Send + Sync>>,
    pub register_provider:
        Option<Arc<dyn Fn(String, String, String) + Send + Sync>>,
    pub unregister_provider: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

/// Error returned when an action op is called before `bind_core()`.
fn not_initialized() -> JsErrorBox {
    JsErrorBox::generic(
        "Extension runtime not initialized. Action methods cannot be called during extension loading.",
    )
}

// ============================================================================
// Ops — the Rust side of the `pi` API surface
// ============================================================================

// --- Registration ops (valid during load) ---

#[op2]
fn op_register_tool(
    state: &mut OpState,
    #[string] name: String,
    #[string] description: String,
    #[string] parameters: Option<String>,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.tools.push(LoadedToolRecord {
        name,
        description,
        parameters,
    });
    state.put(result);
    Ok(())
}

#[op2]
fn op_register_command(
    state: &mut OpState,
    #[string] name: String,
    #[string] description: Option<String>,
    #[string] subcommands: String, // JSON array
) -> Result<(), JsErrorBox> {
    let subs: Vec<String> = if subcommands == "[]" {
        Vec::new()
    } else {
        serde_json::from_str(&subcommands).unwrap_or_default()
    };
    let mut result = take_result(state);
    result.commands.push(LoadedCommandRecord {
        name,
        description,
        subcommands: subs,
    });
    state.put(result);
    Ok(())
}

#[op2]
fn op_register_shortcut(
    state: &mut OpState,
    #[string] shortcut: String,
    #[string] description: Option<String>,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.shortcuts.push(LoadedShortcutRecord {
        shortcut,
        description,
    });
    state.put(result);
    Ok(())
}

#[op2]
fn op_register_flag(
    state: &mut OpState,
    #[string] name: String,
    #[string] flag_type: String,
    #[string] description: Option<String>,
    #[string] default_value: Option<String>,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.flags.push(LoadedFlagRecord {
        name,
        flag_type,
        description,
        default_value,
    });
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_register_handler(
    state: &mut OpState,
    #[string] event: String,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.handlers.push(LoadedHandlerRecord { event });
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_register_message_renderer(
    state: &mut OpState,
    #[string] custom_type: String,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.message_renderers.push(custom_type);
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_register_entry_renderer(
    state: &mut OpState,
    #[string] custom_type: String,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.entry_renderers.push(custom_type);
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_register_provider(
    state: &mut OpState,
    #[string] name: String,
    #[string] config_json: String,
    #[string] extension_path: String,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result
        .pending_providers
        .push(PendingProviderRegistration {
            name,
            config_json,
            extension_path,
        });
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_unregister_provider(
    state: &mut OpState,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result
        .pending_providers
        .retain(|r| r.name != name);
    state.put(result);
    Ok(())
}

#[op2(fast)]
fn op_pi_log(state: &mut OpState, #[string] msg: String) -> Result<(), JsErrorBox> {
    let mut result = take_result(state);
    result.logs.push(msg);
    state.put(result);
    Ok(())
}

// --- Action ops (delegate to RuntimeActions after bind_core) ---

#[op2]
fn op_send_message(
    state: &mut OpState,
    #[string] message_json: String,
    #[string] options_json: Option<String>,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let send = actions
        .send_message
        .as_ref()
        .ok_or_else(not_initialized)?;
    send(message_json, options_json);
    Ok(())
}

#[op2]
fn op_send_user_message(
    state: &mut OpState,
    #[string] content: String,
    #[string] options_json: Option<String>,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let send = actions
        .send_user_message
        .as_ref()
        .ok_or_else(not_initialized)?;
    send(content, options_json);
    Ok(())
}

#[op2]
fn op_append_entry(
    state: &mut OpState,
    #[string] custom_type: String,
    #[string] data_json: Option<String>,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let append = actions
        .append_entry
        .as_ref()
        .ok_or_else(not_initialized)?;
    append(custom_type, data_json);
    Ok(())
}

#[op2(fast)]
fn op_set_session_name(
    state: &mut OpState,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let set = actions
        .set_session_name
        .as_ref()
        .ok_or_else(not_initialized)?;
    set(name);
    Ok(())
}

#[op2]
#[string]
fn op_get_session_name(state: &mut OpState) -> Result<Option<String>, JsErrorBox> {
    let actions = bound_actions(state)?;
    let get = actions
        .get_session_name
        .as_ref()
        .ok_or_else(not_initialized)?;
    Ok(get())
}

#[op2]
fn op_set_label(
    state: &mut OpState,
    #[string] entry_id: String,
    #[string] label: Option<String>,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let set = actions.set_label.as_ref().ok_or_else(not_initialized)?;
    set(entry_id, label);
    Ok(())
}

#[op2]
#[string]
fn op_get_active_tools(state: &mut OpState) -> Result<String, JsErrorBox> {
    let actions = bound_actions(state)?;
    let get = actions
        .get_active_tools
        .as_ref()
        .ok_or_else(not_initialized)?;
    let tools = get();
    Ok(serde_json::to_string(&tools).unwrap_or_else(|_| "[]".into()))
}

#[op2]
#[string]
fn op_get_all_tools(state: &mut OpState) -> Result<String, JsErrorBox> {
    let actions = bound_actions(state)?;
    let get = actions
        .get_all_tools
        .as_ref()
        .ok_or_else(not_initialized)?;
    let tools = get();
    Ok(serde_json::to_string(&tools).unwrap_or_else(|_| "[]".into()))
}

#[op2(fast)]
fn op_set_active_tools(
    state: &mut OpState,
    #[string] tools_json: String,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let set = actions
        .set_active_tools
        .as_ref()
        .ok_or_else(not_initialized)?;
    let tools: Vec<String> = serde_json::from_str(&tools_json).unwrap_or_default();
    set(tools);
    Ok(())
}

#[op2]
#[string]
fn op_get_commands(state: &mut OpState) -> Result<String, JsErrorBox> {
    let actions = bound_actions(state)?;
    let get = actions
        .get_commands
        .as_ref()
        .ok_or_else(not_initialized)?;
    let cmds = get();
    Ok(serde_json::to_string(&cmds).unwrap_or_else(|_| "[]".into()))
}

#[op2]
#[string]
fn op_get_thinking_level(state: &mut OpState) -> Result<String, JsErrorBox> {
    let actions = bound_actions(state)?;
    let get = actions
        .get_thinking_level
        .as_ref()
        .ok_or_else(not_initialized)?;
    Ok(get())
}

#[op2(fast)]
fn op_set_thinking_level(
    state: &mut OpState,
    #[string] level: String,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let set = actions
        .set_thinking_level
        .as_ref()
        .ok_or_else(not_initialized)?;
    set(level);
    Ok(())
}

#[op2(fast)]
fn op_set_model(state: &mut OpState, #[string] model_id: String) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let set = actions.set_model.as_ref().ok_or_else(not_initialized)?;
    // Fire-and-forget: the model is resolved + applied at the session's next
    // drain point (turn boundary). The JS side resolves optimistically.
    set(model_id);
    Ok(())
}

#[op2(fast)]
fn op_register_provider_action(
    state: &mut OpState,
    #[string] name: String,
    #[string] config_json: String,
    #[string] extension_path: String,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let register = actions
        .register_provider
        .as_ref()
        .ok_or_else(not_initialized)?;
    register(name, config_json, extension_path);
    Ok(())
}

#[op2(fast)]
fn op_unregister_provider_action(
    state: &mut OpState,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    let actions = bound_actions(state)?;
    let unregister = actions
        .unregister_provider
        .as_ref()
        .ok_or_else(not_initialized)?;
    unregister(name);
    Ok(())
}

// ============================================================================
// Node.js built-in module ops
// ============================================================================

#[op2(fast)]
fn op_fs_exists_sync(#[string] path: String) -> bool {
    std::path::Path::new(&path).exists()
}

#[op2]
#[string]
fn op_fs_read_file_sync(#[string] path: String) -> Result<String, JsErrorBox> {
    std::fs::read_to_string(&path)
        .map_err(|e| JsErrorBox::generic(format!("ENOENT: {e}")))
}

#[op2(fast)]
fn op_fs_write_file_sync(
    #[string] path: String,
    #[string] data: String,
) -> Result<(), JsErrorBox> {
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, &data).map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))
}

#[op2(fast)]
fn op_fs_append_file_sync(
    #[string] path: String,
    #[string] data: String,
) -> Result<(), JsErrorBox> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))?;
    file.write_all(data.as_bytes())
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))
}

#[op2(fast)]
fn op_fs_mkdir_sync(
    #[string] path: String,
    recursive: bool,
) -> Result<(), JsErrorBox> {
    if recursive {
        std::fs::create_dir_all(&path)
    } else {
        std::fs::create_dir(&path)
    }
    .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))
}

#[op2]
#[string]
fn op_fs_readdir_sync(#[string] path: String) -> Result<String, JsErrorBox> {
    let entries: Vec<String> = std::fs::read_dir(&path)
        .map_err(|e| JsErrorBox::generic(format!("ENOENT: {e}")))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    serde_json::to_string(&entries)
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))
}

#[op2]
#[string]
fn op_fs_stat_sync(#[string] path: String) -> Result<String, JsErrorBox> {
    let meta = std::fs::metadata(&path)
        .map_err(|e| JsErrorBox::generic(format!("ENOENT: {e}")))?;
    let stat = serde_json::json!({
        "size": meta.len(),
        "isFile": meta.is_file(),
        "isDirectory": meta.is_dir(),
        "isSymlink": meta.is_symlink(),
        "mode": 0,
        "mtimeMs": meta.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as f64),
    });
    serde_json::to_string(&stat)
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))
}

#[op2(fast)]
fn op_fs_unlink_sync(#[string] path: String) -> Result<(), JsErrorBox> {
    std::fs::remove_file(&path)
        .map_err(|e| JsErrorBox::generic(format!("ENOENT: {e}")))
}

#[op2(fast)]
fn op_fs_rm_sync(
    #[string] path: String,
    recursive: bool,
) -> Result<(), JsErrorBox> {
    let meta = std::fs::metadata(&path);
    if let Ok(m) = meta {
        if m.is_dir() && recursive {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        }
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))
    } else {
        Err(JsErrorBox::generic(format!("ENOENT: {path}")))
    }
}

#[op2(fast)]
fn op_fs_copy_file_sync(
    #[string] src: String,
    #[string] dest: String,
) -> Result<(), JsErrorBox> {
    std::fs::copy(&src, &dest)
        .map_err(|e| JsErrorBox::generic(format!("ENOENT: {e}")))?;
    Ok(())
}

#[op2(fast)]
fn op_fs_rename_sync(
    #[string] old: String,
    #[string] new: String,
) -> Result<(), JsErrorBox> {
    std::fs::rename(&old, &new)
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))
}

#[op2(fast)]
fn op_fs_access_sync(#[string] path: String) -> bool {
    std::path::Path::new(&path).exists()
}

#[op2]
#[string]
fn op_fs_mkdtemp_sync(#[string] prefix: String) -> Result<String, JsErrorBox> {
    let dir = std::env::temp_dir().join(&prefix);
    std::fs::create_dir_all(&dir)
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))?;
    Ok(dir.to_string_lossy().to_string())
}

#[op2]
#[string]
fn op_cp_exec_sync(#[string] command: String) -> Result<String, JsErrorBox> {
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output()
        .map_err(|e| JsErrorBox::generic(format!("EIO: {e}")))?;
    let result = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = if stderr.is_empty() { result } else { format!("{result}\n{stderr}") };
    Ok(combined)
}

// --- Helpers ---

/// Take the accumulated load result out of `OpState`, leaving an empty one.
fn take_result(state: &mut OpState) -> ExtensionLoadResult {
    state.try_take::<ExtensionLoadResult>().unwrap_or_default()
}

/// Get the bound `RuntimeActions` from `OpState`, or error if uninitialized.
fn bound_actions(state: &mut OpState) -> Result<RuntimeActions, JsErrorBox> {
    let phase = state
        .try_borrow::<RuntimePhase>()
        .ok_or_else(|| JsErrorBox::generic("Runtime phase not set"))?;
    match phase {
        RuntimePhase::Uninitialized => Err(not_initialized()),
        RuntimePhase::Bound(actions) => Ok(actions.clone()),
    }
}

/// The op declarations exposed to JS as `Deno.core.ops.op_*`.
const OPS: &[OpDecl] = &[
    op_register_tool(),
    op_register_command(),
    op_register_shortcut(),
    op_register_flag(),
    op_register_handler(),
    op_register_message_renderer(),
    op_register_entry_renderer(),
    op_register_provider(),
    op_unregister_provider(),
    op_pi_log(),
    op_send_message(),
    op_send_user_message(),
    op_append_entry(),
    op_set_session_name(),
    op_get_session_name(),
    op_set_label(),
    op_get_active_tools(),
    op_get_all_tools(),
    op_set_active_tools(),
    op_get_commands(),
    op_get_thinking_level(),
    op_set_thinking_level(),
    op_set_model(),
    op_register_provider_action(),
    op_unregister_provider_action(),
    // Node.js built-in module ops
    op_fs_exists_sync(),
    op_fs_read_file_sync(),
    op_fs_write_file_sync(),
    op_fs_append_file_sync(),
    op_fs_mkdir_sync(),
    op_fs_readdir_sync(),
    op_fs_stat_sync(),
    op_fs_unlink_sync(),
    op_fs_rm_sync(),
    op_fs_copy_file_sync(),
    op_fs_rename_sync(),
    op_fs_access_sync(),
    op_fs_mkdtemp_sync(),
    op_cp_exec_sync(),
];

fn pi_extension() -> Extension {
    Extension {
        name: "pi_ext",
        ops: Cow::Borrowed(OPS),
        ..Default::default()
    }
}

/// JS bootstrap that builds the `globalThis.__pi` API object wrapping the ops.
/// Mirrors the registration + action surface of TS `createExtensionAPI`.
///
/// Callbacks (tool execute, event handlers, command handlers) are stored in
/// JS-side `Map`s so Rust can call back into JS via `__pi.__invoke*` helpers.
const BOOTSTRAP_JS: &str = r#"
(function() {
  // JS-side callback storage — keeps Function refs alive for later invocation.
  const handlers = new Map();       // event -> Function[]
  const toolExecutors = new Map();  // toolName -> execute fn
  const commandHandlers = new Map();// commandName -> handler fn
  const shortcutHandlers = new Map();// shortcut -> handler fn
  const flagValues = new Map();     // flagName -> value

  function assertActive() {
    // The stale-ctx check is done Rust-side via the phase; if the runtime
    // has been invalidated, ops will throw. Nothing to check in JS here.
  }

  globalThis.__pi = {
    // ---- Logging (not part of TS ExtensionAPI, used for diagnostics) ----

    log(msg) {
      Deno.core.ops.op_pi_log(String(msg));
    },

    // ---- Registration methods ----

    on(event, handler) {
      assertActive();
      const list = handlers.get(event) ?? [];
      list.push(handler);
      handlers.set(event, list);
      Deno.core.ops.op_register_handler(String(event));
    },

    registerTool(tool) {
      assertActive();
      const name = String(tool.name);
      toolExecutors.set(name, tool.execute);
      const params = tool.parameters ? JSON.stringify(tool.parameters) : null;
      Deno.core.ops.op_register_tool(
        name,
        String(tool.description ?? ""),
        params,
      );
    },

    registerCommand(name, options = {}) {
      assertActive();
      commandHandlers.set(name, options.handler);
      const subs = JSON.stringify(options.subcommands ?? []);
      Deno.core.ops.op_register_command(
        String(name),
        options.description ? String(options.description) : null,
        subs,
      );
    },

    registerShortcut(shortcut, options = {}) {
      assertActive();
      shortcutHandlers.set(String(shortcut), options.handler);
      Deno.core.ops.op_register_shortcut(
        String(shortcut),
        options.description ? String(options.description) : null,
      );
    },

    registerFlag(name, options = {}) {
      assertActive();
      const def = options.default !== undefined ? String(options.default) : null;
      if (def !== null && !flagValues.has(name)) {
        flagValues.set(name, options.default);
      }
      Deno.core.ops.op_register_flag(
        String(name),
        String(options.type ?? "boolean"),
        options.description ? String(options.description) : null,
        def,
      );
    },

    getFlag(name) {
      assertActive();
      return flagValues.get(name);
    },

    registerMessageRenderer(customType, renderer) {
      assertActive();
      Deno.core.ops.op_register_message_renderer(String(customType));
    },

    registerEntryRenderer(customType, renderer) {
      assertActive();
      Deno.core.ops.op_register_entry_renderer(String(customType));
    },

    // ---- Action methods (delegate to Rust ops → RuntimeActions) ----

    sendMessage(message, options) {
      assertActive();
      Deno.core.ops.op_send_message(
        JSON.stringify(message),
        options ? JSON.stringify(options) : null,
      );
    },

    sendUserMessage(content, options) {
      assertActive();
      const c = typeof content === "string" ? content : JSON.stringify(content);
      Deno.core.ops.op_send_user_message(
        c,
        options ? JSON.stringify(options) : null,
      );
    },

    appendEntry(customType, data) {
      assertActive();
      Deno.core.ops.op_append_entry(
        String(customType),
        data !== undefined ? JSON.stringify(data) : null,
      );
    },

    setSessionName(name) {
      assertActive();
      Deno.core.ops.op_set_session_name(String(name));
    },

    getSessionName() {
      assertActive();
      return Deno.core.ops.op_get_session_name();
    },

    setLabel(entryId, label) {
      assertActive();
      Deno.core.ops.op_set_label(String(entryId), label != null ? String(label) : null);
    },

    getActiveTools() {
      assertActive();
      return JSON.parse(Deno.core.ops.op_get_active_tools());
    },

    getAllTools() {
      assertActive();
      return JSON.parse(Deno.core.ops.op_get_all_tools());
    },

    setActiveTools(toolNames) {
      assertActive();
      Deno.core.ops.op_set_active_tools(JSON.stringify(toolNames));
    },

    getCommands() {
      assertActive();
      return JSON.parse(Deno.core.ops.op_get_commands());
    },

    setModel(model) {
      assertActive();
      // Fire-and-forget: the op queues the model change; the session applies
      // it at the next turn boundary. The promise resolves optimistically —
      // the actual result (auth check) surfaces via getModel/state reads.
      const id = (model && (model.provider + "/" + model.id)) || String(model);
      Deno.core.ops.op_set_model(String(id));
      return Promise.resolve(true);
    },

    getThinkingLevel() {
      assertActive();
      return Deno.core.ops.op_get_thinking_level();
    },

    setThinkingLevel(level) {
      assertActive();
      Deno.core.ops.op_set_thinking_level(String(level));
    },

    registerProvider(name, config) {
      assertActive();
      // Pre-bind: queue via op_register_provider; post-bind: call the action
      // op directly. The Rust side decides based on the runtime phase.
      Deno.core.ops.op_register_provider(
        String(name),
        JSON.stringify(config),
        String(globalThis.__piExtensionPath ?? "<unknown>"),
      );
    },

    unregisterProvider(name) {
      assertActive();
      Deno.core.ops.op_unregister_provider(String(name));
    },

    // ---- EventBus (events property) ----
    events: {
      on(event, handler) {
        assertActive();
        const list = handlers.get(event) ?? [];
        list.push(handler);
        handlers.set(event, list);
      },
      emit(event, ...args) {
        const list = handlers.get(event);
        if (list) for (const h of list) h(...args);
      },
      off(event, handler) {
        const list = handlers.get(event);
        if (!list) return;
        if (handler) {
          handlers.set(event, list.filter(h => h !== handler));
        } else {
          handlers.delete(event);
        }
      },
    },

    // ---- Internal: Rust calls these to invoke JS callbacks ----
    __invokeToolExecutor: null,  // set by load_extension
    __invokeCommandHandler: null,
    __invokeEventHandler: null,
  };

  // Store the maps on __pi for later Rust-side callback invocation.
  globalThis.__pi.__handlers = handlers;
  globalThis.__pi.__toolExecutors = toolExecutors;
  globalThis.__pi.__commandHandlers = commandHandlers;
  globalThis.__pi.__shortcutHandlers = shortcutHandlers;
})();
"#;

// ============================================================================
// TS module loader (swc transpile, no typecheck — like Deno --no-check)
// ============================================================================

type SourceMapStore = Rc<RefCell<HashMap<String, Vec<u8>>>>;

/// Module loader that reads `.ts`/`.js` files from disk and transpiles TS via
/// swc (deno_ast). Mirrors the `jiti.import` transpile step; no typechecking.
struct TypescriptModuleLoader {
    source_maps: SourceMapStore,
}

impl TypescriptModuleLoader {
    fn new() -> Self {
        Self {
            source_maps: Rc::new(RefCell::new(HashMap::new())),
        }
    }
}

impl ModuleLoader for TypescriptModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        // Intercept bare specifiers (typebox, @earendil-works/*) and resolve
        // them to synthetic pi-shim:// URLs so the load() step can return
        // embedded JS shims instead of hitting the filesystem.
        if let Some(shim_path) = js_shims::lookup_shim(specifier) {
            return ModuleSpecifier::parse(&js_shims::shim_url(shim_path))
                .map_err(|e| JsErrorBox::generic(format!("invalid shim specifier: {e}")));
        }
        resolve_import(specifier, referrer).map_err(JsErrorBox::from_err)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        // Shim modules: return embedded JS source directly (no disk read).
        let url = module_specifier.as_str();
        if js_shims::is_shim_specifier(url) {
            let shim_path = &url[js_shims::SHIM_SCHEME.len() + 3..]; // skip "pi-shim://"
            let source = js_shims::shim_source(shim_path)
                .unwrap_or("export default {}");
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(source.to_string().into()),
                module_specifier,
                None,
            )));
        }
        let source_maps = self.source_maps.clone();
        ModuleLoadResponse::Sync(load(source_maps, module_specifier))
    }

    fn get_source_map(&self, specifier: &str) -> Option<Cow<'_, [u8]>> {
        self.source_maps
            .borrow()
            .get(specifier)
            .map(|v| Cow::Owned(v.clone()))
    }
}

/// Read + (if TS) transpile a module from disk. Free function so it can be
/// returned directly inside `ModuleLoadResponse::Sync`.
fn load(
    source_maps: SourceMapStore,
    module_specifier: &ModuleSpecifier,
) -> Result<ModuleSource, ModuleLoaderError> {
    let path = module_specifier
        .to_file_path()
        .map_err(|_| JsErrorBox::generic("Only file:// URLs are supported."))?;
    let media_type = MediaType::from_path(&path);
    let (module_type, should_transpile) = match media_type {
        MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs => {
            (ModuleType::JavaScript, false)
        }
        MediaType::Jsx => (ModuleType::JavaScript, true),
        MediaType::TypeScript
        | MediaType::Mts
        | MediaType::Cts
        | MediaType::Dts
        | MediaType::Dmts
        | MediaType::Dcts
        | MediaType::Tsx => (ModuleType::JavaScript, true),
        MediaType::Json => (ModuleType::Json, false),
        _ => {
            return Err(JsErrorBox::generic(format!(
                "Unknown extension {:?}",
                path.extension()
            )));
        }
    };

    let code = std::fs::read_to_string(&path).map_err(JsErrorBox::from_err)?;
    let code = if should_transpile {
        let parsed = deno_ast::parse_module(ParseParams {
            specifier: module_specifier.clone(),
            text: code.into(),
            media_type,
            capture_tokens: false,
            scope_analysis: false,
            maybe_syntax: None,
        })
        .map_err(JsErrorBox::from_err)?;
        let res = parsed
            .transpile(
                &deno_ast::TranspileOptions {
                    imports_not_used_as_values: deno_ast::ImportsNotUsedAsValues::Remove,
                    ..Default::default()
                },
                &deno_ast::TranspileModuleOptions {
                    module_kind: None,
                },
                &deno_ast::EmitOptions {
                    source_map: SourceMapOption::Separate,
                    inline_sources: true,
                    ..Default::default()
                },
            )
            .map_err(JsErrorBox::from_err)?;
        let res = res.into_source();
        if let Some(sm) = res.source_map {
            source_maps
                .borrow_mut()
                .insert(module_specifier.to_string(), sm.into_bytes());
        }
        res.text
    } else {
        code
    };
    Ok(ModuleSource::new(
        module_type,
        ModuleSourceCode::String(code.into()),
        module_specifier,
        None,
    ))
}

/// JS that sets up Node.js-compatible globals (`process`, `__piHomeDir`, etc.)
/// so extensions that use `process.cwd()`, `process.env`, etc. as globals (not
/// via `import`) work without a full Node.js runtime.
const NODE_GLOBALS_JS: &str = r#"
(function() {
  const homeDir = globalThis.__piHomeDir || ".";
  const cwd = globalThis.__piCwd || ".";
  const platform = globalThis.__piPlatform || "linux";

  // Minimal `process` global — provides cwd(), env, platform, stdout.write().
  globalThis.process = globalThis.process || {
    cwd() { return cwd; },
    env: globalThis.__piEnv || {},
    platform,
    arch: globalThis.__piArch || "arm64",
    argv: globalThis.__piArgv || [],
    stdout: { write(s) { /* no-op in extension runtime */ } },
    stderr: { write(s) { /* no-op in extension runtime */ } },
    exit(code) { throw new Error("process.exit(" + code + ") is not available in the extension runtime."); },
  };

  // `globalThis.__dirname` and `__filename` — best-effort (set per-extension
  // during load_extension via __piExtensionPath).
  if (!globalThis.__dirname) {
    globalThis.__dirname = cwd;
  }
})();
"#;

// ============================================================================
// JsExtensionRuntime
// ============================================================================

/// A V8 runtime capable of loading and invoking a TS/JS extension's
/// default-export factory against a host-provided `pi` API object.
pub struct JsExtensionRuntime {
    runtime: JsRuntime,
}

impl JsExtensionRuntime {
    /// Create a new runtime with the `pi_ext` ops registered and the `__pi`
    /// bootstrap object installed. The runtime starts in the
    /// `Uninitialized` phase (action methods throw until `bind_core`).
    pub fn new() -> anyhow::Result<Self> {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![pi_extension()],
            module_loader: Some(Rc::new(TypescriptModuleLoader::new())),
            ..Default::default()
        });
        {
            let op_state = runtime.op_state();
            let mut state = op_state.borrow_mut();
            state.put(ExtensionLoadResult::default());
            state.put(RuntimePhase::Uninitialized);
        }
        runtime
            .execute_script("<pi-bootstrap>", BOOTSTRAP_JS)
            .map_err(|e| anyhow::anyhow!("bootstrap: {e}"))?;
        runtime
            .execute_script("<node-globals>", NODE_GLOBALS_JS)
            .map_err(|e| anyhow::anyhow!("node globals: {e}"))?;
        Ok(Self { runtime })
    }

    /// Transition the runtime from `Uninitialized` to `Bound`, installing
    /// the action-method closures. Mirrors TS `ExtensionRunner.bindCore()`.
    ///
    /// After this call, action ops delegate to the provided closures instead
    /// of throwing "not initialized".
    pub fn bind_core(&mut self, actions: RuntimeActions) {
        let op_state = self.runtime.op_state();
        op_state.borrow_mut().put(RuntimePhase::Bound(actions));
    }

    /// Invalidate the runtime context (stale-ctx guard). Mirrors TS
    /// `runtime.invalidate(message)`. After this, action ops throw a
    /// stale-ctx error.
    pub fn invalidate(&mut self) {
        let op_state = self.runtime.op_state();
        op_state.borrow_mut().put(RuntimePhase::Uninitialized);
    }

    /// Load and invoke the default-export factory of the TS/JS extension at
    /// `path` (resolved against `cwd`), passing `globalThis.__pi` as the API
    /// argument. Mirrors TS `jiti.import(path, { default: true })` followed by
    /// `factory(api)`.
    pub async fn load_extension(
        &mut self,
        path: &Path,
        cwd: &Path,
    ) -> anyhow::Result<()> {
        let ext_specifier = resolve_path(&path.to_string_lossy(), cwd)
            .map_err(|e| anyhow::anyhow!("resolve extension path: {e}"))?;
        // Set the extension path so registerProvider can include it.
        self.runtime
            .execute_script("<pi-set-path>", format!(
                "globalThis.__piExtensionPath = {path:?};",
                path = path.to_string_lossy()
            ))
            .map_err(|e| anyhow::anyhow!("set extension path: {e}"))?;
        // A shim module that imports the extension's default export and invokes
        // it with the host-provided `__pi` object. This mirrors jiti's
        // `{ default: true }` extraction + `factory(api)` call.
        let shim_code = format!(
            "import factory from \"{ext_url}\";\nawait factory(globalThis.__pi);\n",
            ext_url = ext_specifier.as_str()
        );
        // Use a file:// specifier (not ext:, which is reserved for deno_core
        // extensions) so the shim can freely import the extension's file:// URL.
        let shim_specifier = resolve_path("pi_loader/main.js", cwd)
            .map_err(|e| anyhow::anyhow!("invalid shim specifier: {e}"))?;

        let mod_id = self
            .runtime
            .load_main_es_module_from_code(&shim_specifier, shim_code)
            .await?;
        let result = self.runtime.mod_evaluate(mod_id);
        self.runtime.run_event_loop(Default::default()).await?;
        result.await?;
        Ok(())
    }

    /// Take the accumulated registration result out of the runtime's op state.
    pub fn take_result(&mut self) -> ExtensionLoadResult {
        self.runtime
            .op_state()
            .borrow_mut()
            .try_take::<ExtensionLoadResult>()
            .unwrap_or_default()
    }

    /// Run the V8 event loop to completion (for async JS callbacks).
    pub async fn run_event_loop(&mut self) -> anyhow::Result<()> {
        self.runtime
            .run_event_loop(Default::default())
            .await
            .map_err(|e| anyhow::anyhow!("V8 event loop: {e}"))?;
        Ok(())
    }

    /// Execute a JS string in the runtime (for calling back into JS callbacks).
    pub fn execute_script(&mut self, name: &str, code: &str) -> anyhow::Result<()> {
        self.runtime
            .execute_script(name.to_string(), code.to_string())
            .map_err(|e| anyhow::anyhow!("execute_script: {e}"))?;
        Ok(())
    }

    /// Execute an async JS expression, run the event loop to resolve any
    /// promises, and return the result as a JSON string.
    ///
    /// The `code` must be an expression that evaluates to a value (or a
    /// Promise). The result is stored in `globalThis.__piResult`, the event
    /// loop is pumped, and then `JSON.stringify(globalThis.__piResult)` is
    /// returned.
    ///
    /// # Errors
    /// Returns an error if the script throws, the event loop fails, or
    /// `JSON.stringify` fails (circular references).
    pub async fn execute_async_and_get_json(
        &mut self,
        name: &str,
        code: &str,
    ) -> anyhow::Result<String> {
        // Wrap the code: store result in __piResult, then run event loop.
        let wrapper = format!(
            "globalThis.__piResult = undefined;
             (async () => {{
               globalThis.__piResult = await ({code});
             }})()",
            code = code
        );
        self.runtime
            .execute_script(name.to_string(), wrapper)
            .map_err(|e| anyhow::anyhow!("execute_script: {e}"))?;
        self.runtime
            .run_event_loop(Default::default())
            .await
            .map_err(|e| anyhow::anyhow!("V8 event loop: {e}"))?;
        // Serialize the result to JSON.
        let json = self
            .runtime
            .execute_script(
                "<pi-result-json>".to_string(),
                "JSON.stringify(globalThis.__piResult ?? null)".to_string(),
            )
            .map_err(|e| anyhow::anyhow!("serialize result: {e}"))?;
        // Extract the JSON string from the v8::Global<v8::Value> using the
        // deno_core scope macro (creates a HandleScope + ContextScope) and the
        // v8 Value::to_rust_string_lossy helper. JSON.stringify always returns
        // a string (or throws for circular refs, caught above), so no cast is
        // needed — to_rust_string_lossy handles the toString conversion.
        let result = {
            deno_core::scope!(scope, self.runtime);
            let value = json.open(scope);
            value.to_rust_string_lossy(scope)
        };
        Ok(result)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    /// Drive a deno_core runtime: it needs a single-threaded tokio runtime
    /// with all features enabled for the event loop to make progress.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
            .block_on(future)
    }

    fn write_extension(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        let mut f = fs::File::create(&path).expect("create");
        f.write_all(body.as_bytes()).expect("write");
        path
    }

    #[test]
    fn test_v8_links_and_sync_op_works() {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![pi_extension()],
            ..Default::default()
        });
        runtime
            .op_state()
            .borrow_mut()
            .put(ExtensionLoadResult::default());
        runtime
            .execute_script("<sync-test>", "Deno.core.ops.op_pi_log('hello from v8');")
            .expect("execute_script");
        let result = runtime
            .op_state()
            .borrow_mut()
            .try_take::<ExtensionLoadResult>()
            .unwrap_or_default();
        assert_eq!(result.logs, vec!["hello from v8".to_string()]);
    }

    #[test]
    fn test_execute_async_and_get_json_simple() {
        let mut js = JsExtensionRuntime::new().expect("runtime");
        // A plain value expression (no promise) — should still resolve.
        let json = block_on(js.execute_async_and_get_json(
            "<simple>",
            "({ answer: 42, items: [1, 2, 3], nested: { ok: true } })",
        ))
        .expect("execute_async_and_get_json");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse json");
        assert_eq!(v["answer"], 42);
        assert_eq!(v["items"][2], 3);
        assert_eq!(v["nested"]["ok"], true);
    }

    #[test]
    fn test_execute_async_and_get_json_async_value() {
        let mut js = JsExtensionRuntime::new().expect("runtime");
        // An async expression that awaits a resolved promise.
        let json = block_on(js.execute_async_and_get_json(
            "<async>",
            "(async () => { await Promise.resolve(); return { done: true }; })()",
        ))
        .expect("execute_async_and_get_json");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse json");
        assert_eq!(v["done"], true);
    }

    #[test]
    fn test_execute_async_and_get_json_null_result() {
        let mut js = JsExtensionRuntime::new().expect("runtime");
        // A null result should serialize to "null".
        let json = block_on(js.execute_async_and_get_json("<null>", "null"))
            .expect("execute_async_and_get_json");
        assert_eq!(json, "null");
    }

    #[test]
    fn test_load_ts_extension_factory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "my-ext.ts",
            r#"
export default async function(pi) {
  pi.log("loading my-ext");
  pi.registerTool({ name: "search", description: "search the web", execute: async () => {} });
  pi.registerCommand("greet", { description: "say hi", handler: async () => {} });
}
"#,
        );

        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert_eq!(result.logs, vec!["loading my-ext".to_string()]);
        assert_eq!(
            result.tools,
            vec![LoadedToolRecord {
                name: "search".into(),
                description: "search the web".into(),
                parameters: None,
            }]
        );
        assert_eq!(
            result.commands,
            vec![LoadedCommandRecord {
                name: "greet".into(),
                description: Some("say hi".into()),
                subcommands: vec![],
            }]
        );
    }

    #[test]
    fn test_load_js_extension_factory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "plain.js",
            r#"
export default async function(pi) {
  pi.registerTool({ name: "t", description: "d", execute: async () => {} });
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "t");
    }

    #[test]
    fn test_extension_with_imports() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_extension(
            dir.path(),
            "util.ts",
            "export function upper(s) { return s.toUpperCase(); }",
        );
        let ext = write_extension(
            dir.path(),
            "main.ts",
            r#"
import { upper } from "./util.ts";
export default async function(pi) {
  pi.log(upper("loaded"));
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert_eq!(result.logs, vec!["LOADED".to_string()]);
    }

    #[test]
    fn test_full_registration_surface() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "full.ts",
            r#"
export default async function(pi) {
  pi.on("session_start", async () => {});
  pi.on("tool_call", async (e) => {});
  pi.registerTool({ name: "t1", description: "tool 1", execute: async () => {} });
  pi.registerCommand("cmd1", { description: "cmd 1", handler: async () => {} });
  pi.registerShortcut("ctrl+k", { description: "shortcut 1", handler: async () => {} });
  pi.registerFlag("verbose", { type: "boolean", default: false, description: "verbose mode" });
  pi.registerMessageRenderer("custom-msg", () => {});
  pi.registerEntryRenderer("custom-entry", () => {});
  pi.registerProvider("my-provider", { baseUrl: "http://localhost:8080" });
  pi.log("done");
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();

        assert_eq!(result.logs, vec!["done".to_string()]);
        assert_eq!(result.handlers.len(), 2);
        assert_eq!(result.handlers[0].event, "session_start");
        assert_eq!(result.handlers[1].event, "tool_call");
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "t1");
        assert_eq!(result.commands.len(), 1);
        assert_eq!(result.commands[0].name, "cmd1");
        assert_eq!(result.commands[0].description.as_deref(), Some("cmd 1"));
        assert_eq!(result.shortcuts.len(), 1);
        assert_eq!(result.shortcuts[0].shortcut, "ctrl+k");
        assert_eq!(result.flags.len(), 1);
        assert_eq!(result.flags[0].name, "verbose");
        assert_eq!(result.flags[0].flag_type, "boolean");
        assert_eq!(result.flags[0].default_value.as_deref(), Some("false"));
        assert_eq!(result.message_renderers, vec!["custom-msg".to_string()]);
        assert_eq!(result.entry_renderers, vec!["custom-entry".to_string()]);
        assert_eq!(result.pending_providers.len(), 1);
        assert_eq!(result.pending_providers[0].name, "my-provider");
        assert!(result.pending_providers[0].config_json.contains("localhost"));
    }

    #[test]
    fn test_action_method_before_bind_throws() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "actions.ts",
            r#"
export default async function(pi) {
  // Action methods should throw during load (before bind_core).
  try {
    pi.setSessionName("test");
    pi.log("SHOULD NOT REACH");
  } catch (e) {
    pi.log("correctly threw: " + e.message);
  }
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert!(result.logs[0].starts_with("correctly threw"));
        assert!(result.logs[0].contains("not initialized"));
    }

    #[test]
    fn test_action_method_after_bind_works() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "bound.ts",
            r#"
export default async function(pi) {
  // Just register; action methods will be tested via execute_script after bind.
  pi.log("loaded");
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let _ = js.take_result(); // clear load result

        // Track calls via a shared cell.
        let called = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let called_clone = called.clone();
        let actions = RuntimeActions {
            set_session_name: Some(Arc::new(move |name: String| {
                called_clone.lock().unwrap().push(format!("set_session_name({name})"));
            })),
            get_session_name: Some(Arc::new(|| Some("test-session".into()))),
            ..Default::default()
        };
        js.bind_core(actions);

        // Now action methods should work.
        js.execute_script(
            "<test-actions>",
            r#"
            globalThis.__pi.setSessionName("hello");
            const name = globalThis.__pi.getSessionName();
            if (name !== "test-session") throw new Error("expected test-session, got " + name);
            "#,
        )
        .expect("execute_script");

        let calls = called.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "set_session_name(hello)");
    }

    #[test]
    fn test_event_bus_on_emit_off() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "events.ts",
            r#"
export default async function(pi) {
  let received = [];
  const handler = (data) => { received.push(data); };
  pi.events.on("test-event", handler);
  pi.events.emit("test-event", "a");
  pi.events.emit("test-event", "b");
  pi.events.off("test-event", handler);
  pi.events.emit("test-event", "c"); // should not be received
  pi.log("received: " + received.join(","));
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();
        assert_eq!(result.logs, vec!["received: a,b".to_string()]);
    }

    #[test]
    fn test_invalidate_blocks_actions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "invalidate.ts",
            r#"
export default async function(pi) {
  pi.log("loaded");
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let _ = js.take_result();

        let actions = RuntimeActions {
            set_session_name: Some(Arc::new(|_name: String| {})),
            ..Default::default()
        };
        js.bind_core(actions);

        // Should work before invalidate.
        js.execute_script("<ok>", "globalThis.__pi.setSessionName('before');")
            .expect("before invalidate");

        // Invalidate.
        js.invalidate();

        // Should throw after invalidate.
        let err = js
            .runtime
            .execute_script("<after>", "globalThis.__pi.setSessionName('after');")
            .unwrap_err();
        assert!(err.to_string().contains("not initialized"));
    }

    #[test]
    fn test_extension_imports_typebox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "tb.ts",
            r#"
import { Type } from "typebox";

const Params = Type.Object({
  name: Type.String({ description: "The name" }),
  count: Type.Optional(Type.Number({ description: "Count" })),
  active: Type.Boolean(),
  tags: Type.Array(Type.String()),
});

export default async function(pi) {
  pi.log("params: " + JSON.stringify(Params));
  pi.registerTool({
    name: "tb-tool",
    description: "typebox tool",
    parameters: Params,
    execute: async () => {},
  });
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();

        // The log should contain a JSON Schema string.
        assert_eq!(result.logs.len(), 1);
        let schema_json = &result.logs[0];
        // ~kind and ~optional must NOT appear (non-enumerable).
        assert!(!schema_json.contains("~kind"), "schema should not contain ~kind: {schema_json}");
        assert!(!schema_json.contains("~optional"), "schema should not contain ~optional: {schema_json}");
        // Should contain the expected JSON Schema fields.
        assert!(schema_json.contains("\"type\":\"object\""), "missing type:object: {schema_json}");
        assert!(schema_json.contains("\"required\""), "missing required array: {schema_json}");
        assert!(schema_json.contains("\"name\""), "missing name property: {schema_json}");
        assert!(schema_json.contains("\"type\":\"string\""), "missing string type: {schema_json}");
        assert!(schema_json.contains("\"type\":\"number\""), "missing number type: {schema_json}");
        assert!(schema_json.contains("\"type\":\"boolean\""), "missing boolean type: {schema_json}");
        assert!(schema_json.contains("\"type\":\"array\""), "missing array type: {schema_json}");

        // The tool should be registered with the schema as parameters.
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "tb-tool");
        let params = result.tools[0].parameters.as_ref().expect("parameters");
        assert!(params.contains("\"type\":\"object\""));
        // Required should include "name" and "active" but NOT "count" (optional).
        assert!(params.contains("\"name\""));
        assert!(params.contains("\"active\""));
    }

    #[test]
    fn test_extension_imports_string_enum() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "se.ts",
            r#"
import { StringEnum } from "@earendil-works/pi-ai";
import { Type } from "typebox";

const Params = Type.Object({
  action: StringEnum(["list", "add", "toggle", "clear"]),
  text: Type.Optional(Type.String()),
});

export default async function(pi) {
  pi.log("schema: " + JSON.stringify(Params));
  pi.registerTool({
    name: "se-tool",
    description: "string enum tool",
    parameters: Params,
    execute: async () => {},
  });
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();

        assert_eq!(result.logs.len(), 1);
        let schema_json = &result.logs[0];
        // StringEnum should produce {type:"string", enum:[...]}.
        assert!(schema_json.contains("\"enum\""), "missing enum: {schema_json}");
        assert!(schema_json.contains("\"list\""), "missing list value: {schema_json}");
        assert!(schema_json.contains("\"clear\""), "missing clear value: {schema_json}");
        assert!(schema_json.contains("\"type\":\"string\""), "missing string type: {schema_json}");
        // ~kind / ~unsafe must not appear (non-enumerable).
        assert!(!schema_json.contains("~kind"), "should not contain ~kind: {schema_json}");
        assert!(!schema_json.contains("~unsafe"), "should not contain ~unsafe: {schema_json}");

        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "se-tool");
    }

    #[test]
    fn test_extension_imports_pi_coding_agent_constants() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "ca.ts",
            r#"
import { VERSION, CONFIG_DIR_NAME, defineTool } from "@earendil-works/pi-coding-agent";

export default async function(pi) {
  pi.log("version: " + VERSION);
  pi.log("config: " + CONFIG_DIR_NAME);
  const tool = defineTool({
    name: "dt",
    description: "defined tool",
    execute: async () => {},
  });
  pi.registerTool(tool);
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();

        assert_eq!(result.logs, vec![
            "version: 1.79.41".to_string(),
            "config: .pi".to_string(),
        ]);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "dt");
    }

    #[test]
    fn test_extension_imports_pi_tui_stub() {
        // Extensions that import TUI components but never call them at runtime
        // should load successfully. The stubs throw only on invocation.
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "tui.ts",
            r#"
import { Text, matchesKey } from "@earendil-works/pi-tui";

export default async function(pi) {
  // Import succeeds; only calling throws.
  pi.log("tui imported, typeof Text: " + typeof Text);
  pi.log("typeof matchesKey: " + typeof matchesKey);
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();

        assert_eq!(result.logs.len(), 2);
        assert_eq!(result.logs[0], "tui imported, typeof Text: function");
        assert_eq!(result.logs[1], "typeof matchesKey: function");
    }

    #[test]
    fn test_typebox_literal_and_union() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = write_extension(
            dir.path(),
            "lu.ts",
            r#"
import { Type } from "typebox";

const Params = Type.Object({
  mode: Type.Union([Type.Literal("fast"), Type.Literal("slow")]),
});

export default async function(pi) {
  pi.log("schema: " + JSON.stringify(Params));
}
"#,
        );
        let mut js = JsExtensionRuntime::new().expect("runtime");
        block_on(js.load_extension(&ext, dir.path())).expect("load_extension");
        let result = js.take_result();

        assert_eq!(result.logs.len(), 1);
        let schema_json = &result.logs[0];
        assert!(schema_json.contains("\"anyOf\""), "missing anyOf: {schema_json}");
        assert!(schema_json.contains("\"const\":\"fast\""), "missing const fast: {schema_json}");
        assert!(schema_json.contains("\"const\":\"slow\""), "missing const slow: {schema_json}");
    }

    #[test]
    fn test_load_real_todo_extension() {
        // Load the actual todo.ts extension from the pi source tree.
        // This exercises typebox + StringEnum + pi-tui imports together.
        let pi_ext_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../pi/packages/coding-agent/examples/extensions");
        let todo_path = pi_ext_dir.join("todo.ts");
        if !todo_path.exists() {
            eprintln!("Skipping test_load_real_todo_extension: pi source not found at {todo_path:?}");
            return;
        }
        let mut js = JsExtensionRuntime::new().expect("runtime");
        // The todo extension imports from "@earendil-works/pi-tui" at runtime
        // (Text, matchesKey, truncateToWidth) but only uses them inside
        // renderCall/renderResult and the /todos command handler, which are
        // not called during factory load. So it should load successfully.
        block_on(js.load_extension(&todo_path, &pi_ext_dir)).expect("load_extension");
        let result = js.take_result();

        // The todo extension registers a "todo" tool and a "todos" command.
        assert_eq!(result.tools.len(), 1, "expected 1 tool, got {:?}: {:?}", result.tools.len(), result.tools);
        assert_eq!(result.tools[0].name, "todo");
        assert!(result.tools[0].parameters.is_some(), "todo tool should have parameters");
        let params = result.tools[0].parameters.as_ref().expect("parameters");
        assert!(params.contains("\"enum\""), "todo params should have enum: {params}");
        assert!(params.contains("\"list\""), "todo params should have list: {params}");

        assert_eq!(result.commands.len(), 1, "expected 1 command: {:?}", result.commands);
        assert_eq!(result.commands[0].name, "todos");

        // Should have session_start and session_tree event handlers.
        assert_eq!(result.handlers.len(), 2);
        assert_eq!(result.handlers[0].event, "session_start");
        assert_eq!(result.handlers[1].event, "session_tree");
    }

    #[test]
    fn test_load_multiple_real_extensions() {
        let pi_ext_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../pi/packages/coding-agent/examples/extensions");
        if !pi_ext_dir.exists() {
            eprintln!("Skipping: pi source not found");
            return;
        }

        // Scan all .ts files in the extensions directory.
        let mut ok_count = 0u32;
        let mut fail_count = 0u32;
        let mut entries: Vec<_> = std::fs::read_dir(&pi_ext_dir)
            .expect("read extensions dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "ts"))
            .collect();
        entries.sort();
        for ext_path in &entries {
            let name = ext_path.file_name().unwrap().to_string_lossy().to_string();
            // Skip the doom-overlay (wasm, not supported) and test files.
            if name.contains("doom") || name.starts_with("test") {
                continue;
            }
            let mut js = match JsExtensionRuntime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("  FAIL {name}: runtime creation failed: {e}");
                    fail_count += 1;
                    continue;
                }
            };
            match block_on(js.load_extension(ext_path, &pi_ext_dir)) {
                Ok(()) => {
                    let result = js.take_result();
                    eprintln!("  OK   {name}: {} tools, {} commands, {} handlers",
                        result.tools.len(), result.commands.len(), result.handlers.len());
                    ok_count += 1;
                }
                Err(e) => {
                    eprintln!("  FAIL {name}: {e}");
                    fail_count += 1;
                }
            }
        }
        eprintln!("Summary: {} OK, {} FAIL out of {}", ok_count, fail_count, ok_count + fail_count);
        // At least 70% of extensions should load.
        let total = ok_count + fail_count;
        assert!(total > 0, "no extensions found to test");
        assert!(ok_count * 100 / total >= 70, "too many failures: {ok_count} ok / {total} total");
    }

}
