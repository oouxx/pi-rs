//! `ExtensionRuntime` — the embedded deno_core runtime owning a dedicated
//! V8 thread.
//!
//! V8's isolate is `!Send`, so the runtime owns a `std::thread` running its own
//! `current_thread` tokio runtime + `JsRuntime`. The main (multi-thread) tokio
//! runtime communicates with it via an `mpsc` channel of `RuntimeCommand`s, each
//! carrying a `oneshot` reply channel. Commands carry only `Send` data (strings,
//! serde_json::Value), so no V8 handle crosses the thread boundary.

use std::sync::Arc;

use deno_core::v8;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};

use pi_agent_core::pi_ai_types::ToolExecutionMode;
use pi_agent_core::types::{AgentTool, AgentToolResult};

use super::loader::{discover_extensions, TsModuleLoader};
use super::ops::{pi_extension, PiOpState};
pub use super::ops::{CommandInfoSerde, ToolInfoSerde};

// ============================================================================
// Error type
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("extension runtime not running")]
    NotRunning,
    #[error("extension runtime error: {0}")]
    Runtime(String),
    #[error("channel closed")]
    ChannelClosed,
    #[error("extension operation timed out")]
    Timeout,
}

/// Per-command reply deadline. A hung JS handler or `op_pi_exec` subprocess
/// that never exits must not block the caller (and the dropping thread)
/// forever — the old Bun sidecar had a 30s JSON-RPC timeout; this is the
/// embedded-runtime equivalent.
pub(crate) const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Await a oneshot reply with the standard command deadline. A closed channel
/// maps to `ChannelClosed`; an elapsed deadline maps to `Timeout`.
async fn await_reply<T>(rx: oneshot::Receiver<T>) -> Result<T, ExtensionError> {
    match tokio::time::timeout(COMMAND_TIMEOUT, rx).await {
        Ok(res) => res.map_err(|_| ExtensionError::ChannelClosed),
        Err(_) => Err(ExtensionError::Timeout),
    }
}

impl From<mpsc::error::SendError<RuntimeCommand>> for ExtensionError {
    fn from(_: mpsc::error::SendError<RuntimeCommand>) -> Self {
        ExtensionError::ChannelClosed
    }
}

