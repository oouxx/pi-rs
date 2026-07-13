//! deno_core ops exposed to extension JS as `Deno.core.ops.op_pi_*`.
//!
//! These run inside the V8 isolate on the extension runtime thread. Registration
//! ops receive metadata only (the JS `execute` handler stays in V8). `op_pi_exec`
//! delegates to the shared `core::exec::exec_command`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::op2;
use deno_core::OpState;
use deno_error::JsErrorBox;
use serde::{Deserialize, Serialize};

use crate::core::exec::{exec_command, ExecOptions, ExecResult};

// ============================================================================
// Types crossing the JS<->Rust boundary
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolInfoSerde {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    #[serde(default, rename = "prompt_guidelines")]
    pub prompt_guidelines: Option<Vec<String>>,
    #[serde(default, rename = "execution_mode")]
    pub execution_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandInfoSerde {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FlagOptionsSerde {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub flag_type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShortcutInfoSerde {
    pub key: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExecOptionsSerde {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

impl From<ExecOptionsSerde> for ExecOptions {
    fn from(o: ExecOptionsSerde) -> Self {
        ExecOptions {
            signal: None,
            timeout: o.timeout.map(std::time::Duration::from_secs),
            cwd: o.cwd,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecResultSerde {
    pub stdout: String,
    pub stderr: String,
    #[serde(rename = "exitCode")]
    pub exit_code: i32,
    pub killed: bool,
}

impl From<ExecResult> for ExecResultSerde {
    fn from(r: ExecResult) -> Self {
        ExecResultSerde {
            stdout: r.stdout,
            stderr: r.stderr,
            exit_code: r.code,
            killed: r.killed,
        }
    }
}

// ============================================================================
// OpState: Rust-side mirrors of JS registries
// ============================================================================

/// A command from the V8 thread to the host (main thread). Ops that need host
/// state (ctx methods, message injection, session management) push a HostCommand
/// onto the shared Vec; the main thread polls and processes them.
pub struct HostCommand {
    pub function: String,
    pub args: serde_json::Value,
    pub reply: tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
}

/// State stored in deno_core's OpState. Only holds what JS can't: the
/// notification buffer (returned with each tool call). Tool/command/flag/handler
/// registries live in JS (V8 owns the JS function references).
pub struct PiOpState {
    pub pending_notifications: Rc<RefCell<Vec<String>>>,
    pub shortcuts: Rc<RefCell<Vec<ShortcutInfoSerde>>>,
    pub flags: Rc<RefCell<Vec<FlagOptionsSerde>>>,
    pub host_commands: Option<Arc<std::sync::Mutex<Vec<HostCommand>>>>,
    pub error_tx: Option<tokio::sync::broadcast::Sender<super::runtime::ExtensionErrorEvent>>,
}

impl PiOpState {
    pub fn new() -> Self {
        Self {
            pending_notifications: Rc::new(RefCell::new(Vec::new())),
            shortcuts: Rc::new(RefCell::new(Vec::new())),
            flags: Rc::new(RefCell::new(Vec::new())),
            host_commands: None,
            error_tx: None,
        }
    }
}

// ============================================================================
// Registration ops (metadata only; JS keeps the handler)
// ============================================================================

#[op2]
#[serde]
pub fn op_pi_register_tool(
    state: &mut OpState,
    #[serde] tool: ToolInfoSerde,
) -> Result<ToolInfoSerde, JsErrorBox> {
    // Mirror so Rust can build AgentTools without round-tripping back to JS.
    // The JS side already stored the full tool (with execute handler) in its
    // own Map; this op just confirms the metadata.
    let _ = state;
    Ok(tool)
}

#[op2]
#[serde]
pub fn op_pi_register_command(
    state: &mut OpState,
    #[string] name: String,
    #[serde] options: serde_json::Value,
) -> Result<CommandInfoSerde, JsErrorBox> {
    let _ = state;
    let description = options
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok(CommandInfoSerde { name, description })
}

#[op2]
pub fn op_pi_register_shortcut(
    state: &mut OpState,
    #[string] key: String,
    #[serde] options: serde_json::Value,
) -> Result<(), JsErrorBox> {
    let pi_state = state.borrow_mut::<PiOpState>();
    let description = options.get("description").and_then(|v| v.as_str()).map(String::from);
    pi_state.shortcuts.borrow_mut().push(ShortcutInfoSerde { key, description });
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_get_shortcuts(state: &mut OpState) -> Result<Vec<ShortcutInfoSerde>, JsErrorBox> {
    let pi_state = state.borrow_mut::<PiOpState>();
    Ok(pi_state.shortcuts.borrow().clone())
}

#[op2]
pub fn op_pi_register_flag(
    state: &mut OpState,
    #[string] name: String,
    #[serde] options: FlagOptionsSerde,
) -> Result<(), JsErrorBox> {
    let pi_state = state.borrow_mut::<PiOpState>();
    pi_state.flags.borrow_mut().push(options);
    let _ = name;
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_get_flags(state: &mut OpState) -> Result<Vec<FlagOptionsSerde>, JsErrorBox> {
    let pi_state = state.borrow_mut::<PiOpState>();
    Ok(pi_state.flags.borrow().clone())
}

#[op2]
#[serde]
pub fn op_pi_get_commands(state: &mut OpState) -> Result<Vec<CommandInfoSerde>, JsErrorBox> {
    let _ = state;
    // JS owns the command registry; return empty here. The runtime.rs Load
    // path reads commands back via __piGetCommands instead.
    Ok(Vec::new())
}

// ============================================================================
// Helper: push a HostCommand onto the shared Vec
// ============================================================================

/// Push a host command onto the shared Vec for main-thread processing.
/// Returns `true` if the command was queued, `false` if no host_commands channel
/// is available (e.g. runtime not yet initialized).
fn push_host_command(
    state: &mut OpState,
    function: &str,
    args: serde_json::Value,
) -> bool {
    let pi_state = state.borrow_mut::<PiOpState>();
    if let Some(ref host_cmds) = pi_state.host_commands {
        let (reply, _rx) = tokio::sync::oneshot::channel();
        let cmd = HostCommand {
            function: function.to_string(),
            args,
            reply,
        };
        if let Ok(mut guard) = host_cmds.lock() {
            guard.push(cmd);
            return true;
        }
    }
    false
}

// ============================================================================
// Message injection ops
// ============================================================================

#[op2]
#[serde]
pub fn op_pi_send_message(
    state: &mut OpState,
    #[string] custom_type: String,
    #[string] content: String,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "send_message",
        serde_json::json!({ "customType": custom_type, "content": content }),
    );
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_send_user_message(
    state: &mut OpState,
    #[string] content: String,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "send_user_message",
        serde_json::json!({ "content": content }),
    );
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_append_entry(
    state: &mut OpState,
    #[string] custom_type: String,
    #[serde] data: Option<serde_json::Value>,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "append_entry",
        serde_json::json!({ "customType": custom_type, "data": data }),
    );
    Ok(())
}

// ============================================================================
// Session metadata ops — push HostCommand for main-thread processing
// ============================================================================

#[op2]
#[serde]
pub fn op_pi_set_session_name(
    state: &mut OpState,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "set_session_name",
        serde_json::json!({ "name": name }),
    );
    Ok(())
}

#[op2(fast)]
pub fn op_pi_get_session_name(_state: &mut OpState) -> Result<(), JsErrorBox> {
    Ok(())
}

#[op2(fast)]
pub fn op_pi_set_label(
    state: &mut OpState,
    #[string] entry_id: String,
    #[string] label: String,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "set_label",
        serde_json::json!({ "entryId": entry_id, "label": label }),
    );
    Ok(())
}

// ============================================================================
// Model/thinking ops — push HostCommand for main-thread processing
// ============================================================================

#[op2]
#[serde]
pub fn op_pi_set_model(
    state: &mut OpState,
    #[string] model: String,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "set_model",
        serde_json::json!({ "model": model }),
    );
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_set_thinking_level(
    state: &mut OpState,
    #[string] level: String,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "set_thinking_level",
        serde_json::json!({ "level": level }),
    );
    Ok(())
}

// ============================================================================
// Tool management ops — push HostCommand for main-thread processing
// ============================================================================

#[op2]
#[serde]
pub fn op_pi_get_active_tools(
    state: &mut OpState,
) -> Result<Vec<String>, JsErrorBox> {
    // Push a host command to get active tools from the main thread.
    // The reply channel is oneshot, so the result is sent back asynchronously.
    // For now, return empty list as a safe default.
    push_host_command(
        state,
        "get_active_tools",
        serde_json::json!({}),
    );
    Ok(Vec::new())
}

#[op2]
#[serde]
pub fn op_pi_get_all_tools(
    state: &mut OpState,
) -> Result<Vec<String>, JsErrorBox> {
    push_host_command(
        state,
        "get_all_tools",
        serde_json::json!({}),
    );
    Ok(Vec::new())
}

#[op2]
#[serde]
pub fn op_pi_set_active_tools(
    state: &mut OpState,
    #[serde] tool_names: Vec<String>,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "set_active_tools",
        serde_json::json!({ "toolNames": tool_names }),
    );
    Ok(())
}

// ============================================================================
// getThinkingLevel op — push HostCommand for main-thread processing
// ============================================================================

#[op2]
#[string]
pub fn op_pi_get_thinking_level(
    state: &mut OpState,
) -> Result<String, JsErrorBox> {
    push_host_command(
        state,
        "get_thinking_level",
        serde_json::json!({}),
    );
    // Return a default; the real value will be processed asynchronously.
    Ok("medium".to_string())
}

#[op2]
#[serde]
pub fn op_pi_register_provider(
    state: &mut OpState,
    #[string] name: String,
    #[serde] config: serde_json::Value,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "register_provider",
        serde_json::json!({ "name": name, "config": config }),
    );
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_unregister_provider(
    state: &mut OpState,
    #[string] name: String,
) -> Result<(), JsErrorBox> {
    push_host_command(
        state,
        "unregister_provider",
        serde_json::json!({ "name": name }),
    );
    Ok(())
}

// ============================================================================
// ctx action method ops — push HostCommand for main-thread processing
// ============================================================================

#[op2(fast)]
pub fn op_pi_ctx_is_idle(_state: &mut OpState) -> Result<bool, JsErrorBox> {
    // Push a host command to check if the agent is idle.
    // For now, return true as a safe default.
    push_host_command(_state, "ctx_is_idle", serde_json::json!({}));
    Ok(true)
}

#[op2(fast)]
pub fn op_pi_ctx_is_project_trusted(_state: &mut OpState) -> Result<bool, JsErrorBox> {
    Ok(true)
}

#[op2(fast)]
pub fn op_pi_ctx_has_pending_messages(_state: &mut OpState) -> Result<bool, JsErrorBox> {
    push_host_command(_state, "ctx_has_pending_messages", serde_json::json!({}));
    Ok(false)
}

#[op2]
#[string]
pub fn op_pi_ctx_get_system_prompt(
    state: &mut OpState,
) -> Result<String, JsErrorBox> {
    push_host_command(state, "ctx_get_system_prompt", serde_json::json!({}));
    Ok(String::new())
}

#[op2(fast)]
pub fn op_pi_ctx_abort(
    state: &mut OpState,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "ctx_abort", serde_json::json!({}));
    Ok(())
}

#[op2(fast)]
pub fn op_pi_ctx_shutdown(
    state: &mut OpState,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "ctx_shutdown", serde_json::json!({}));
    Ok(())
}

// ============================================================================
// Missing ExtensionContext method ops
// ============================================================================

#[op2]
#[string]
pub fn op_pi_ctx_get_model(
    state: &mut OpState,
) -> Result<String, JsErrorBox> {
    push_host_command(state, "ctx_get_model", serde_json::json!({}));
    Ok(String::new())
}

#[op2]
#[serde]
pub fn op_pi_ctx_get_context_usage(
    state: &mut OpState,
) -> Result<serde_json::Value, JsErrorBox> {
    push_host_command(state, "ctx_get_context_usage", serde_json::json!({}));
    Ok(serde_json::json!({
        "tokensUsed": 0,
        "tokensTotal": 0,
        "percentUsed": 0.0,
    }))
}

#[op2(fast)]
pub fn op_pi_ctx_compact(
    state: &mut OpState,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "ctx_compact", serde_json::json!({}));
    Ok(())
}

// ============================================================================
// ctx.ui ops (stubs)
// ============================================================================

#[op2(fast)]
pub fn op_pi_ui_set_status(
    _state: &mut OpState,
    #[string] _key: String,
    #[string] _text: String,
) -> Result<(), JsErrorBox> {
    Ok(())
}

#[op2(fast)]
pub fn op_pi_ui_set_working_message(
    _state: &mut OpState,
    #[string] _message: String,
) -> Result<(), JsErrorBox> {
    Ok(())
}

#[op2(fast)]
pub fn op_pi_ui_set_title(
    _state: &mut OpState,
    #[string] _title: String,
) -> Result<(), JsErrorBox> {
    Ok(())
}

// ============================================================================
// ExtensionCommandContext ops — push HostCommand for main-thread processing
// ============================================================================

#[op2]
#[serde]
pub fn op_pi_new_session(
    state: &mut OpState,
    #[serde] _options: serde_json::Value,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "new_session", serde_json::json!({}));
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_fork(
    state: &mut OpState,
    #[string] _entry_id: String,
    #[serde] _options: serde_json::Value,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "fork", serde_json::json!({}));
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_switch_session(
    state: &mut OpState,
    #[string] _session_path: String,
    #[serde] _options: serde_json::Value,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "switch_session", serde_json::json!({}));
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_reload(
    state: &mut OpState,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "reload", serde_json::json!({}));
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_wait_for_idle(
    state: &mut OpState,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "wait_for_idle", serde_json::json!({}));
    Ok(())
}

#[op2]
#[serde]
pub fn op_pi_navigate_tree(
    state: &mut OpState,
    #[string] _direction: String,
) -> Result<(), JsErrorBox> {
    push_host_command(state, "navigate_tree", serde_json::json!({}));
    Ok(())
}

// ============================================================================
// exec / notify / log
// ============================================================================

#[op2]
#[serde]
pub async fn op_pi_exec(
    #[string] command: String,
    #[serde] args: Vec<String>,
    #[serde] options: ExecOptionsSerde,
) -> Result<ExecResultSerde, JsErrorBox> {
    let cwd = options.cwd.clone().unwrap_or_else(|| ".".to_string());
    // Default a missing timeout so a runaway subprocess can't pin the V8
    // thread forever (matching the old Bun sidecar's per-call timeout). Clamp
    // any caller-supplied timeout to COMMAND_TIMEOUT: the main thread awaits
    // the V8 reply with that deadline (await_reply), and the V8 command loop is
    // strictly serial — an op running longer than COMMAND_TIMEOUT would let the
    // caller time out first while the V8 thread stays occupied, starving every
    // subsequently-queued dispatch.
    let timeout = options
        .timeout
        .map(std::time::Duration::from_secs)
        .map(|d| d.min(super::runtime::COMMAND_TIMEOUT))
        .unwrap_or_else(|| std::time::Duration::from_secs(30));
    let exec_opts = ExecOptions {
        signal: None,
        timeout: Some(timeout),
        cwd: options.cwd.clone(),
    };
    let result = exec_command(&command, &args, &cwd, Some(exec_opts)).await;
    Ok(result.into())
}

#[op2]
pub fn op_pi_notify(
    state: &mut OpState,
    #[string] message: String,
    #[string] r#type: Option<String>,
) -> Result<(), JsErrorBox> {
    let _ = r#type;
    let pi_state = state.try_borrow_mut::<PiOpState>();
    if let Some(pi) = pi_state {
        pi.pending_notifications.borrow_mut().push(message);
    }
    Ok(())
}

#[op2(fast)]
pub fn op_pi_log(#[string] message: String) {
    eprintln!("[pi extension] {message}");
}

#[op2(fast)]
pub fn op_pi_emit_error(
    state: &mut OpState,
    #[string] extension_path: String,
    #[string] event: String,
    #[string] error: String,
) -> Result<(), JsErrorBox> {
    let pi_state = state.try_borrow_mut::<PiOpState>();
    if let Some(pi) = pi_state {
        if let Some(ref error_tx) = pi.error_tx {
            let _ = error_tx.send(super::runtime::ExtensionErrorEvent {
                extension_path,
                event,
                error,
            });
        }
    }
    Ok(())
}

// ============================================================================
// extension! macro — package ops + runtime.js as a deno_core extension
// ============================================================================

deno_core::extension!(
    pi_extension,
    ops = [
        op_pi_register_tool,
        op_pi_register_command,
        op_pi_register_shortcut,
        op_pi_get_shortcuts,
        op_pi_register_flag,
        op_pi_get_flags,
        op_pi_get_commands,
        op_pi_send_message,
        op_pi_send_user_message,
        op_pi_append_entry,
        op_pi_set_session_name,
        op_pi_get_session_name,
        op_pi_set_label,
        op_pi_set_model,
        op_pi_set_thinking_level,
        op_pi_get_thinking_level,
        op_pi_register_provider,
        op_pi_unregister_provider,
        op_pi_get_active_tools,
        op_pi_get_all_tools,
        op_pi_set_active_tools,
        op_pi_ctx_is_idle,
        op_pi_ctx_is_project_trusted,
        op_pi_ctx_has_pending_messages,
        op_pi_ctx_get_system_prompt,
        op_pi_ctx_abort,
        op_pi_ctx_shutdown,
        op_pi_ctx_get_model,
        op_pi_ctx_get_context_usage,
        op_pi_ctx_compact,
        op_pi_ui_set_status,
        op_pi_ui_set_working_message,
        op_pi_ui_set_title,
        op_pi_new_session,
        op_pi_fork,
        op_pi_switch_session,
        op_pi_reload,
        op_pi_wait_for_idle,
        op_pi_navigate_tree,
        op_pi_exec,
        op_pi_notify,
        op_pi_log,
        op_pi_emit_error,
    ],
    esm_entry_point = "ext:pi_extension/runtime.js",
    esm = [dir "src/core/extensions", "runtime.js"],
);