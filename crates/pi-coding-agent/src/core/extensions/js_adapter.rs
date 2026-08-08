//! Adapter that wraps a loaded JS extension as a `HookHandler`.
//!
//! After `JsExtensionRuntime::load_extension()` captures an
//! `ExtensionLoadResult` (tools, commands, shortcuts, flags, handlers), this
//! module turns it into a `HookHandler` impl that can be registered into the
//! existing `HookRunner` / `ExtensionRegistry`.
//!
//! **Registration** (tools/commands/shortcuts/flags) is synchronous — it just
//! reads the captured metadata, no JS call needed.
//!
//! **Tool execution** and **event dispatch** call back into the V8 runtime via
//! a channel. The V8 runtime runs on a dedicated current-thread task (V8
//! isolates are `!Send`); the adapter sends `JsCommand`s and awaits oneshot
//! responses.

#![cfg(feature = "js-runtime")]

use std::sync::Arc;
use async_trait::async_trait;

use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use pi_extension_api::{
    create_synthetic_source_info, CommandRegistry, CommandRegistration, ExtensionContext,
    FlagRegistry, HookHandler, HookResult, ShortcutRegistry,
    SourceInfo, SourceOrigin, SourceScope, ToolCallOutput, ToolDefinition, ToolExecuteFn,
    ToolRegistry,
};

use super::js_runtime::ExtensionLoadResult;

// ============================================================================
// JsCommand — channel protocol between adapter and V8 runtime
// ============================================================================

/// A command sent from the adapter (running on any tokio task) to the V8
/// runtime task (running on a current-thread `LocalSet`).
pub enum JsCommand {
    /// Load a TS/JS extension file and invoke its factory. Returns the
    /// captured `ExtensionLoadResult` (metadata); JS callbacks stay alive
    /// in the V8 runtime for later invocation.
    LoadExtension {
        path: std::path::PathBuf,
        cwd: std::path::PathBuf,
        response_tx: oneshot::Sender<Result<ExtensionLoadResult, String>>,
    },
    /// Execute a registered tool's JS `execute` callback.
    ExecuteTool {
        tool_name: String,
        tool_call_id: String,
        params: Value,
        response_tx: oneshot::Sender<Result<ToolCallOutput, String>>,
    },
    /// Fire an event to all registered JS handlers for that event name.
    /// Returns the (JSON array of) handler return values — used by
    /// result-bearing hooks like `tool_call` (`{block: true}`) and
    /// `before_agent_start`.
    FireEvent {
        event: String,
        data_json: String,
        response_tx: oneshot::Sender<Result<serde_json::Value, String>>,
    },
    /// Execute a registered command's JS handler.
    ExecuteCommand {
        command_name: String,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Bind the runtime core: install action-method closures so that
    /// post-load action ops (sendMessage, registerProvider, etc.) delegate
    /// to the host instead of throwing "not initialized". Mirrors TS
    /// `ExtensionRunner.bindCore()`.
    BindCore {
        actions: super::js_runtime::RuntimeActions,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Invalidate the runtime context (session changed: new/fork/switch/
    /// reload). After this, action ops throw a stale-ctx error.
    Invalidate,
    /// Shut down the V8 runtime thread. The receiver breaks out of its
    /// command loop on receipt, so shutdown is deterministic even if some
    /// adapter-held sender clones are still alive.
    Shutdown,
}

impl std::fmt::Debug for JsCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadExtension { .. } => write!(f, "JsCommand::LoadExtension"),
            Self::ExecuteTool { .. } => write!(f, "JsCommand::ExecuteTool"),
            Self::FireEvent { .. } => write!(f, "JsCommand::FireEvent"),
            Self::ExecuteCommand { .. } => write!(f, "JsCommand::ExecuteCommand"),
            Self::BindCore { .. } => write!(f, "JsCommand::BindCore"),
            Self::Invalidate => write!(f, "JsCommand::Invalidate"),
            Self::Shutdown => write!(f, "JsCommand::Shutdown"),
        }
    }
}

// ============================================================================
// JsToolResult — deserialization of a JS tool's `AgentToolResult` return value
// ============================================================================

/// The shape returned by a JS extension tool's `execute()` callback.
///
/// This mirrors the TS `AgentToolResult<T>` interface (`content`, `details`,
/// `terminate`, `addedToolNames`). It is **not** the same as `ToolCallOutput`:
/// `AgentToolResult` has no `isError` field (errors are signalled by the
/// callback throwing, which we catch separately). We deserialize with
/// `#[serde(default)]` so a tool that omits optional fields still parses.
#[derive(Debug, serde::Deserialize)]
struct JsToolResult {
    #[serde(default)]
    content: Vec<Value>,
    #[serde(default)]
    details: Option<Value>,
    #[serde(default)]
    terminate: Option<bool>,
    // `addedToolNames` is intentionally ignored — the Rust extension API has
    // no equivalent field in `ToolCallOutput`.
}

impl JsToolResult {
    /// Convert into the Rust-side `ToolCallOutput`. `is_error` is always
    /// `false` here — if the JS callback had thrown, the caller handles that
    /// path before reaching this conversion.
    fn into_output(self) -> ToolCallOutput {
        ToolCallOutput {
            content: self.content,
            details: self.details,
            is_error: false,
            terminate: self.terminate,
        }
    }
}

/// Build a minimal `ExtensionContext`-like JS object and the tool-execute
/// script.
///
/// The original TS `ToolDefinition.execute` takes five arguments:
/// `(toolCallId, params, signal, onUpdate, ctx)`. We pass `undefined` for
/// `signal` and `onUpdate` (no abort signalling / streaming updates in the
/// runtime loader) and a `ctx` built by `__pi.createCtx()` whose methods are
/// no-ops or safe defaults. Tools that require real UI interaction or abort
/// signals will degrade gracefully rather than crash.
fn build_tool_execute_script(tool_name: &str, tool_call_id: &str, params_json: &str) -> String {
    format!(
        "(async () => {{
          const fn = globalThis.__pi.__toolExecutors.get({name:?});
          if (!fn) throw new Error('Tool not found: ' + {name:?});
          const ctx = globalThis.__pi.createCtx();
          const result = await fn({call_id:?}, {params}, undefined, undefined, ctx);
          return result;
        }})()",
        name = tool_name,
        call_id = tool_call_id,
        params = params_json,
    )
}

// ============================================================================
// JsExtensionAdapter — HookHandler impl for a loaded JS extension
// ============================================================================

/// A `HookHandler` backed by a JS extension loaded via `JsExtensionRuntime`.
///
/// Created after `load_extension()` returns an `ExtensionLoadResult`. The
/// adapter registers tools/commands/shortcuts/flags from the captured
/// metadata, and delegates tool execution + event dispatch to the V8 runtime
/// via the `JsCommand` channel.
pub struct JsExtensionAdapter {
    /// Extension display name (derived from the file path).
    name: String,
    /// Metadata captured during factory invocation.
    load_result: ExtensionLoadResult,
    /// Provenance for registered tools/commands/etc.
    source_info: SourceInfo,
    /// Channel to the V8 runtime task.
    cmd_tx: mpsc::Sender<JsCommand>,
}