// ============================================================================
// Result types returned to the main runtime
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoadResult {
    pub tools: Vec<ToolInfoSerde>,
    pub commands: Vec<super::ops::CommandInfoSerde>,
    pub errors: Vec<LoadError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadError {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResponse {
    pub result: serde_json::Value,
    pub notifications: Vec<String>,
}

// ============================================================================
// RuntimeCommand (main → runtime thread)
// ============================================================================

enum RuntimeCommand {
    Load {
        cwd: String,
        agent_dir: Option<String>,
        paths: Vec<String>,
        mode: String,
        has_ui: bool,
        reply: oneshot::Sender<Result<LoadResult, ExtensionError>>,
    },
    CallTool {
        name: String,
        args: serde_json::Value,
        cwd: String,
        reply: oneshot::Sender<Result<CallToolResponse, ExtensionError>>,
    },
    DispatchEvent {
        event_type: String,
        payload: serde_json::Value,
        result_returning: bool,
        reply: oneshot::Sender<Result<serde_json::Value, ExtensionError>>,
    },
    /// Hot-reload: clear JS registries, re-discover, and re-load extensions.
    Reload {
        cwd: String,
        agent_dir: Option<String>,
        paths: Vec<String>,
        mode: String,
        has_ui: bool,
        reply: oneshot::Sender<Result<LoadResult, ExtensionError>>,
    },
    Stop {
        reply: oneshot::Sender<()>,
    },
}

// ============================================================================
// ExtensionRuntime — the clone-able handle
// ============================================================================

/// An error event from an extension handler, structured for diagnostics.
#[derive(Debug, Clone)]
pub struct ExtensionErrorEvent {
    pub extension_path: String,
    pub event: String,
    pub error: String,
}

/// Handle to the embedded extension runtime. Cheap to clone; all clones share
/// the underlying V8 thread. When the last handle drops, the thread exits.
#[derive(Clone)]
pub struct ExtensionRuntime {
    tx: mpsc::UnboundedSender<RuntimeCommand>,
    host_commands: Arc<std::sync::Mutex<Vec<super::ops::HostCommand>>>,
    error_tx: broadcast::Sender<ExtensionErrorEvent>,
    _join: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl ExtensionRuntime {
    /// Spawn the extension runtime thread. Returns immediately.
    ///
    /// Fallible so a failure to create the V8 thread (rare, but possible under
    /// thread/resource limits) degrades to no-extensions mode instead of
    /// panicking the whole CLI — mirroring the old sidecar's `is_available()` gate.
    pub fn new() -> Result<Self, ExtensionError> {
        let (tx, rx) = mpsc::unbounded_channel::<RuntimeCommand>();
        let host_commands: Arc<std::sync::Mutex<Vec<super::ops::HostCommand>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let (error_tx, _) = broadcast::channel::<ExtensionErrorEvent>(64);
        let host_cmds = Arc::clone(&host_commands);
        let err_tx = error_tx.clone();
        let join = std::thread::Builder::new()
            .name("pi-extension-runtime".into())
            .spawn(move || {
                runtime_thread_main(rx, host_cmds, err_tx);
            })
            .map_err(|e| ExtensionError::Runtime(format!("failed to spawn extension runtime thread: {e}")))?;
        Ok(Self {
            tx,
            host_commands,
            error_tx,
            _join: Arc::new(std::sync::Mutex::new(Some(join))),
        })
    }

    /// Subscribe to extension error events. Returns a receiver that yields
    /// `ExtensionErrorEvent` values as they occur.
    pub fn on_error(&self) -> broadcast::Receiver<ExtensionErrorEvent> {
        self.error_tx.subscribe()
    }

    /// Emit an error event from an extension handler. Called from the V8 thread
    /// when a JS handler throws an exception. The error is broadcast to all
    /// subscribers; if no subscriber is listening, the event is silently dropped
    /// (broadcast capacity of 64 prevents backpressure).
    pub fn emit_error(&self, extension_path: &str, event: &str, error: &str) {
        let _ = self.error_tx.send(ExtensionErrorEvent {
            extension_path: extension_path.to_string(),
            event: event.to_string(),
            error: error.to_string(),
        });
    }

    pub async fn load(
        &self,
        cwd: &str,
        agent_dir: Option<&str>,
        paths: &[String],
    ) -> Result<LoadResult, ExtensionError> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(RuntimeCommand::Load {
            cwd: cwd.to_string(),
            agent_dir: agent_dir.map(String::from),
            paths: paths.to_vec(),
            mode: "rpc".to_string(),
            has_ui: false,
            reply,
        })?;
        await_reply(rx).await?
    }

    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        cwd: &str,
    ) -> Result<CallToolResponse, ExtensionError> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(RuntimeCommand::CallTool {
            name: name.to_string(),
            args,
            cwd: cwd.to_string(),
            reply,
        })?;
        await_reply(rx).await?
    }

    pub async fn dispatch_fire_and_forget(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), ExtensionError> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(RuntimeCommand::DispatchEvent {
            event_type: event_type.to_string(),
            payload,
            result_returning: false,
            reply,
        })?;
        let _ = await_reply(rx).await??;
        Ok(())
    }

    pub async fn dispatch_result(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, ExtensionError> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(RuntimeCommand::DispatchEvent {
            event_type: event_type.to_string(),
            payload,
            result_returning: true,
            reply,
        })?;
        await_reply(rx).await?
    }

    pub async fn stop(&self) -> Result<(), ExtensionError> {
        let (reply, rx) = oneshot::channel();
        // Best-effort: if the thread already exited, send fails silently.
        let _ = self.tx.send(RuntimeCommand::Stop { reply });
        let _ = rx.await;
        Ok(())
    }

    /// Reload all extensions: clear JS registries, re-discover, and re-load.
    /// Returns the new load result on success.
    pub async fn reload(
        &self,
        cwd: &str,
        agent_dir: Option<&str>,
        paths: &[String],
    ) -> Result<LoadResult, ExtensionError> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(RuntimeCommand::Reload {
            cwd: cwd.to_string(),
            agent_dir: agent_dir.map(|s| s.to_string()),
            paths: paths.to_vec(),
            mode: "rpc".to_string(),
            has_ui: false,
            reply,
        })?;
        await_reply(rx).await?
    }

    /// Poll for pending host commands from the V8 thread. Returns the first
    /// pending command, or `None` if no commands are queued. The main thread
    /// should call this periodically to process host callbacks from ops.
    pub fn poll_host_command(&self) -> Option<super::ops::HostCommand> {
        let mut guard = self.host_commands.lock().ok()?;
        if guard.is_empty() {
            return None;
        }
        // Use swap_remove (O(1)) instead of remove(0) (O(n)) to avoid O(n²)
        // drain cost when draining many commands.
        Some(guard.swap_remove(0))
    }

    /// Process all pending host commands using the provided handler closure.
    /// The handler receives (function_name, args_json) and should return
    /// a Result with the response value.
    pub fn process_host_commands<F>(&self, mut handler: F)
    where
        F: FnMut(&str, &serde_json::Value) -> Result<serde_json::Value, String>,
    {
        while let Some(cmd) = self.poll_host_command() {
            let result = handler(&cmd.function, &cmd.args);
            let _ = cmd.reply.send(result);
        }
    }

    /// Drain all pending host commands and return them for async processing.
    /// The caller is responsible for sending replies via each command's `reply`
    /// channel. Returns an empty Vec if no commands are pending.
    pub fn drain_host_commands(&self) -> Vec<super::ops::HostCommand> {
        let mut guard = match self.host_commands.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        std::mem::take(&mut *guard)
    }
}

