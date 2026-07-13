//! deno_core ops exposed to extension JS as `Deno.core.ops.op_pi_*`.
//!
//! These run inside the V8 isolate on the extension runtime thread. Registration
//! ops receive metadata only (the JS `execute` handler stays in V8). `op_pi_exec`
//! delegates to the shared `core::exec::exec_command`.

use std::cell::RefCell;
use std::rc::Rc;

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

/// State stored in deno_core's OpState. Only holds what JS can't: the
/// notification buffer (returned with each tool call). Tool/command/flag/handler
/// registries live in JS (V8 owns the JS function references).
pub struct PiOpState {
    pub pending_notifications: Rc<RefCell<Vec<String>>>,
    pub shortcuts: Rc<RefCell<Vec<ShortcutInfoSerde>>>,
    pub flags: Rc<RefCell<Vec<FlagOptionsSerde>>>,
}

impl PiOpState {
    pub fn new() -> Self {
        Self {
            pending_notifications: Rc::new(RefCell::new(Vec::new())),
            shortcuts: Rc::new(RefCell::new(Vec::new())),
            flags: Rc::new(RefCell::new(Vec::new())),
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
// Message injection ops (stubs — full impl requires RuntimeCommand variants)
// ============================================================================

#[op2]
#[serde]
pub fn op_pi_send_message(
    _state: &mut OpState,
    #[string] _custom_type: String,
    #[string] _content: String,
) -> Result<(), JsErrorBox> {
    Err(JsErrorBox::generic("pi.sendMessage is not yet supported by the embedded runtime"))
}

#[op2]
#[serde]
pub fn op_pi_send_user_message(
    _state: &mut OpState,
    #[string] _content: String,
) -> Result<(), JsErrorBox> {
    Err(JsErrorBox::generic("pi.sendUserMessage is not yet supported by the embedded runtime"))
}

#[op2]
#[serde]
pub fn op_pi_append_entry(
    _state: &mut OpState,
    #[string] _custom_type: String,
    #[serde] _data: Option<serde_json::Value>,
) -> Result<(), JsErrorBox> {
    Err(JsErrorBox::generic("pi.appendEntry is not yet supported by the embedded runtime"))
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
    // thread forever (matching the old Bun sidecar's per-call timeout).
    let timeout = options
        .timeout
        .map(std::time::Duration::from_secs)
        .or_else(|| Some(std::time::Duration::from_secs(30)));
    let exec_opts = ExecOptions {
        signal: None,
        timeout,
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
        op_pi_exec,
        op_pi_notify,
        op_pi_log,
    ],
    esm_entry_point = "ext:pi_extension/runtime.js",
    esm = [dir "src/core/extensions", "runtime.js"],
);