impl JsExtensionAdapter {
    /// Create a new adapter from a loaded extension's result.
    ///
    /// `extension_path` is the file path of the extension (used for the
    /// adapter name and `SourceInfo`). `cmd_tx` is the channel to the V8
    /// runtime task that owns the `JsExtensionRuntime`.
    #[must_use]
    pub fn new(
        extension_path: &str,
        load_result: ExtensionLoadResult,
        cmd_tx: mpsc::Sender<JsCommand>,
    ) -> Self {
        let name = std::path::Path::new(extension_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| extension_path.to_string());

        let source_info = create_synthetic_source_info(
            extension_path.to_string(),
            "extension".to_string(),
            Some(SourceScope::Project),
            Some(SourceOrigin::TopLevel),
            None,
        );

        Self {
            name,
            load_result,
            source_info,
            cmd_tx,
        }
    }

    /// The per-extension `SourceInfo` (derived from the extension file path).
    /// Used by the integration layer to stamp provenance onto the tools and
    /// commands this adapter registers, instead of a generic placeholder.
    #[must_use]
    pub fn source_info(&self) -> &SourceInfo {
        &self.source_info
    }

    /// Provider registrations queued via `pi.registerProvider` during the
    /// extension's factory invocation (pre-bind). The caller flushes these
    /// to the `ModelRegistry` after loading, mirroring TS
    /// `pendingProviderRegistrations` consumption in `bindCore()`.
    #[must_use]
    pub fn pending_providers(&self) -> &[super::js_runtime::PendingProviderRegistration] {
        &self.load_result.pending_providers
    }

    /// Send a tool execution request to the V8 runtime and await the result.
    async fn execute_tool_via_js(
        &self,
        tool_name: &str,
        tool_call_id: &str,
        params: Value,
    ) -> Result<ToolCallOutput, String> {
        let (response_tx, response_rx) = oneshot::channel();
        self.cmd_tx
            .send(JsCommand::ExecuteTool {
                tool_name: tool_name.to_string(),
                tool_call_id: tool_call_id.to_string(),
                params,
                response_tx,
            })
            .await
            .map_err(|_| "V8 runtime channel closed".to_string())?;
        response_rx.await.map_err(|_| "V8 runtime dropped response".to_string())?
    }

    /// Fire an event to JS handlers and collect their return values.
    async fn fire_event_via_js(
        &self,
        event: &str,
        data_json: &str,
    ) -> Result<serde_json::Value, String> {
        let (response_tx, response_rx) = oneshot::channel();
        self.cmd_tx
            .send(JsCommand::FireEvent {
                event: event.to_string(),
                data_json: data_json.to_string(),
                response_tx,
            })
            .await
            .map_err(|_| "V8 runtime channel closed".to_string())?;
        response_rx.await.map_err(|_| "V8 runtime dropped response".to_string())?
    }
}

#[async_trait]
impl HookHandler for JsExtensionAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn register_tools(&self, tools: &mut ToolRegistry) {
        for tool in &self.load_result.tools {
            let tool_name = tool.name.clone();
            let cmd_tx = self.cmd_tx.clone();

            let execute: ToolExecuteFn = Arc::new(
                move |_tool_call_id: String,
                      params: Value,
                      _signal: Option<tokio::sync::watch::Receiver<bool>>|
                      -> std::pin::Pin<
                    Box<
                        dyn std::future::Future<
                            Output = Result<
                                ToolCallOutput,
                                Box<dyn std::error::Error + Send + Sync>,
                            >,
                        > + Send,
                    >,
                > {
                    let cmd_tx = cmd_tx.clone();
                    let tool_name = tool_name.clone();
                    Box::pin(async move {
                        let (response_tx, response_rx) = oneshot::channel();
                        cmd_tx
                            .send(JsCommand::ExecuteTool {
                                tool_name: tool_name.clone(),
                                tool_call_id: _tool_call_id,
                                params,
                                response_tx,
                            })
                            .await
                            .map_err(|e| {
                                Box::<dyn std::error::Error + Send + Sync>::from(format!(
                                    "V8 channel send failed: {e}"
                                ))
                            })?;
                        let result = response_rx
                            .await
                            .map_err(|e| {
                                Box::<dyn std::error::Error + Send + Sync>::from(format!(
                                    "V8 response failed: {e}"
                                ))
                            })?
                            .map_err(|e| {
                                Box::<dyn std::error::Error + Send + Sync>::from(e)
                            })?;
                        Ok(result)
                    })
                },
            );

            let parameters = tool
                .parameters
                .as_ref()
                .and_then(|p| serde_json::from_str(p).ok());

            let definition = ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters,
                execute: Some(execute),
                ..Default::default()
            };