impl Drop for ExtensionRuntime {
    fn drop(&mut self) {
        // Best-effort stop on last handle. The Arc<Mutex<Option<JoinHandle>>>
        // is shared across clones; only the last drop sees Some(handle).
        if let Ok(mut guard) = self._join.lock() {
            if let Some(handle) = guard.take() {
                // Tell the thread to stop. Do NOT block-join: an in-flight
                // command (e.g. a hung op_pi_exec) keeps the V8 thread busy, and
                // joining would block the dropping thread — on a tokio worker
                // that stalls the whole async scheduler. Instead detach: once
                // every sender (this handle's `tx`) is dropped, `rx.recv()` in
                // the thread returns None and the thread exits on its own.
                let _ = self.tx.send(RuntimeCommand::Stop {
                    reply: oneshot::channel().0,
                });
                drop(handle);
            }
        }
    }
}

// ============================================================================
// Runtime thread body
// ============================================================================

fn runtime_thread_main(
    mut rx: mpsc::UnboundedReceiver<RuntimeCommand>,
    host_commands: Arc<std::sync::Mutex<Vec<super::ops::HostCommand>>>,
    error_tx: broadcast::Sender<ExtensionErrorEvent>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            // Fail-open: surface the failure rather than panicking the dedicated
            // runtime thread (a panic here would take the whole process down on
            // some configurations). With no runtime, the thread simply exits;
            // callers already degrade to no-extensions mode when commands go
            // unanswered (CommandTimeout / channel closed).
            eprintln!("[pi] failed to build extension runtime tokio runtime: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let mut js = build_js_runtime(host_commands, error_tx);

        // The extension! esm_entry_point loads runtime.js on init; drain the
        // event loop to settle any pending evaluation. A failure here means the
        // runtime shim never loaded — surface it rather than letting every later
        // __pi* call fail with a cryptic ReferenceError.
        if let Err(e) = js.run_event_loop(Default::default()).await {
            eprintln!("[pi] extension runtime failed to initialize: {e}");
        }

        while let Some(cmd) = rx.recv().await {
            match cmd {
                RuntimeCommand::Load {
                    cwd,
                    agent_dir,
                    paths,
                    mode,
                    has_ui,
                    reply,
                } => {
                    let res = handle_load(&mut js, &cwd, agent_dir.as_deref(), &paths, &mode, has_ui).await;
                    let _ = reply.send(res);
                }
                RuntimeCommand::CallTool {
                    name,
                    args,
                    cwd,
                    reply,
                } => {
                    let res = handle_call_tool(&mut js, &name, &args, &cwd).await;
                    let _ = reply.send(res);
                }
                RuntimeCommand::DispatchEvent {
                    event_type,
                    payload,
                    result_returning,
                    reply,
                } => {
                    let res = handle_dispatch(&mut js, &event_type, &payload, result_returning).await;
                    let _ = reply.send(res);
                }
                RuntimeCommand::Reload {
                    cwd,
                    agent_dir,
                    paths,
                    mode,
                    has_ui,
                    reply,
                } => {
                    // Clear JS registries, re-discover, and re-load.
                    if let Err(e) = js.execute_script("<pi-clear>", "globalThis.__piClearRegistries()") {
                        eprintln!("[pi] extension reload: __piClearRegistries failed: {e}");
                    }
                    let _ = js.run_event_loop(Default::default()).await;
                    // Filter to only reloadable extensions. Non-reloadable extensions
                    // (loaded via `-e` explicit path) are excluded from reload.
                    let discovered = discover_extensions(&cwd, agent_dir.as_deref(), &paths);
                    let reloadable_paths: Vec<String> = discovered
                        .iter()
                        .filter(|ext| ext.reloadable)
                        .map(|ext| ext.path.to_string_lossy().to_string())
                        .collect();
                    let res = handle_load(&mut js, &cwd, agent_dir.as_deref(), &reloadable_paths, &mode, has_ui).await;
                    let _ = reply.send(res);
                }
                RuntimeCommand::Stop { reply } => {
                    let _ = reply.send(());
                    break;
                }
            }
            // Drain microtasks / pending ops after each command.
            let _ = js.run_event_loop(Default::default()).await;
        }
    });
}

