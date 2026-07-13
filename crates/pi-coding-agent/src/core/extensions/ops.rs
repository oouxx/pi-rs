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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    /// Helper: create a JsRuntime with pi_extension ops and PiOpState.
    fn test_runtime() -> deno_core::JsRuntime {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });

        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(host_commands);
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);
        js
    }

    /// Helper: evaluate JS and return the result as a serde_json::Value.
    fn eval(js: &mut deno_core::JsRuntime, code: &str) -> serde_json::Value {
        let global = js
            .execute_script("<test>", code.to_string())
            .unwrap();
        deno_core::scope!(scope, js);
        let local = deno_core::v8::Local::new(scope, global);
        deno_core::serde_v8::from_v8(scope, local).unwrap()
    }

    // -----------------------------------------------------------------------
    // op_pi_register_tool tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_register_tool() {
        let mut js = test_runtime();
        let result = eval(&mut js, r#"
            Deno.core.ops.op_pi_register_tool({
                name: "my-tool",
                description: "A test tool",
                parameters: { type: "object", properties: {} },
            });
        "#);
        assert_eq!(result["name"], "my-tool");
        assert_eq!(result["description"], "A test tool");
    }

    #[test]
    fn test_op_register_tool_with_prompt_guidelines() {
        let mut js = test_runtime();
        let result = eval(&mut js, r#"
            Deno.core.ops.op_pi_register_tool({
                name: "guided-tool",
                description: "Tool with guidelines",
                prompt_guidelines: ["be careful", "check permissions"],
                execution_mode: "sequential",
            });
        "#);
        assert_eq!(result["name"], "guided-tool");
        assert!(result["prompt_guidelines"].is_array());
        assert_eq!(result["execution_mode"], "sequential");
    }

    // -----------------------------------------------------------------------
    // op_pi_register_command tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_register_command() {
        let mut js = test_runtime();
        let result = eval(&mut js, r#"
            Deno.core.ops.op_pi_register_command("test-cmd", {
                description: "A test command",
            });
        "#);
        assert_eq!(result["name"], "test-cmd");
        assert_eq!(result["description"], "A test command");
    }

    #[test]
    fn test_op_register_command_no_description() {
        let mut js = test_runtime();
        let result = eval(&mut js, r#"
            Deno.core.ops.op_pi_register_command("bare-cmd", {});
        "#);
        assert_eq!(result["name"], "bare-cmd");
        assert!(result["description"].is_null());
    }

    // -----------------------------------------------------------------------
    // op_pi_register_shortcut / op_pi_get_shortcuts tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_register_and_get_shortcuts() {
        let mut js = test_runtime();
        eval(&mut js, r#"
            Deno.core.ops.op_pi_register_shortcut("ctrl+k", {
                description: "Clear screen",
            });
            Deno.core.ops.op_pi_register_shortcut("ctrl+shift+x", {
                description: "Custom action",
            });
        "#);

        let op_state = js.op_state();
        let mut guard = op_state.borrow_mut();
        let pi_state = guard.borrow_mut::<PiOpState>();
        let shortcuts = pi_state.shortcuts.borrow().clone();
        assert_eq!(shortcuts.len(), 2);
        assert_eq!(shortcuts[0].key, "ctrl+k");
        assert_eq!(shortcuts[0].description.as_deref(), Some("Clear screen"));
        assert_eq!(shortcuts[1].key, "ctrl+shift+x");
    }

    #[test]
    fn test_op_get_shortcuts() {
        let mut js = test_runtime();
        eval(&mut js, r#"
            Deno.core.ops.op_pi_register_shortcut("ctrl+a", { description: "Select all" });
        "#);

        let result = eval(&mut js, "Deno.core.ops.op_pi_get_shortcuts();");
        assert!(result.is_array());
        assert_eq!(result[0]["key"], "ctrl+a");
    }

    // -----------------------------------------------------------------------
    // op_pi_register_flag / op_pi_get_flags tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_register_and_get_flags() {
        let mut js = test_runtime();
        eval(&mut js, r#"
            Deno.core.ops.op_pi_register_flag("--verbose", {
                description: "Enable verbose output",
                type: "boolean",
                default: false,
            });
            Deno.core.ops.op_pi_register_flag("--theme", {
                description: "Select theme",
                type: "string",
                default: "dark",
            });
        "#);

        let op_state = js.op_state();
        let mut guard = op_state.borrow_mut();
        let pi_state = guard.borrow_mut::<PiOpState>();
        let flags = pi_state.flags.borrow().clone();
        assert_eq!(flags.len(), 2);
        assert_eq!(flags[0].description.as_deref(), Some("Enable verbose output"));
        assert_eq!(flags[0].flag_type, "boolean");
        assert_eq!(flags[1].flag_type, "string");
    }

    #[test]
    fn test_op_get_flags() {
        let mut js = test_runtime();
        eval(&mut js, r#"
            Deno.core.ops.op_pi_register_flag("--test-flag", {
                description: "Test flag",
                type: "boolean",
            });
        "#);

        let result = eval(&mut js, "Deno.core.ops.op_pi_get_flags();");
        assert!(result.is_array());
        assert_eq!(result[0]["description"], "Test flag");
    }

    // -----------------------------------------------------------------------
    // op_pi_get_commands tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_get_commands_empty() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_get_commands();");
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 0);
    }

    // -----------------------------------------------------------------------
    // op_pi_send_message tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_send_message_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"
            Deno.core.ops.op_pi_send_message("custom_type", "hello world");
        "#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "send_message");
        assert_eq!(cmds[0].args["customType"], "custom_type");
        assert_eq!(cmds[0].args["content"], "hello world");
    }

    // -----------------------------------------------------------------------
    // op_pi_send_user_message tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_send_user_message_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"
            Deno.core.ops.op_pi_send_user_message("user said something");
        "#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "send_user_message");
        assert_eq!(cmds[0].args["content"], "user said something");
    }

    // -----------------------------------------------------------------------
    // op_pi_append_entry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_append_entry_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"
            Deno.core.ops.op_pi_append_entry("log", { message: "test log" });
        "#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "append_entry");
        assert_eq!(cmds[0].args["customType"], "log");
    }

    // -----------------------------------------------------------------------
    // op_pi_set_session_name / op_pi_get_session_name tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_set_session_name_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_set_session_name("my-session");"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "set_session_name");
        assert_eq!(cmds[0].args["name"], "my-session");
    }

    // -----------------------------------------------------------------------
    // op_pi_notify tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_notify_appends_to_pending() {
        let mut js = test_runtime();
        eval(&mut js, r#"Deno.core.ops.op_pi_notify("Hello!", "info");"#);

        let op_state = js.op_state();
        let mut guard = op_state.borrow_mut();
        let pi_state = guard.borrow_mut::<PiOpState>();
        let notifications = pi_state.pending_notifications.borrow().clone();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0], "Hello!");
    }

    #[test]
    fn test_op_notify_multiple() {
        let mut js = test_runtime();
        eval(&mut js, r#"
            Deno.core.ops.op_pi_notify("First", "info");
            Deno.core.ops.op_pi_notify("Second", "error");
        "#);

        let op_state = js.op_state();
        let mut guard = op_state.borrow_mut();
        let pi_state = guard.borrow_mut::<PiOpState>();
        let notifications = pi_state.pending_notifications.borrow().clone();
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0], "First");
        assert_eq!(notifications[1], "Second");
    }

    // -----------------------------------------------------------------------
    // op_pi_log tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_log_does_not_crash() {
        let mut js = test_runtime();
        // op_pi_log just prints to stderr; verify it doesn't throw.
        eval(&mut js, r#"Deno.core.ops.op_pi_log("test log message");"#);
    }

    // -----------------------------------------------------------------------
    // op_pi_emit_error tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_emit_error_sends_to_broadcast() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, mut error_rx) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(host_commands);
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"
            Deno.core.ops.op_pi_emit_error("/path/to/ext.ts", "tool_call", "Something went wrong");
        "#);

        let err = error_rx.try_recv().unwrap();
        assert_eq!(err.extension_path, "/path/to/ext.ts");
        assert_eq!(err.event, "tool_call");
        assert_eq!(err.error, "Something went wrong");
    }

    // -----------------------------------------------------------------------
    // op_pi_set_model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_set_model_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_set_model("gpt-4o");"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "set_model");
    }

    // -----------------------------------------------------------------------
    // op_pi_set_thinking_level / op_pi_get_thinking_level tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_set_thinking_level_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_set_thinking_level("high");"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "set_thinking_level");
    }

    // -----------------------------------------------------------------------
    // op_pi_ctx_* tests (stubs that return defaults)
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_ctx_is_idle_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_ctx_is_idle();");
        assert_eq!(result, true);
    }

    #[test]
    fn test_op_ctx_is_project_trusted_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_ctx_is_project_trusted();");
        assert_eq!(result, true);
    }

    #[test]
    fn test_op_ctx_has_pending_messages_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_ctx_has_pending_messages();");
        assert_eq!(result, false);
    }

    #[test]
    fn test_op_ctx_get_system_prompt_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_ctx_get_system_prompt();");
        assert_eq!(result, "");
    }

    #[test]
    fn test_op_ctx_get_model_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_ctx_get_model();");
        assert_eq!(result, "");
    }

    // -----------------------------------------------------------------------
    // op_pi_ui_set_status tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_ui_set_status_does_not_crash() {
        let mut js = test_runtime();
        // op_pi_ui_set_status is a stub that just returns Ok(()) without
        // queuing a host command. Verify it doesn't throw.
        eval(&mut js, r#"Deno.core.ops.op_pi_ui_set_status("status-key", "Running...");"#);
    }

    // -----------------------------------------------------------------------
    // op_pi_new_session / op_pi_fork / op_pi_switch_session tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_new_session_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_new_session({ mode: "rpc" });"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "new_session");
    }

    #[test]
    fn test_op_fork_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_fork("entry-1", { position: "at" });"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "fork");
    }

    #[test]
    fn test_op_switch_session_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_switch_session("/path/to/session", {});"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "switch_session");
    }

    // -----------------------------------------------------------------------
    // op_pi_reload / op_pi_wait_for_idle / op_pi_navigate_tree tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_reload_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, "Deno.core.ops.op_pi_reload();");

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "reload");
    }

    #[test]
    fn test_op_wait_for_idle_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, "Deno.core.ops.op_pi_wait_for_idle();");

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "wait_for_idle");
    }

    #[test]
    fn test_op_navigate_tree_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_navigate_tree("parent");"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "navigate_tree");
    }

    // -----------------------------------------------------------------------
    // op_pi_set_label tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_set_label_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_set_label("entry-1", "my-label");"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "set_label");
    }

    // -----------------------------------------------------------------------
    // op_pi_get_active_tools / op_pi_get_all_tools / op_pi_set_active_tools tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_get_active_tools_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_get_active_tools();");
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_op_get_all_tools_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_get_all_tools();");
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_op_set_active_tools_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_set_active_tools(["read", "write"]);"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "set_active_tools");
    }

    // -----------------------------------------------------------------------
    // op_pi_register_provider / op_pi_unregister_provider tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_register_provider_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"
            Deno.core.ops.op_pi_register_provider("test-provider", {
                baseUrl: "https://test.api/v1",
                apiKey: "test-key",
            });
        "#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "register_provider");
    }

    #[test]
    fn test_op_unregister_provider_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, r#"Deno.core.ops.op_pi_unregister_provider("test-provider");"#);

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "unregister_provider");
    }

    // -----------------------------------------------------------------------
    // op_pi_ctx_abort / op_pi_ctx_shutdown / op_pi_ctx_compact tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_ctx_abort_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, "Deno.core.ops.op_pi_ctx_abort();");

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "ctx_abort");
    }

    #[test]
    fn test_op_ctx_shutdown_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, "Deno.core.ops.op_pi_ctx_shutdown();");

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "ctx_shutdown");
    }

    #[test]
    fn test_op_ctx_compact_queues_host_command() {
        let host_commands = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<super::super::runtime::ExtensionErrorEvent>(64);

        let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![pi_extension::init()],
            ..Default::default()
        });
        let mut pi_state = PiOpState::new();
        pi_state.host_commands = Some(Arc::clone(&host_commands));
        pi_state.error_tx = Some(error_tx);
        js.op_state().borrow_mut().put(pi_state);

        eval(&mut js, "Deno.core.ops.op_pi_ctx_compact();");

        let cmds = host_commands.lock().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].function, "ctx_compact");
    }

    // -----------------------------------------------------------------------
    // op_pi_ctx_get_context_usage tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_ctx_get_context_usage_default() {
        let mut js = test_runtime();
        let result = eval(&mut js, "Deno.core.ops.op_pi_ctx_get_context_usage();");
        assert_eq!(result["tokensUsed"], 0);
        assert_eq!(result["tokensTotal"], 0);
        assert_eq!(result["percentUsed"], 0);
    }

    // -----------------------------------------------------------------------
    // op_pi_ui_set_working_message / op_pi_ui_set_title tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_op_ui_set_working_message_does_not_crash() {
        let mut js = test_runtime();
        // op_pi_ui_set_working_message is a stub that just returns Ok(()).
        eval(&mut js, r#"Deno.core.ops.op_pi_ui_set_working_message("Processing...");"#);
    }

    #[test]
    fn test_op_ui_set_title_does_not_crash() {
        let mut js = test_runtime();
        // op_pi_ui_set_title is a stub that just returns Ok(()).
        eval(&mut js, r#"Deno.core.ops.op_pi_ui_set_title("New Title");"#);
    }

    // -----------------------------------------------------------------------
    // PiOpState tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_pi_op_state_new() {
        let state = PiOpState::new();
        assert!(state.pending_notifications.borrow().is_empty());
        assert!(state.shortcuts.borrow().is_empty());
        assert!(state.flags.borrow().is_empty());
        assert!(state.host_commands.is_none());
        assert!(state.error_tx.is_none());
    }

    // -----------------------------------------------------------------------
    // ToolInfoSerde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_info_serde_roundtrip() {
        let info = ToolInfoSerde {
            name: "test".into(),
            description: "A test tool".into(),
            parameters: Some(serde_json::json!({"type": "object"})),
            prompt_guidelines: Some(vec!["guideline 1".into()]),
            execution_mode: Some("sequential".into()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: ToolInfoSerde = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.execution_mode.as_deref(), Some("sequential"));
    }

    // -----------------------------------------------------------------------
    // CommandInfoSerde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_command_info_serde_roundtrip() {
        let info = CommandInfoSerde {
            name: "my-cmd".into(),
            description: Some("My command".into()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: CommandInfoSerde = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "my-cmd");
        assert_eq!(parsed.description.as_deref(), Some("My command"));
    }

    // -----------------------------------------------------------------------
    // FlagOptionsSerde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_flag_options_serde_roundtrip() {
        let info = FlagOptionsSerde {
            description: Some("A flag".into()),
            flag_type: "boolean".into(),
            default: Some(serde_json::json!(true)),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: FlagOptionsSerde = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.flag_type, "boolean");
        assert_eq!(parsed.default.as_ref().and_then(|v| v.as_bool()), Some(true));
    }

    // -----------------------------------------------------------------------
    // ShortcutInfoSerde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_shortcut_info_serde_roundtrip() {
        let info = ShortcutInfoSerde {
            key: "ctrl+k".into(),
            description: Some("Clear screen".into()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: ShortcutInfoSerde = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.key, "ctrl+k");
        assert_eq!(parsed.description.as_deref(), Some("Clear screen"));
    }

    // -----------------------------------------------------------------------
    // ExecOptionsSerde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_exec_options_serde_default() {
        let opts = ExecOptionsSerde::default();
        assert!(opts.cwd.is_none());
        assert!(opts.timeout.is_none());
    }

    #[test]
    fn test_exec_options_serde_to_exec_options() {
        let opts = ExecOptionsSerde {
            cwd: Some("/tmp".into()),
            timeout: Some(30),
        };
        let exec: ExecOptions = opts.into();
        assert_eq!(exec.cwd, Some("/tmp".into()));
        assert!(exec.timeout.is_some());
    }

    // -----------------------------------------------------------------------
    // ExecResultSerde tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_exec_result_serde_from_exec_result() {
        let result = crate::core::exec::ExecResult {
            stdout: "hello".into(),
            stderr: "".into(),
            code: 0,
            killed: false,
        };
        let serde: ExecResultSerde = result.into();
        assert_eq!(serde.stdout, "hello");
        assert_eq!(serde.exit_code, 0);
        assert!(!serde.killed);
    }
}