            tools.register(&tool.name, definition);
        }
    }

    fn register_commands(&self, commands: &mut CommandRegistry) {
        for cmd in &self.load_result.commands {
            let cmd_name = cmd.name.clone();
            let cmd_tx = self.cmd_tx.clone();

            let execute: Arc<
                dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                    + Send
                    + Sync,
            > = Arc::new(move |_arg: String| {
                let cmd_tx = cmd_tx.clone();
                let cmd_name = cmd_name.clone();
                Box::pin(async move {
                    let (response_tx, response_rx) = oneshot::channel();
                    if cmd_tx
                        .send(JsCommand::ExecuteCommand {
                            command_name: cmd_name.clone(),
                            response_tx,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let _ = response_rx.await;
                })
            });

            commands.register(
                &cmd.name,
                CommandRegistration {
                    description: cmd.description.clone().unwrap_or_default(),
                    execute,
                    get_argument_completions: None,
                },
            );
        }
    }

    fn register_shortcuts(&self, shortcuts: &mut ShortcutRegistry) {
        for sc in &self.load_result.shortcuts {
            shortcuts.register(
                &sc.shortcut,
                sc.description.as_deref().unwrap_or(""),
            );
        }
    }

    fn register_flags(&self, flags: &mut FlagRegistry) {
        for flag in &self.load_result.flags {
            flags.register(&flag.name, &flag.description.clone().unwrap_or_default());
        }
    }

    async fn handle_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        _ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        // Only handle tools that this extension registered.
        if !self.load_result.tools.iter().any(|t| t.name == tool_name) {
            return None;
        }
        match self.execute_tool_via_js(tool_name, "js-tool-call", params).await {
            Ok(output) => Some(output),
            Err(e) => Some(ToolCallOutput {
                content: vec![Value::String(format!("JS tool execution error: {e}"))],
                details: None,
                is_error: true,
                terminate: None,
            }),
        }
    }

    // ── Event hooks — delegate to JS handlers via channel ──────────

    async fn on_session_start(&self, _reason: &str, _previous_session_file: Option<&str>) {
        if self.load_result.handlers.iter().any(|h| h.event == "session_start") {
            let data = serde_json::json!({ "reason": _reason, "previousSessionFile": _previous_session_file });
            let _ = self.fire_event_via_js("session_start", &data.to_string()).await;
        }
    }

    async fn on_session_shutdown(&self, _reason: &str, _target_session_file: Option<&str>) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "session_shutdown")
        {
            let data = serde_json::json!({ "reason": _reason, "targetSessionFile": _target_session_file });
            let _ = self.fire_event_via_js("session_shutdown", &data.to_string()).await;
        }
    }

    async fn on_agent_start(&self) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "agent_start")
        {
            let _ = self.fire_event_via_js("agent_start", "{}").await;
        }
    }

    async fn on_agent_end(&self, _messages: &[Value]) {
        if self.load_result.handlers.iter().any(|h| h.event == "agent_end") {
            let data = serde_json::json!({ "messages": _messages });
            let _ = self.fire_event_via_js("agent_end", &data.to_string()).await;
        }
    }

    async fn on_agent_settled(&self) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "agent_settled")
        {
            let _ = self.fire_event_via_js("agent_settled", "{}").await;
        }
    }

    async fn on_turn_start(&self, _turn_index: u32) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "turn_start")
        {
            let data = serde_json::json!({ "turnIndex": _turn_index });
            let _ = self.fire_event_via_js("turn_start", &data.to_string()).await;
        }
    }

    async fn on_turn_end(&self, _turn_index: u32, _message: &Value, _tool_results: &[Value]) {
        if self.load_result.handlers.iter().any(|h| h.event == "turn_end") {
            let data = serde_json::json!({ "turnIndex": _turn_index, "message": _message, "toolResults": _tool_results });
            let _ = self.fire_event_via_js("turn_end", &data.to_string()).await;
        }
    }

    async fn on_message_start(&self, _message: &Value) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "message_start")
        {
            let data = serde_json::json!({ "message": _message });
            let _ = self.fire_event_via_js("message_start", &data.to_string()).await;
        }
    }

    async fn on_message_update(&self, _message: &Value) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "message_update")
        {
            let data = serde_json::json!({ "message": _message });
            let _ = self.fire_event_via_js("message_update", &data.to_string()).await;
        }
    }

    async fn on_message_end(&self, _message: &Value) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "message_end")
        {
            let data = serde_json::json!({ "message": _message });
            let _ = self.fire_event_via_js("message_end", &data.to_string()).await;
        }
    }

    async fn on_model_select(&self, _model: &str, _previous_model: Option<&str>) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "model_select")
        {
            let data = serde_json::json!({ "model": _model, "previousModel": _previous_model });
            let _ = self.fire_event_via_js("model_select", &data.to_string()).await;
        }
    }

    async fn on_compact(&self, _summary: &str, _tokens_before: u64) {
        if self.load_result.handlers.iter().any(|h| h.event == "session_compact") {
            let data = serde_json::json!({ "summary": _summary, "tokensBefore": _tokens_before });
            let _ = self.fire_event_via_js("session_compact", &data.to_string()).await;
        }
    }

    async fn on_tool_execution_start(
        &self,
        _tool_call_id: &str,
        _tool_name: &str,
        _args: &Value,
    ) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "tool_execution_start")
        {
            let data = serde_json::json!({
                "toolCallId": _tool_call_id,
                "toolName": _tool_name,
                "input": _args,
            });
            let _ = self
                .fire_event_via_js("tool_execution_start", &data.to_string())
                .await;
        }
    }

    async fn on_tool_execution_end(
        &self,
        _tool_call_id: &str,
        _tool_name: &str,
        _result: &Value,
        _is_error: bool,
    ) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "tool_execution_end")
        {
            let data = serde_json::json!({
                "toolCallId": _tool_call_id,
                "toolName": _tool_name,
                "result": _result,
                "isError": _is_error,
            });
            let _ = self
                .fire_event_via_js("tool_execution_end", &data.to_string())
                .await;
        }
    }

    async fn before_tool_call(
        &self,
        tool_name: String,
        args: Value,
    ) -> HookResult<(String, Value)> {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "tool_call")
        {
            let data = serde_json::json!({ "toolName": tool_name, "input": args });
            if let Ok(results) = self
                .fire_event_via_js("tool_call", &data.to_string())
                .await
            {
                // TS `tool_call` handlers may return `{block: true, reason}` to
                // veto the call (e.g. permission-gate extensions).
                if let Some(blocked) = results.as_array().and_then(|arr| {
                    arr.iter().find_map(|r| {
                        let is_block = r
                            .get("block")
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false);
                        is_block.then_some(
                            r.get("reason")
                                .and_then(|x| x.as_str())
                                .unwrap_or("blocked by extension")
                                .to_string(),
                        )
                    })
                }) {
                    return HookResult::Cancel(blocked);
                }
            }
        }
        HookResult::Continue((tool_name, args))
    }

    async fn after_tool_call(
        &self,
        _tool_name: &str,
        _result: &Value,
        _is_error: bool,
    ) -> HookResult<()> {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "tool_result")
        {
            let data = serde_json::json!({
                "toolName": _tool_name,
                "result": _result,
                "isError": _is_error,
            });
            let _ = self.fire_event_via_js("tool_result", &data.to_string()).await;
        }
        HookResult::Continue(())
    }

    // ── Remaining hooks (aligned with TS ExtensionAPI event surface) ──
    // Notification hooks fire with marshalled data; result-bearing hooks
    // parse the JS handler return values (via the FireEvent collector).

    async fn on_session_info_changed(&self, _name: Option<&str>) {
        if has_handler(&self, "session_info_changed") {
            let data = serde_json::json!({ "name": _name });
            let _ = self.fire_event_via_js("session_info_changed", &data.to_string()).await;
        }
    }

    async fn on_thinking_level_select(&self, _level: &str, _previous_level: &str) {
        if has_handler(&self, "thinking_level_select") {
            let data = serde_json::json!({ "level": _level, "previousLevel": _previous_level });
            let _ = self.fire_event_via_js("thinking_level_select", &data.to_string()).await;
        }
    }

    async fn on_tree(&self, _new_leaf_id: Option<&str>, _old_leaf_id: Option<&str>) {
        if has_handler(&self, "session_tree") {
            let data = serde_json::json!({ "newLeafId": _new_leaf_id, "oldLeafId": _old_leaf_id });
            let _ = self.fire_event_via_js("session_tree", &data.to_string()).await;
        }
    }

    async fn on_resources_discover(
        &self,
        _cwd: &str,
        _reason: &str,
    ) -> Option<pi_extension_api::ResourcesDiscoverResult> {
        if !has_handler(&self, "resources_discover") {
            return None;
        }
        let data = serde_json::json!({ "cwd": _cwd, "reason": _reason });
        if let Ok(results) = self.fire_event_via_js("resources_discover", &data.to_string()).await {
            if let Some(r) = first_result(&results) {
                return serde_json::from_value(r.clone()).ok();
            }
        }
        None
    }

    async fn on_project_trust(&self, _cwd: &str) -> Option<pi_extension_api::ProjectTrustResult> {
        if !has_handler(&self, "project_trust") {
            return None;
        }
        let data = serde_json::json!({ "cwd": _cwd });
        if let Ok(results) = self.fire_event_via_js("project_trust", &data.to_string()).await {
            if let Some(r) = first_result(&results) {
                return serde_json::from_value(r.clone()).ok();
            }
        }
        None
    }

    async fn on_user_bash(&self, _command: &str, _cwd: &str) -> Option<pi_extension_api::UserBashResult> {
        if !has_handler(&self, "user_bash") {
            return None;
        }
        let data = serde_json::json!({ "command": _command, "cwd": _cwd });
        if let Ok(results) = self.fire_event_via_js("user_bash", &data.to_string()).await {
            if let Some(r) = first_result(&results) {
                return serde_json::from_value(r.clone()).ok();
            }
        }
        None
    }

    async fn on_context(&self, _messages: &[Value]) {
        if has_handler(&self, "context") {
            let data = serde_json::json!({ "messages": _messages });
            let _ = self.fire_event_via_js("context", &data.to_string()).await;
        }
    }

    async fn on_context_mut(&self, _messages: Vec<Value>) -> HookResult<Vec<Value>> {
        if !has_handler(&self, "context") {
            return HookResult::Continue(_messages);
        }
        let data = serde_json::json!({ "messages": _messages });
        if let Ok(results) = self.fire_event_via_js("context", &data.to_string()).await {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
            if let Some(r) = first_result(&results) {
                let messages = r
                    .get("messages")
                    .and_then(|m| serde_json::from_value::<Vec<Value>>(m.clone()).ok())
                    .or_else(|| r.as_array().cloned());
                if let Some(m) = messages {
                    return HookResult::Continue(m);
                }
            }
        }
        HookResult::Continue(_messages)
    }

    async fn on_tool_result_mut(
        &self,
        _tool_name: &str,
        _content: Vec<Value>,
        _details: Option<Value>,
        _is_error: bool,
    ) -> HookResult<(Vec<Value>, Option<Value>, bool)> {
        if !has_handler(&self, "tool_result") {
            return HookResult::Continue((_content, _details, _is_error));
        }
        let data = serde_json::json!({
            "toolName": _tool_name,
            "content": _content,
            "details": _details,
            "isError": _is_error,
        });
        if let Ok(results) = self.fire_event_via_js("tool_result", &data.to_string()).await {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
            if let Some(r) = first_result(&results) {
                if let Some(content) = r
                    .get("content")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok())
                {
                    let is_error = r
                        .get("isError")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(_is_error);
                    return HookResult::Continue((content, r.get("details").cloned(), is_error));
                }
            }
        }
        HookResult::Continue((_content, _details, _is_error))
    }

    async fn before_agent_start(
        &self,
        _prompt: String,
        _images: Option<Vec<Value>>,
        _system_prompt: String,
        _system_prompt_options: Option<Value>,
    ) -> HookResult<(String, String, Option<Vec<Value>>)> {
        if !has_handler(&self, "before_agent_start") {
            return HookResult::Continue((_prompt, _system_prompt, None));
        }
        let data = serde_json::json!({
            "prompt": _prompt,
            "images": _images,
            "systemPrompt": _system_prompt,
            "systemPromptOptions": _system_prompt_options,
        });
        if let Ok(results) = self.fire_event_via_js("before_agent_start", &data.to_string()).await {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
            if let Some(r) = first_result(&results) {
                let prompt = r
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&_prompt)
                    .to_string();
                let system_prompt = r
                    .get("systemPrompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&_system_prompt)
                    .to_string();
                let images = r
                    .get("images")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok());
                return HookResult::Continue((prompt, system_prompt, images));
            }
        }
        HookResult::Continue((_prompt, _system_prompt, None))
    }

    async fn on_input(
        &self,
        _text: String,
        _images: Option<Vec<Value>>,
        _source: String,
        _streaming_behavior: Option<String>,
    ) -> HookResult<String> {
        if !has_handler(&self, "input") {
            return HookResult::Continue(_text);
        }
        let data = serde_json::json!({
            "text": _text,
            "images": _images,
            "source": _source,
            "streamingBehavior": _streaming_behavior,
        });
        if let Ok(results) = self.fire_event_via_js("input", &data.to_string()).await {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
            if let Some(r) = first_result(&results) {
                if let Some(text) = r.as_str() {
                    return HookResult::Continue(text.to_string());
                }
                if let Some(text) = r.get("text").and_then(|v| v.as_str()) {
                    return HookResult::Continue(text.to_string());
                }
            }
        }
        HookResult::Continue(_text)
    }

    async fn before_provider_request(&self, _payload: Value) -> HookResult<Value> {
        if !has_handler(&self, "before_provider_request") {
            return HookResult::Continue(_payload);
        }
        let data = serde_json::json!({ "payload": _payload });
        if let Ok(results) = self
            .fire_event_via_js("before_provider_request", &data.to_string())
            .await
        {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
            if let Some(r) = first_result(&results) {
                if let Some(p) = r.get("payload") {
                    return HookResult::Continue(p.clone());
                }
                if !r.is_null() {
                    return HookResult::Continue(r.clone());
                }
            }
        }
        HookResult::Continue(_payload)
    }

    async fn before_provider_headers(
        &self,
        _headers: std::collections::HashMap<String, String>,
    ) -> HookResult<std::collections::HashMap<String, String>> {
        if !has_handler(&self, "before_provider_headers") {
            return HookResult::Continue(_headers);
        }
        let data = serde_json::json!({ "headers": _headers });
        if let Ok(results) = self
            .fire_event_via_js("before_provider_headers", &data.to_string())
            .await
        {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
            if let Some(r) = first_result(&results) {
                if let Some(h) = r
                    .get("headers")
                    .and_then(|v| serde_json::from_value::<std::collections::HashMap<String, String>>(v.clone()).ok())
                {
                    return HookResult::Continue(h);
                }
            }
        }
        HookResult::Continue(_headers)
    }

    async fn after_provider_response(
        &self,
        _status: u16,
        _headers: std::collections::HashMap<String, String>,
    ) -> HookResult<()> {
        if has_handler(&self, "after_provider_response") {
            let data = serde_json::json!({ "status": _status, "headers": _headers });
            let _ = self.fire_event_via_js("after_provider_response", &data.to_string()).await;
        }
        HookResult::Continue(())
    }

    async fn before_session_switch(
        &self,
        _reason: String,
        _target_session_file: Option<String>,
    ) -> HookResult<()> {
        if !has_handler(&self, "session_before_switch") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "reason": _reason, "targetSessionFile": _target_session_file });
        if let Ok(results) = self
            .fire_event_via_js("session_before_switch", &data.to_string())
            .await
        {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
        }
        HookResult::Continue(())
    }

    async fn before_session_fork(&self, _entry_id: String, _position: String) -> HookResult<()> {
        if !has_handler(&self, "session_before_fork") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "entryId": _entry_id, "position": _position });
        if let Ok(results) = self
            .fire_event_via_js("session_before_fork", &data.to_string())
            .await
        {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
        }
        HookResult::Continue(())
    }

    async fn before_session_compact(&self, _reason: String, _will_retry: bool) -> HookResult<()> {
        if !has_handler(&self, "session_before_compact") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "reason": _reason, "willRetry": _will_retry });
        if let Ok(results) = self
            .fire_event_via_js("session_before_compact", &data.to_string())
            .await
        {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
        }
        HookResult::Continue(())
    }

    async fn before_session_tree(&self, _target_id: String) -> HookResult<()> {
        if !has_handler(&self, "session_before_tree") {
            return HookResult::Continue(());
        }
        let data = serde_json::json!({ "targetId": _target_id });
        if let Ok(results) = self
            .fire_event_via_js("session_before_tree", &data.to_string())
            .await
        {
            if let Some(reason) = first_block_reason(&results) {
                return HookResult::Cancel(reason);
            }
        }
        HookResult::Continue(())
    }
}