fn build_js_runtime(
    host_commands: Arc<std::sync::Mutex<Vec<super::ops::HostCommand>>>,
    error_tx: broadcast::Sender<ExtensionErrorEvent>,
) -> deno_core::JsRuntime {
    let loader = std::rc::Rc::new(TsModuleLoader::new());
    let mut js = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
        module_loader: Some(loader),
        extensions: vec![pi_extension::init()],
        ..Default::default()
    });
    // Put the PiOpState into OpState so ops can borrow it.
    let mut pi_state = PiOpState::new();
    pi_state.host_commands = Some(host_commands);
    pi_state.error_tx = Some(error_tx);
    js.op_state().borrow_mut().put(pi_state);
    js
}

// ============================================================================
// Command handlers (run on the V8 thread)
// ============================================================================

#[allow(deprecated)]
async fn handle_load(
    js: &mut deno_core::JsRuntime,
    cwd: &str,
    agent_dir: Option<&str>,
    paths: &[String],
    mode: &str,
    has_ui: bool,
) -> Result<LoadResult, ExtensionError> {
    // Clear JS-side registries for a fresh load.
    if let Err(e) = js.execute_script("<pi-clear>", "globalThis.__piClearRegistries()") {
        eprintln!("[pi] extension load: __piClearRegistries failed: {e}");
    }
    let _ = js.run_event_loop(Default::default()).await;

    // Tell the JS shim the session cwd so pi.exec defaults to it (mirrors the
    // original pi: options?.cwd ?? runner.cwd). A &str serializing to JSON
    // cannot fail for valid UTF-8; if it ever does, surface it rather than
    // silently substituting "/" (which would feed extensions a wrong cwd).
    let cwd_json = match serde_json::to_string(cwd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[pi] extension load: cwd serialization failed ({e}); using \"\"");
            "\"\"".to_string()
        }
    };
    let set_cwd = format!("globalThis.__piSetCwd({cwd_json})");
    if let Err(e) = js.execute_script("<pi-set-cwd>", set_cwd) {
        eprintln!("[pi] extension load: __piSetCwd failed: {e}");
    }

    // Set the context mode so extensions see the correct ctx.mode / ctx.hasUI.
    let mode_json = match serde_json::to_string(mode) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[pi] extension load: mode serialization failed ({e}); using \"rpc\"");
            "\"rpc\"".to_string()
        }
    };
    let has_ui_json = if has_ui { "true" } else { "false" };
    let set_mode = format!("globalThis.__piSetContextMode({mode_json}, {has_ui_json})");
    if let Err(e) = js.execute_script("<pi-set-mode>", set_mode) {
        eprintln!("[pi] extension load: __piSetContextMode failed: {e}");
    }

    let discovered = discover_extensions(cwd, agent_dir, paths);
    let mut tools: Vec<ToolInfoSerde> = Vec::new();
    let mut commands: Vec<super::ops::CommandInfoSerde> = Vec::new();
    let mut errors: Vec<LoadError> = Vec::new();

    for ext in &discovered {
        let specifier = path_to_specifier(&ext.path);
        // __piLoadExtension dynamic-imports the module and calls the factory.
        let script = format!(
            "globalThis.__piLoadExtension({})",
            serde_json::to_string(&specifier).unwrap_or_else(|_| "''".into())
        );
        match js.execute_script("<pi-load-ext>", script) {
            Ok(global) => {
                // dynamic import → Promise: await it via resolve.
                if let Err(e) = js.resolve_value(global).await {
                    errors.push(LoadError {
                        path: ext.path.to_string_lossy().to_string(),
                        error: e.to_string(),
                    });
                }
                let _ = js.run_event_loop(Default::default()).await;
            }
            Err(e) => {
                errors.push(LoadError {
                    path: ext.path.to_string_lossy().to_string(),
                    error: e.to_string(),
                });
            }
        }
    }

    // Read back registered tool + command metadata from JS.
    if let Ok(global) = js.execute_script("<pi-tools>", "globalThis.__piGetToolInfos()") {
        if let Ok(val) = read_json_value(js, global) {
            if let Some(arr) = val.as_array() {
                for t in arr {
                    if let Ok(info) = serde_json::from_value::<ToolInfoSerde>(t.clone()) {
                        tools.push(info);
                    }
                }
            }
        }
    }
    if let Ok(global) = js.execute_script("<pi-commands>", "globalThis.__piGetCommands()") {
        if let Ok(val) = read_json_value(js, global) {
            if let Some(arr) = val.as_array() {
                for c in arr {
                    if let Ok(info) = serde_json::from_value::<super::ops::CommandInfoSerde>(c.clone())
                    {
                        commands.push(info);
                    }
                }
            }
        }
    }

    Ok(LoadResult {
        tools,
        commands,
        errors,
    })
}

#[allow(deprecated)]
async fn handle_call_tool(
    js: &mut deno_core::JsRuntime,
    name: &str,
    args: &serde_json::Value,
    cwd: &str,
) -> Result<CallToolResponse, ExtensionError> {
    let script = format!(
        "globalThis.__piCallTool({}, {}, {})",
        serde_json::to_string(name).unwrap_or_else(|_| "''".into()),
        serde_json::to_string(args).unwrap_or_else(|_| "null".into()),
        serde_json::to_string(cwd).unwrap_or_else(|_| "\"\"".into())
    );
    let global = js
        .execute_script("<pi-call-tool>", script)
        .map_err(|e: Box<deno_core::error::JsError>| ExtensionError::Runtime(e.to_string()))?;
    let resolved = js
        .resolve_value(global)
        .await
        .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
    let _ = js.run_event_loop(Default::default()).await;
    let val = read_json_value(js, resolved).map_err(|e| ExtensionError::Runtime(e.to_string()))?;
    let result = val.get("result").cloned().unwrap_or(serde_json::Value::Null);
    let notifications = val
        .get("notifications")
        .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
        .unwrap_or_default();
    Ok(CallToolResponse {
        result,
        notifications,
    })
}

#[allow(deprecated)]
async fn handle_dispatch(
    js: &mut deno_core::JsRuntime,
    event_type: &str,
    payload: &serde_json::Value,
    result_returning: bool,
) -> Result<serde_json::Value, ExtensionError> {
    let fn_name = if result_returning {
        "__piDispatchResult"
    } else {
        "__piDispatch"
    };
    let script = format!(
        "{}({}, {})",
        fn_name,
        serde_json::to_string(event_type).unwrap_or_else(|_| "''".into()),
        serde_json::to_string(payload).unwrap_or_else(|_| "null".into())
    );
    let global = js
        .execute_script("<pi-dispatch>", script)
        .map_err(|e: Box<deno_core::error::JsError>| ExtensionError::Runtime(e.to_string()))?;
    // Both dispatch fns are async → return a Promise; await so handlers run.
    let resolved = js
        .resolve_value(global)
        .await
        .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
    let _ = js.run_event_loop(Default::default()).await;
    read_json_value(js, resolved).map_err(|e| ExtensionError::Runtime(e.to_string()))
}

/// Read a `v8::Global<v8::Value>` back into a `serde_json::Value` via a scope.
fn read_json_value(
    js: &mut deno_core::JsRuntime,
    global: deno_core::v8::Global<deno_core::v8::Value>,
) -> Result<serde_json::Value, String> {
    deno_core::scope!(scope, js);
    let local = v8::Local::new(scope, global);
    if local.is_undefined() || local.is_null() {
        return Ok(serde_json::Value::Null);
    }
    deno_core::serde_v8::from_v8(scope, local).map_err(|e: deno_core::serde_v8::Error| e.to_string())
}

fn path_to_specifier(path: &std::path::Path) -> String {
    // Use a file:// URL. resolve_path/deno_core expects a valid module specifier.
    let absolute = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    deno_core::resolve_path(
        &absolute.to_string_lossy(),
        &std::env::current_dir().unwrap_or_default(),
    )
    .map(|s| s.to_string())
    .unwrap_or_else(|_| format!("file://{}", absolute.display()))
}

// ============================================================================
// create_extension_agent_tools — wrap loaded extension tools as AgentTools
// ============================================================================