/// Whether any loaded handler for `event` exists.
fn has_handler(adapter: &JsExtensionAdapter, event: &str) -> bool {
    adapter.load_result.handlers.iter().any(|h| h.event == event)
}

/// Find the first non-null handler return value.
fn first_result(results: &Value) -> Option<&Value> {
    // `undefined` JS values are dropped by the FireEvent collector, so
    // only null needs filtering here.
    results.as_array()?.iter().find(|r| !r.is_null())
}

/// Find the first `{block: true, reason}` among handler return values.
fn first_block_reason(results: &Value) -> Option<String> {
    results.as_array()?.iter().find_map(|r| {
        let is_block = r.get("block").and_then(|b| b.as_bool()).unwrap_or(false);
        is_block.then_some(
            r.get("reason")
                .and_then(|x| x.as_str())
                .unwrap_or("blocked by extension")
                .to_string(),
        )
    })
}

// ============================================================================
// JsExtensionManager — owns the V8 runtime + processes JsCommands
// ============================================================================

/// Manages the V8 runtime lifecycle and processes `JsCommand`s from adapters.
///
/// The V8 runtime is `!Send`, so this manager runs on a dedicated thread with
/// a current-thread tokio runtime + `LocalSet`. Adapters communicate via
/// `mpsc::Sender<JsCommand>`.
pub struct JsExtensionManager {
    /// Channel for adapters to send commands to the V8 runtime.
    cmd_tx: mpsc::Sender<JsCommand>,
    /// Handle to the V8 runtime thread. Joining it shuts down the runtime.
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl JsExtensionManager {
    /// Spawn the V8 runtime manager thread.
    ///
    /// Returns the manager (which holds the channel sender) and a
    /// `JsExtensionRuntime` handle for loading extensions. The runtime is
    /// accessible via the closure passed to `spawn`.
    ///
    /// The `load_fn` closure is called on the V8 thread to load extensions
    /// and produce adapters. It receives the `JsExtensionRuntime` and the
    /// command receiver.
    pub fn spawn() -> (Self, mpsc::Sender<JsCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<JsCommand>(64);

        let thread_tx = cmd_tx.clone();
        let thread_handle = std::thread::Builder::new()
            .name("pi-v8-runtime".into())
            .spawn(move || {
                Self::run_v8_thread(thread_tx, cmd_rx);
            })
            .expect("spawn V8 thread");

        let manager = Self {
            cmd_tx: cmd_tx.clone(),
            thread_handle: Some(thread_handle),
        };

        (manager, cmd_tx)
    }

    /// Get a clone of the command channel sender (for creating new adapters).
    #[must_use]
    pub fn command_sender(&self) -> mpsc::Sender<JsCommand> {
        self.cmd_tx.clone()
    }

    /// Bind the runtime core: install `RuntimeActions` closures so that
    /// post-load action ops (sendMessage, registerProvider, etc.) delegate
    /// to the host. Mirrors TS `ExtensionRunner.bindCore()`. Must be called
    /// after all extensions are loaded and after pending provider
    /// registrations are flushed to the `ModelRegistry`.
    ///
    /// # Errors
    /// Returns a `String` if the V8 runtime thread has already shut down.
    pub async fn bind_core(
        &self,
        actions: super::js_runtime::RuntimeActions,
    ) -> Result<(), String> {
        let (response_tx, response_rx) = oneshot::channel();
        self.cmd_tx
            .send(JsCommand::BindCore {
                actions,
                response_tx,
            })
            .await
            .map_err(|_| "V8 runtime channel closed".to_string())?;
        response_rx
            .await
            .map_err(|_| "V8 runtime dropped response".to_string())?
    }

    /// Load a JS/TS extension file on the V8 thread and return the
    /// captured registration metadata. The JS callbacks (tool execute,
    /// event handlers, command handlers) stay alive in the V8 runtime
    /// for later invocation via `JsCommand`.
    ///
    /// Returns `(ExtensionLoadResult, command_sender)` — the sender is
    /// returned so the caller can create a `JsExtensionAdapter` with it.
    pub async fn load_extension(
        &self,
        path: std::path::PathBuf,
        cwd: std::path::PathBuf,
    ) -> Result<ExtensionLoadResult, String> {
        let (response_tx, response_rx) = oneshot::channel();
        self.cmd_tx
            .send(JsCommand::LoadExtension {
                path,
                cwd,
                response_tx,
            })
            .await
            .map_err(|_| "V8 runtime channel closed".to_string())?;
        response_rx
            .await
            .map_err(|_| "V8 runtime dropped response".to_string())?
    }

    /// Run the V8 runtime thread: create the runtime, load extensions, and
    /// process commands from the channel.
    fn run_v8_thread(_cmd_tx: mpsc::Sender<JsCommand>, mut cmd_rx: mpsc::Receiver<JsCommand>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime for V8 thread");

        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let mut js_runtime = match super::js_runtime::JsExtensionRuntime::new() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("V8 runtime init failed: {e}");
                    return;
                }
            };