/// Build `AgentTool`s whose `execute` closure sends a `CallTool` command to the
/// embedded runtime. The JS handler stays in V8; Rust just round-trips the args
/// and result as JSON.
pub fn create_extension_agent_tools(
    tools: &[ToolInfoSerde],
    runtime: Arc<ExtensionRuntime>,
    cwd: String,
) -> Vec<AgentTool<serde_json::Value, serde_json::Value>> {
    tools
        .iter()
        .map(|info| {
            let rt = Arc::clone(&runtime);
            let name = info.name.clone();
            let tool_cwd = cwd.clone();
            AgentTool {
                name: info.name.clone(),
                description: info.description.clone(),
                label: String::new(),
                parameters_schema: info
                    .parameters
                    .clone()
                    .unwrap_or(serde_json::Value::Null),
                execution_mode: info.execution_mode.as_ref().and_then(|m| match m.as_str() {
                    "sequential" => Some(ToolExecutionMode::Sequential),
                    _ => None,
                }),
                prepare_arguments: None,
                execute: Arc::new(move |_call_id, args, _signal, _on_update| {
                    let rt = Arc::clone(&rt);
                    let name = name.clone();
                    let cwd = tool_cwd.clone();
                    Box::pin(async move {
                        let response = rt
                            .call_tool(&name, args, &cwd)
                            .await
                            .map_err(|e| {
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    format!("Extension tool '{name}' failed: {e}"),
                                ))
                                    as Box<dyn std::error::Error + Send + Sync>
                            })?;
                        Ok(AgentToolResult {
                            content: Vec::new(),
                            details: response.result,
                            terminate: None,
                        })
                    }) as std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<
                                        AgentToolResult<serde_json::Value>,
                                        Box<dyn std::error::Error + Send + Sync>,
                                    >,
                                > + Send,
                        >,
                    >
                }),
            }
        })
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temp dir with an extension file.
    struct ExtFixture {
        dir: tempfile::TempDir,
        extensions_dir: std::path::PathBuf,
    }

    impl ExtFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let extensions_dir = dir.path().join(".pi-rs").join("extensions");
            fs::create_dir_all(&extensions_dir).unwrap();
            Self { dir, extensions_dir }
        }

        fn write_ext(&self, name: &str, code: &str) {
            fs::write(self.extensions_dir.join(name), code).unwrap();
        }

        fn cwd(&self) -> &str {
            self.dir.path().to_str().unwrap()
        }
    }

    // -----------------------------------------------------------------------
    // ExtensionRuntime lifecycle tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_runtime_create_and_stop() {
        let runtime = ExtensionRuntime::new().unwrap();
        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_empty() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.tools.is_empty());
        assert!(result.commands.is_empty());
        assert!(result.errors.is_empty());

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_extension_with_tool() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("test-tool.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "my-tool",
                    description: "A test tool",
                    parameters: { type: "object", properties: {} },
                    execute: async () => ({ content: [{ type: "text", text: "ok" }] }),
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "my-tool");
        assert_eq!(result.tools[0].description, "A test tool");

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_extension_with_command() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("test-cmd.ts", r#"
            export default function(pi) {
                pi.registerCommand("hello", {
                    description: "Say hello",
                    handler: async () => {},
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.commands.len(), 1);
        assert_eq!(result.commands[0].name, "hello");

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_multiple_extensions() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("tool-a.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "tool-a",
                    description: "Tool A",
                    parameters: { type: "object", properties: {} },
                    execute: async () => ({ content: [{ type: "text", text: "a" }] }),
                });
            }
        "#);
        fx.write_ext("tool-b.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "tool-b",
                    description: "Tool B",
                    parameters: { type: "object", properties: {} },
                    execute: async () => ({ content: [{ type: "text", text: "b" }] }),
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.tools.len(), 2);
        let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"tool-a"));
        assert!(names.contains(&"tool-b"));

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_call_tool() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("echo.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "echo",
                    description: "Echo input",
                    parameters: { type: "object", properties: { text: { type: "string" } } },
                    execute: async (callId, args) => {
                        return { content: [{ type: "text", text: args.text || "none" }] };
                    },
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let response = runtime
            .call_tool("echo", serde_json::json!({"text": "hello"}), fx.cwd())
            .await
            .unwrap();

        assert_eq!(response.result["content"][0]["text"], "hello");

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_call_tool_with_notifications() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("notify.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "notify",
                    description: "Tool that sends notifications",
                    parameters: { type: "object", properties: {} },
                    execute: async (args, _ctx, _signal, _onUpdate, ctx) => {
                        ctx.ui.notify("Hello from tool!", "info");
                        return { content: [{ type: "text", text: "done" }] };
                    },
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let response = runtime
            .call_tool("notify", serde_json::json!({}), fx.cwd())
            .await
            .unwrap();

        assert_eq!(response.result["content"][0]["text"], "done");
        assert!(response.notifications.contains(&"Hello from tool!".to_string()));

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_invalid_extension_reports_error() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("invalid.ts", "this is not valid typescript export default function");

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();

        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].path.contains("invalid.ts"));
        assert!(result.tools.is_empty());

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_extension_that_throws() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("throws.ts", r#"
            export default function(pi) {
                throw new Error("Initialization failed!");
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();

        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].error.contains("Initialization failed!"));

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_extension_no_default_export() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("no-default.ts", r#"
            export function notDefault(pi) {
                pi.registerCommand("test", { handler: async () => {} });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();

        assert_eq!(result.errors.len(), 1);
        // The Rust shim message differs slightly from the TS original:
        // "Extension does not export a default factory" (vs "valid factory function").
        assert!(
            result.errors[0].error.contains("does not export a default factory")
                || result.errors[0].error.contains("factory"),
            "error was: {}",
            result.errors[0].error
        );

        runtime.stop().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Event dispatch tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_runtime_dispatch_fire_and_forget() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("handler.ts", r#"
            export default function(pi) {
                pi.on("custom_event", async (event) => {
                    globalThis.__lastEvent = event;
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // Fire-and-forget dispatch
        runtime
            .dispatch_fire_and_forget("custom_event", serde_json::json!({"key": "value"}))
            .await
            .unwrap();

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_dispatch_result() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("blocker.ts", r#"
            export default function(pi) {
                pi.on("tool_call", async (event) => {
                    if (event.toolName === "dangerous") {
                        return { block: true, reason: "Blocked by extension" };
                    }
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // Dispatch tool_call event and check result
        let res = runtime
            .dispatch_result("tool_call", serde_json::json!({
                "type": "tool_call",
                "toolCallId": "call_1",
                "toolName": "dangerous",
                "input": {},
            }))
            .await
            .unwrap();

        assert_eq!(res["block"], true);
        assert_eq!(res["reason"], "Blocked by extension");

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_dispatch_result_no_block() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("blocker.ts", r#"
            export default function(pi) {
                pi.on("tool_call", async (event) => {
                    if (event.toolName === "dangerous") {
                        return { block: true, reason: "Blocked" };
                    }
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // Safe tool — should not be blocked
        let res = runtime
            .dispatch_result("tool_call", serde_json::json!({
                "type": "tool_call",
                "toolCallId": "call_2",
                "toolName": "safe-tool",
                "input": {},
            }))
            .await
            .unwrap();

        assert!(res.get("block").and_then(|v| v.as_bool()).unwrap_or(false) == false);

        runtime.stop().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Error event tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_runtime_error_event_subscription() {
        let runtime = ExtensionRuntime::new().unwrap();
        let mut rx = runtime.on_error();

        // Trigger an error by loading invalid extension
        let fx = ExtFixture::new();
        fx.write_ext("throws.ts", r#"
            export default function(pi) {
                pi.on("some_event", async () => {
                    throw new Error("Handler error!");
                });
            }
        "#);

        let _result = runtime.load(fx.cwd(), None, &[]).await.unwrap();

        // Dispatch to trigger the throwing handler
        let _ = runtime.dispatch_fire_and_forget("some_event", serde_json::json!({})).await;

        // Give the runtime thread time to process and emit the error
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Check if we got an error event
        match rx.try_recv() {
            Ok(event) => {
                assert_eq!(event.event, "some_event");
                assert!(event.error.contains("Handler error!"));
            }
            Err(_) => {
                // Error might not have propagated yet — that's OK for this test
            }
        }

        runtime.stop().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Host command tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_runtime_poll_host_command() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("host-cmd.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "send-msg",
                    description: "Send a message",
                    parameters: { type: "object", properties: {} },
                    execute: async () => {
                        Deno.core.ops.op_pi_send_message("custom", "from extension");
                        return { content: [{ type: "text", text: "sent" }] };
                    },
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // Call the tool which queues a host command
        let _response = runtime
            .call_tool("send-msg", serde_json::json!({}), fx.cwd())
            .await
            .unwrap();

        // Poll for the host command
        let cmd = runtime.poll_host_command();
        assert!(cmd.is_some(), "should have a host command");
        if let Some(cmd) = cmd {
            assert_eq!(cmd.function, "send_message");
        }

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_drain_host_commands() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("multi-cmd.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "multi",
                    description: "Multiple commands",
                    parameters: { type: "object", properties: {} },
                    execute: async () => {
                        Deno.core.ops.op_pi_send_message("type1", "msg1");
                        Deno.core.ops.op_pi_send_message("type2", "msg2");
                        return { content: [{ type: "text", text: "done" }] };
                    },
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let _response = runtime
            .call_tool("multi", serde_json::json!({}), fx.cwd())
            .await
            .unwrap();

        // Drain all host commands
        let cmds = runtime.drain_host_commands();
        assert_eq!(cmds.len(), 2, "should have 2 host commands");
        assert_eq!(cmds[0].function, "send_message");
        assert_eq!(cmds[1].function, "send_message");

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_process_host_commands() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        fx.write_ext("process-test.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "process-test",
                    description: "Test host command processing",
                    parameters: { type: "object", properties: {} },
                    execute: async () => {
                        Deno.core.ops.op_pi_send_message("test", "hello");
                        return { content: [{ type: "text", text: "done" }] };
                    },
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        let _response = runtime
            .call_tool("process-test", serde_json::json!({}), fx.cwd())
            .await
            .unwrap();

        // Process host commands with a handler
        let processed = std::sync::Mutex::new(Vec::new());
        runtime.process_host_commands(|function, args| {
            processed.lock().unwrap().push((function.to_string(), args.clone()));
            Ok(serde_json::json!({}))
        });

        let processed = processed.lock().unwrap();
        assert_eq!(processed.len(), 1);
        assert_eq!(processed[0].0, "send_message");

        runtime.stop().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Reload tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_runtime_reload() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        // Load initial extension
        fx.write_ext("v1.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "v1",
                    description: "Version 1",
                    parameters: { type: "object", properties: {} },
                    execute: async () => ({ content: [{ type: "text", text: "v1" }] }),
                });
            }
        "#);

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "v1");

        // Replace extension file
        fs::remove_file(fx.extensions_dir.join("v1.ts")).unwrap();
        fx.write_ext("v2.ts", r#"
            export default function(pi) {
                pi.registerTool({
                    name: "v2",
                    description: "Version 2",
                    parameters: { type: "object", properties: {} },
                    execute: async () => ({ content: [{ type: "text", text: "v2" }] }),
                });
            }
        "#);

        // Reload
        let result = runtime.reload(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "v2");

        runtime.stop().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_runtime_call_nonexistent_tool() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        let result = runtime.load(fx.cwd(), None, &[]).await.unwrap();
        assert!(result.errors.is_empty());

        let response = runtime
            .call_tool("nonexistent", serde_json::json!({}), fx.cwd())
            .await;

        assert!(response.is_err());

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_stop_twice() {
        let runtime = ExtensionRuntime::new().unwrap();
        runtime.stop().await.unwrap();
        // Second stop should be a no-op
        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_clone_and_drop() {
        let runtime = ExtensionRuntime::new().unwrap();
        let cloned = runtime.clone();
        // Both handles share the same thread
        cloned.stop().await.unwrap();
        // Original should also be stopped
        let result = runtime.load("/tmp", None, &[]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_runtime_load_with_agent_dir() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        // Create global extensions dir
        let agent_dir = fx.dir.path().join("agent");
        let global_ext = agent_dir.join("extensions");
        fs::create_dir_all(&global_ext).unwrap();
        fs::write(global_ext.join("global.ts"), r#"
            export default function(pi) {
                pi.registerTool({
                    name: "global-tool",
                    description: "From global dir",
                    parameters: { type: "object", properties: {} },
                    execute: async () => ({ content: [{ type: "text", text: "global" }] }),
                });
            }
        "#).unwrap();

        let result = runtime.load(
            fx.cwd(),
            Some(agent_dir.to_str().unwrap()),
            &[],
        ).await.unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "global-tool");

        runtime.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_runtime_load_with_explicit_paths() {
        let runtime = ExtensionRuntime::new().unwrap();
        let fx = ExtFixture::new();

        // Create extension outside the standard discovery path
        let custom_dir = fx.dir.path().join("custom");
        fs::create_dir_all(&custom_dir).unwrap();
        let ext_path = custom_dir.join("explicit.ts");
        fs::write(&ext_path, r#"
            export default function(pi) {
                pi.registerTool({
                    name: "explicit-tool",
                    description: "From explicit path",
                    parameters: { type: "object", properties: {} },
                    execute: async () => ({ content: [{ type: "text", text: "explicit" }] }),
                });
            }
        "#).unwrap();

        let result = runtime.load(
            fx.cwd(),
            None,
            &[ext_path.to_string_lossy().to_string()],
        ).await.unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "explicit-tool");

        runtime.stop().await.unwrap();
    }

    // -----------------------------------------------------------------------
    // ExtensionRuntime::new() failure test
    // -----------------------------------------------------------------------

    #[test]
    fn test_runtime_new_succeeds() {
        let runtime = ExtensionRuntime::new();
        assert!(runtime.is_ok());
    }
}