            // Process commands from adapters.
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    JsCommand::LoadExtension { path, cwd, response_tx } => {
                        match js_runtime.load_extension(&path, &cwd).await {
                            Ok(()) => {
                                let result = js_runtime.take_result();
                                let _ = response_tx.send(Ok(result));
                            }
                            Err(e) => {
                                let _ = response_tx.send(Err(format!("{e}")));
                            }
                        }
                    }
                    JsCommand::ExecuteTool {
                        tool_name,
                        tool_call_id,
                        params,
                        response_tx,
                    } => {
                        let params_json = serde_json::to_string(&params)
                            .unwrap_or_else(|_| "null".into());
                        let script = build_tool_execute_script(
                            &tool_name,
                            &tool_call_id,
                            &params_json,
                        );
                        // Execute the async JS, pump the event loop, and
                        // extract the result as a JSON string in one call.
                        match js_runtime
                            .execute_async_and_get_json("<tool-exec>", &script)
                            .await
                        {
                            Ok(json_str) => {
                                let output = match serde_json::from_str::<JsToolResult>(&json_str)
                                {
                                    Ok(result) => result.into_output(),
                                    // The JS callback returned a value that
                                    // doesn't match AgentToolResult — wrap the
                                    // raw JSON as a single text content block
                                    // so the agent still sees something.
                                    Err(_) => ToolCallOutput {
                                        content: vec![Value::String(json_str.clone())],
                                        details: Some(Value::String(json_str)),
                                        is_error: false,
                                        terminate: None,
                                    },
                                };
                                let _ = response_tx.send(Ok(output));
                            }
                            Err(e) => {
                                // The JS callback threw or the event loop
                                // failed. Surface it as an error tool output so
                                // the agent can react, mirroring how the TS
                                // runtime reports tool execution failures.
                                let _ = response_tx.send(Ok(ToolCallOutput {
                                    content: vec![Value::String(format!(
                                        "JS tool execution error: {e}"
                                    ))],
                                    details: None,
                                    is_error: true,
                                    terminate: None,
                                }));
                            }
                        }
                    }
                    JsCommand::BindCore {
                        actions,
                        response_tx,
                    } => {
                        js_runtime.bind_core(actions);
                        let _ = response_tx.send(Ok(()));
                    }
                    JsCommand::Invalidate => {
                        js_runtime.invalidate();
                    }
                    JsCommand::Shutdown => break,
                    JsCommand::FireEvent {
                        event,
                        data_json,
                        response_tx,
                    } => {
                        let script = format!(
                            "(async () => {{
                              const handlers = globalThis.__pi.__handlers.get({event:?}) ?? [];
                              const results = [];
                              const ctx = globalThis.__pi.createCtx();
                              for (const h of handlers) {{
                                const r = await h({data}, ctx);
                                if (r !== undefined) results.push(r);
                              }}
                              return results;
                            }})()",
                            event = event,
                            data = data_json,
                        );
                        // Collect handler return values as a JSON array (used
                        // by result-bearing hooks like `tool_call` -> `{block}`).
                        match js_runtime
                            .execute_async_and_get_json("<event-fire>", &script)
                            .await
                        {
                            Ok(json_str) => {
                                let value = serde_json::from_str(&json_str)
                                    .unwrap_or(serde_json::Value::Array(Vec::new()));
                                let _ = response_tx.send(Ok(value));
                            }
                            Err(e) => {
                                let _ = response_tx.send(Err(format!("V8 fire: {e}")));
                            }
                        }
                    }
                    JsCommand::ExecuteCommand {
                        command_name,
                        response_tx,
                    } => {
                        let script = format!(
                            "(async () => {{
                              const fn = globalThis.__pi.__commandHandlers.get({name:?});
                              if (fn) await fn();
                            }})()",
                            name = command_name,
                        );
                        match js_runtime.execute_script("<cmd-exec>", &script) {
                            Ok(_) => {
                                if let Err(e) = js_runtime.run_event_loop()
                                    .await
                                {
                                    let _ = response_tx.send(Err(format!("V8 event loop: {e}")));
                                    continue;
                                }
                                let _ = response_tx.send(Ok(()));
                            }
                            Err(e) => {
                                let _ = response_tx.send(Err(format!("V8 execute: {e}")));
                            }
                        }
                    }
                }
            }
        });
    }

    /// Invalidate the extension runtime context (stale-ctx guard): after a
    /// session change (new/fork/switch/reload), extensions that hold stale
    /// contexts must have their action ops throw. Mirrors TS
    /// `runtime.invalidate()`. The intended wiring point is
    /// `AgentSessionRuntime::set_before_session_invalidate`.
    ///
    /// # Errors
    /// Returns a `String` if the V8 runtime thread has already shut down.
    pub async fn invalidate(&self) -> Result<(), String> {
        self.cmd_tx
            .send(JsCommand::Invalidate)
            .await
            .map_err(|_| "V8 runtime channel closed".to_string())
    }

    /// Synchronous invalidate (best-effort, no await) — usable from a sync
    /// `Fn()` callback (e.g. the session-switch invalidator).
    pub fn invalidate_sync(&self) {
        let _ = self.cmd_tx.try_send(JsCommand::Invalidate);
    }

    /// Shut down the V8 runtime (drop the channel sender and join the thread).
    ///
    /// Sends an explicit `Shutdown` command so the V8 thread breaks out of
    /// its command loop deterministically — even if adapter-held sender
    /// clones are still alive (they would otherwise keep the channel open
    /// and `recv()` would never observe closure). After sending, we drop our
    /// own sender and join the thread.
    pub fn shutdown(&mut self) {
        // Best-effort: send Shutdown. If the channel is already closed (thread
        // exited), this is a no-op.
        let _ = self.cmd_tx.try_send(JsCommand::Shutdown);
        // Release our sender so the channel can drain.
        let (dummy_tx, _dummy_rx) = mpsc::channel::<JsCommand>(1);
        let _real_sender = std::mem::replace(&mut self.cmd_tx, dummy_tx);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for JsExtensionManager {
    fn drop(&mut self) {
        if self.thread_handle.is_some() {
            self.shutdown();
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use pi_extension_api::ToolRegistry;

    #[test]
    fn test_adapter_registration() {
        let (cmd_tx, _cmd_rx) = mpsc::channel::<JsCommand>(8);
        let load_result = ExtensionLoadResult {
            tools: vec![super::super::js_runtime::LoadedToolRecord {
                name: "search".into(),
                description: "search the web".into(),
                parameters: None,
            }],
            commands: vec![super::super::js_runtime::LoadedCommandRecord {
                name: "greet".into(),
                description: Some("say hi".into()),
                subcommands: vec![],
            }],
            shortcuts: vec![super::super::js_runtime::LoadedShortcutRecord {
                shortcut: "ctrl+k".into(),
                description: Some("shortcut 1".into()),
            }],
            flags: vec![super::super::js_runtime::LoadedFlagRecord {
                name: "verbose".into(),
                flag_type: "boolean".into(),
                description: Some("verbose mode".into()),
                default_value: Some("false".into()),
            }],
            ..Default::default()
        };

        let adapter = JsExtensionAdapter::new("/path/to/my-ext.ts", load_result, cmd_tx);
        assert_eq!(adapter.name(), "my-ext");

        let mut tools = ToolRegistry::new(adapter.source_info().clone());
        adapter.register_tools(&mut tools);
        assert_eq!(tools.into_vec().len(), 1);
    }

    /// Block on a future using a fresh current-thread tokio runtime (the V8
    /// runtime runs on its own thread with its own runtime; communication is
    /// via channels, so the test runtime just needs to drive the adapter side).
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
            .block_on(future)
    }

    /// End-to-end: load an extension whose tool `execute` returns a real
    /// `AgentToolResult`, then invoke the registered tool's `ToolExecuteFn`
    /// and verify the extracted `ToolCallOutput`.
    #[test]
    fn test_tool_execution_result_extraction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = {
            let path = dir.path().join("echo-ext.ts");
            std::fs::write(
                &path,
                r#"
export default async function(pi) {
  pi.registerTool({
    name: "echo",
    description: "echo back the param",
    execute: async (toolCallId, params, signal, onUpdate, ctx) => {
      const n = (params && params.n) || 0;
      return {
        content: [{ type: "text", text: "echo: " + n }],
        details: { toolCallId, doubled: n * 2 },
        terminate: false,
      };
    },
  });
}
"#,
            )
            .expect("write extension");
            path
        };
        let (manager, _cmd_tx) = JsExtensionManager::spawn();
        let load_result = block_on(manager.load_extension(
            ext.clone(),
            dir.path().to_path_buf(),
        ))
        .expect("load_extension");
        assert_eq!(load_result.tools.len(), 1);

        let adapter = JsExtensionAdapter::new(
            &ext.to_string_lossy(),
            load_result,
            manager.command_sender(),
        );

        let mut tools = ToolRegistry::new(adapter.source_info().clone());
        adapter.register_tools(&mut tools);
        let registered = tools.into_vec();
        assert_eq!(registered.len(), 1);
        let tool = &registered[0];
        assert_eq!(tool.name, "echo");
        let execute = tool.definition.execute.clone().expect("tool has execute fn");

        let output = block_on(execute(
            "call-1".to_string(),
            serde_json::json!({ "n": 21 }),
            None,
        ))
        .expect("tool execute");
        assert!(!output.is_error, "should not be an error");
        assert!(output.terminate == Some(false));
        // content is the raw JSON array of TextContent objects.
        assert_eq!(output.content.len(), 1);
        assert_eq!(output.content[0]["type"], "text");
        assert_eq!(output.content[0]["text"], "echo: 21");
        // details carries the structured object from JS.
        let details = output.details.expect("details present");
        assert_eq!(details["toolCallId"], "call-1");
        assert_eq!(details["doubled"], 42);

        // `manager` drops last (natural reverse-order drop), after `registered`
        // and `execute` release their cmd_tx clones — so the V8 thread exits
        // cleanly during shutdown().
    }

    /// A tool whose `execute` throws should surface as an `is_error` output.
    #[test]
    fn test_tool_execution_error_surfaces_as_is_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = {
            let path = dir.path().join("boom-ext.ts");
            std::fs::write(
                &path,
                r#"
export default async function(pi) {
  pi.registerTool({
    name: "boom",
    description: "always throws",
    execute: async () => {
      throw new Error("kaboom");
    },
  });
}
"#,
            )
            .expect("write extension");
            path
        };

        let (manager, _cmd_tx) = JsExtensionManager::spawn();
        let load_result = block_on(manager.load_extension(
            ext.clone(),
            dir.path().to_path_buf(),
        ))
        .expect("load_extension");

        let adapter = JsExtensionAdapter::new(
            &ext.to_string_lossy(),
            load_result,
            manager.command_sender(),
        );
        let mut tools = ToolRegistry::new(adapter.source_info().clone());
        adapter.register_tools(&mut tools);
        let registered = tools.into_vec();
        let execute = registered[0].definition.execute.clone().expect("execute fn");

        let output = block_on(execute("c".to_string(), serde_json::json!({}), None))
            .expect("execute returns output");
        assert!(output.is_error, "error must be flagged");
        assert!(output.content[0].is_string());
        assert!(output.content[0]
            .as_str()
            .unwrap()
            .contains("kaboom"));
    }

    /// End-to-end integration: create a .ts extension that registers a tool,
    /// a command, and a provider; load it via the V8 runtime; register the
    /// adapter into an `ExtensionRegistry`; flush pending providers to a
    /// `ModelRegistry`; bind core; then execute the tool and verify output.
    ///
    /// This exercises the full pipeline that `sdk.rs::create_agent_session`
    /// uses, but in isolation (without creating a full `AgentSession`).
    #[test]
    fn test_e2e_extension_full_pipeline() {
        use crate::core::model_registry::{ModelRegistry, ProviderConfig};
        use crate::core::slash_commands::resolve_extension_commands;
        use pi_extension_api::ExtensionRegistry;

        let dir = tempfile::tempdir().expect("tempdir");
        let ext = {
            let path = dir.path().join("full-e2e.ts");
            std::fs::write(
                &path,
                r#"
export default async function(pi) {
  // Register a tool that echoes params.
  pi.registerTool({
    name: "calc",
    description: "calculate something",
    execute: async (toolCallId, params) => {
      const x = (params && params.x) || 0;
      return {
        content: [{ type: "text", text: "result: " + (x * 3) }],
        details: { toolCallId, input: x },
        terminate: false,
      };
    },
  });

  // Register a command.
  pi.registerCommand("summarize", {
    description: "Summarize the conversation",
    handler: async () => {},
  });

  // Register a provider (pre-bind, queued as pending).
  pi.registerProvider("e2e-provider", {
    baseUrl: "http://e2e.test:9999",
    apiKey: "secret-key",
    api: "openai",
    authHeader: true,
  });

  pi.log("e2e loaded");
}
"#,
            )
            .expect("write extension");
            path
        };

        // 1. Load the extension via V8.
        let (manager, _cmd_tx) = JsExtensionManager::spawn();
        let load_result = block_on(manager.load_extension(
            ext.clone(),
            dir.path().to_path_buf(),
        ))
        .expect("load_extension");

        // 2. Verify captured metadata.
        assert_eq!(load_result.tools.len(), 1);
        assert_eq!(load_result.tools[0].name, "calc");
        assert_eq!(load_result.commands.len(), 1);
        assert_eq!(load_result.commands[0].name, "summarize");
        assert_eq!(load_result.pending_providers.len(), 1);
        assert_eq!(load_result.pending_providers[0].name, "e2e-provider");
        assert!(load_result.pending_providers[0].config_json.contains("e2e.test"));

        // 3. Create adapter and register into ExtensionRegistry.
        let adapter = JsExtensionAdapter::new(
            &ext.to_string_lossy(),
            load_result,
            manager.command_sender(),
        );
        let source_info = adapter.source_info().clone();

        // Flush pending providers to a ModelRegistry (mirrors sdk.rs).
        let model_registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
        for pending in adapter.pending_providers() {
            let config: ProviderConfig =
                serde_json::from_str(&pending.config_json).expect("parse provider config");
            model_registry.register_provider(&pending.name, config);
        }

        let mut ext_registry = ExtensionRegistry::new();
        ext_registry.register(Box::new(adapter), source_info);

        // 4. Verify tools are in the registry.
        let tools = ext_registry.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].definition.name, "calc");

        // 5. Verify commands are in the registry and resolve correctly.
        let commands = ext_registry.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "summarize");
        let resolved = resolve_extension_commands(commands);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].invocation_name, "summarize");
        assert_eq!(resolved[0].name, "summarize");

        // 6. Verify the provider was registered in ModelRegistry.
        let providers = model_registry.get_registered_providers();
        assert!(providers.contains(&"e2e-provider".to_string()),
            "e2e-provider should be in registered_providers");
        let provider_config = model_registry
            .get_provider_config("e2e-provider")
            .expect("provider config exists");
        assert_eq!(provider_config.base_url.as_deref(), Some("http://e2e.test:9999"));
        assert_eq!(provider_config.api_key.as_deref(), Some("secret-key"));
        assert_eq!(provider_config.api.as_deref(), Some("openai"));
        assert_eq!(provider_config.auth_header, Some(true));

        // 7. Clone the registry for the bind_core closures (shares the Arc).
        let registry_for_register = model_registry.clone();
        let registry_for_unregister = model_registry.clone();

        // 8. Bind core with register_provider/unregister_provider closures.
        let actions = super::super::js_runtime::RuntimeActions {
            register_provider: Some(std::sync::Arc::new(
                move |name: String, config_json: String, _ext: String| {
                    if let Ok(config) = serde_json::from_str::<ProviderConfig>(&config_json) {
                        registry_for_register.register_provider(&name, config);
                    }
                },
            )),
            unregister_provider: Some(std::sync::Arc::new(
                move |name: String| {
                    registry_for_unregister.unregister_provider(&name);
                },
            )),
            ..Default::default()
        };
        block_on(manager.bind_core(actions)).expect("bind_core");

        // 9. Execute the tool and verify output.
        let tool = &tools[0];
        let execute = tool.definition.execute.clone().expect("execute fn");
        let output = block_on(execute(
            "e2e-call-1".to_string(),
            serde_json::json!({ "x": 7 }),
            None,
        ))
        .expect("tool execute");
        assert!(!output.is_error);
        assert_eq!(output.content.len(), 1);
        assert_eq!(output.content[0]["type"], "text");
        assert_eq!(output.content[0]["text"], "result: 21");
        let details = output.details.expect("details present");
        assert_eq!(details["toolCallId"], "e2e-call-1");
        assert_eq!(details["input"], 7);

        // 10. Verify the ModelRegistry clone (from the closure) shares the
        //     same registered_providers map — unregister via the clone
        //     path would affect the original. Just verify the provider is
        //     still there (the closure hasn't been called, but the Arc is shared).
        let providers2 = model_registry.get_registered_providers();
        assert!(providers2.contains(&"e2e-provider".to_string()));
    }

    /// A `tool_call` handler returning `{block: true, reason}` must veto the
    /// call via `HookResult::Cancel` — the permission-gate pattern. Previously
    /// the JS handler's return value was dropped, so blocking never worked.
    #[test]
    fn test_before_tool_call_block_interception() {
        use pi_extension_api::HookResult;
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = {
            let path = dir.path().join("gate.ts");
            std::fs::write(
                &path,
                r#"
export default async function(pi) {
  pi.on("tool_call", async (event) => {
    const cmd = String((event.input && event.input.command) || "");
    if (event.toolName === "bash" && cmd.includes("rm -rf")) {
      return { block: true, reason: "dangerous command blocked" };
    }
    return undefined;
  });
}
"#,
            )
            .expect("write extension");
            path
        };

        let (manager, _cmd_tx) = JsExtensionManager::spawn();
        let load_result =
            block_on(manager.load_extension(ext.clone(), dir.path().to_path_buf()))
                .expect("load_extension");
        assert_eq!(load_result.handlers.len(), 1);
        assert_eq!(load_result.handlers[0].event, "tool_call");

        let adapter = JsExtensionAdapter::new(
            &ext.to_string_lossy(),
            load_result,
            manager.command_sender(),
        );

        // Dangerous bash → blocked with the handler's reason.
        let result = block_on(adapter.before_tool_call(
            "bash".to_string(),
            serde_json::json!({ "command": "rm -rf /" }),
        ));
        assert!(
            result.is_cancel(),
            "dangerous tool_call must be blocked, got {result:?}"
        );
        if let HookResult::Cancel(reason) = result {
            assert_eq!(reason, "dangerous command blocked");
        }

        // Safe bash → continue.
        let result = block_on(adapter.before_tool_call(
            "bash".to_string(),
            serde_json::json!({ "command": "ls" }),
        ));
        assert!(result.is_continue(), "safe tool_call must continue");
    }

    /// Event handlers must receive the marshalled event payload (not `{}`),
    /// and their return values must come back through `fire_event_via_js`.
    #[test]
    fn test_event_payload_reaches_handler() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = {
            let path = dir.path().join("payload.ts");
            std::fs::write(
                &path,
                r#"
export default async function(pi) {
  pi.on("tool_call", async (event) => {
    if (event && event.input) {
      return { seenCommand: String(event.input.command || "") };
    }
    return undefined;
  });
}
"#,
            )
            .expect("write extension");
            path
        };

        let (manager, _cmd_tx) = JsExtensionManager::spawn();
        let load_result =
            block_on(manager.load_extension(ext.clone(), dir.path().to_path_buf()))
                .expect("load_extension");
        let adapter = JsExtensionAdapter::new(
            &ext.to_string_lossy(),
            load_result,
            manager.command_sender(),
        );

        let results = block_on(adapter.fire_event_via_js(
            "tool_call",
            r#"{"toolName":"bash","input":{"command":"ls -la"}}"#,
        ))
        .expect("fire_event_via_js");
        let arr = results.as_array().expect("results must be an array");
        assert_eq!(arr.len(), 1, "one handler registered");
        assert_eq!(arr[0]["seenCommand"], "ls -la", "handler must see the payload");
    }

    /// Event handlers receive the second `ctx` argument (TS `(event, ctx)`),
    /// so `ctx.hasUI` / `ctx.cwd` guards work instead of crashing.
    #[test]
    fn test_event_handler_receives_ctx() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = {
            let path = dir.path().join("ctx.ts");
            std::fs::write(
                &path,
                r#"
export default async function(pi) {
  pi.on("tool_call", async (event, ctx) => {
    // Permission-gate style guard: uses ctx without crashing.
    return {
      hasUI: ctx.hasUI,
      mode: ctx.mode,
      cwd: ctx.cwd,
      isIdle: ctx.isIdle(),
      canSelect: typeof ctx.ui.select === "function",
    };
  });
}
"#,
            )
            .expect("write extension");
            path
        };

        let (manager, _cmd_tx) = JsExtensionManager::spawn();
        let load_result =
            block_on(manager.load_extension(ext.clone(), dir.path().to_path_buf()))
                .expect("load_extension");
        let adapter = JsExtensionAdapter::new(
            &ext.to_string_lossy(),
            load_result,
            manager.command_sender(),
        );

        let results = block_on(adapter.fire_event_via_js(
            "tool_call",
            r#"{"toolName":"bash","input":{}}"#,
        ))
        .expect("fire_event_via_js");
        let r = &results.as_array().expect("array")[0];
        assert_eq!(r["hasUI"], false, "no UI in fast mode");
        assert_eq!(r["mode"], "fast");
        assert!(r["isIdle"].as_bool().unwrap_or(false), "ctx.isIdle() must work");
        assert_eq!(r["canSelect"], true, "ctx.ui.select must exist");
        let _ = r["cwd"]; // cwd present (may be empty in tests)
    }

    /// A `before_agent_start` handler can modify the prompt (result-bearing
    /// hook) — its return value is parsed and applied.
    #[test]
    fn test_before_agent_start_modifies_prompt() {
        use pi_extension_api::HookResult;
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = {
            let path = dir.path().join("preagent.ts");
            std::fs::write(
                &path,
                r#"
export default async function(pi) {
  pi.on("before_agent_start", async (event) => {
    return { prompt: "PREFIX: " + event.prompt, systemPrompt: event.systemPrompt };
  });
}
"#,
            )
            .expect("write extension");
            path
        };

        let (manager, _cmd_tx) = JsExtensionManager::spawn();
        let load_result =
            block_on(manager.load_extension(ext.clone(), dir.path().to_path_buf()))
                .expect("load_extension");
        let adapter = JsExtensionAdapter::new(
            &ext.to_string_lossy(),
            load_result,
            manager.command_sender(),
        );

        let result = block_on(adapter.before_agent_start(
            "hello".to_string(),
            None,
            "sys".to_string(),
            None,
        ));
        match result {
            HookResult::Continue((prompt, system_prompt, _)) => {
                assert_eq!(prompt, "PREFIX: hello");
                assert_eq!(system_prompt, "sys");
            }
            HookResult::Cancel(reason) => panic!("unexpected cancel: {reason}"),
        }
    }
}

// ============================================================================
// load_js_extensions — top-level integration: discover + load + adapt
// ============================================================================

use super::loader::discover_extension_paths;
use std::path::PathBuf;

/// Result of loading JS extensions: adapters to register + the manager
/// (which must be kept alive for the lifetime of the session).
pub struct JsExtensionsLoaded {
    /// Adapters ready to be registered into `ExtensionRegistry`.
    pub adapters: Vec<JsExtensionAdapter>,
    /// The V8 runtime manager — must be kept alive (dropping it shuts down V8).
    pub manager: JsExtensionManager,
}

/// Discover, load, and adapt JS/TS extensions from the configured paths
/// and extension directories.
///
/// This is the live counterpart to the dead `extension_paths` parameter:
/// it uses `loader::discover_extension_paths` to find extension files,
/// then loads each one via the V8 runtime and creates a `JsExtensionAdapter`.
///
/// Returns `None` if no extension files were discovered (no V8 thread
/// is spawned in that case).
///
/// # Errors
/// Returns a `Vec<String>` of per-extension load errors (extensions that
/// failed to load are skipped; successful ones are still returned).
pub async fn load_js_extensions(
    extension_paths: &[String],
    cwd: &str,
    agent_dir: &str,
) -> Result<Option<JsExtensionsLoaded>, Vec<String>> {
    let discovered = discover_extension_paths(extension_paths, cwd, agent_dir);
    if discovered.paths.is_empty() {
        return Ok(None);
    }

    let (manager, cmd_tx) = JsExtensionManager::spawn();
    let mut adapters = Vec::new();
    let mut errors = Vec::new();

    for ext_path in &discovered.paths {
        let path_str = ext_path.to_string_lossy().to_string();
        match manager
            .load_extension(ext_path.clone(), PathBuf::from(cwd))
            .await
        {
            Ok(load_result) => {
                if load_result.tools.is_empty()
                    && load_result.commands.is_empty()
                    && load_result.shortcuts.is_empty()
                    && load_result.flags.is_empty()
                    && load_result.handlers.is_empty()
                    && load_result.message_renderers.is_empty()
                    && load_result.entry_renderers.is_empty()
                    && load_result.pending_providers.is_empty()
                {
                    // Extension loaded but registered nothing — skip.
                    continue;
                }
                let adapter = JsExtensionAdapter::new(&path_str, load_result, cmd_tx.clone());
                adapters.push(adapter);
            }
            Err(e) => {
                errors.push(format!("Failed to load extension {path_str}: {e}"));
            }
        }
    }

    if adapters.is_empty() && errors.is_empty() {
        // Nothing loaded; shut down the V8 thread.
        drop(manager);
        return Ok(None);
    }

    Ok(Some(JsExtensionsLoaded { adapters, manager }))
}
