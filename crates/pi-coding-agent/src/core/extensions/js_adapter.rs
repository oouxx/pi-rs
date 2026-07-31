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
    FlagRegistry, HookHandler, HookResult, RegisteredFlag, RegisteredShortcut, ShortcutRegistry,
    SourceInfo, SourceOrigin, SourceScope, ToolCallOutput, ToolDefinition, ToolExecuteFn,
    ToolRegistry,
};

use super::js_runtime::ExtensionLoadResult;

// ============================================================================
// JsCommand — channel protocol between adapter and V8 runtime
// ============================================================================

/// A command sent from the adapter (running on any tokio task) to the V8
/// runtime task (running on a current-thread `LocalSet`).
#[derive(Debug)]
pub enum JsCommand {
    /// Execute a registered tool's JS `execute` callback.
    ExecuteTool {
        tool_name: String,
        tool_call_id: String,
        params: Value,
        response_tx: oneshot::Sender<Result<ToolCallOutput, String>>,
    },
    /// Fire an event to all registered JS handlers for that event name.
    FireEvent {
        event: String,
        data_json: String,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Execute a registered command's JS handler.
    ExecuteCommand {
        command_name: String,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
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

    /// Fire an event to JS handlers.
    async fn fire_event_via_js(&self, event: &str, data_json: &str) -> Result<(), String> {
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
            let _ = self.fire_event_via_js("session_start", "{}").await;
        }
    }

    async fn on_session_shutdown(&self, _reason: &str, _target_session_file: Option<&str>) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "session_shutdown")
        {
            let _ = self.fire_event_via_js("session_shutdown", "{}").await;
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
            let _ = self.fire_event_via_js("agent_end", "{}").await;
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
            let _ = self.fire_event_via_js("turn_start", "{}").await;
        }
    }

    async fn on_turn_end(&self, _turn_index: u32, _message: &Value, _tool_results: &[Value]) {
        if self.load_result.handlers.iter().any(|h| h.event == "turn_end") {
            let _ = self.fire_event_via_js("turn_end", "{}").await;
        }
    }

    async fn on_message_start(&self, _message: &Value) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "message_start")
        {
            let _ = self.fire_event_via_js("message_start", "{}").await;
        }
    }

    async fn on_message_update(&self, _message: &Value) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "message_update")
        {
            let _ = self.fire_event_via_js("message_update", "{}").await;
        }
    }

    async fn on_message_end(&self, _message: &Value) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "message_end")
        {
            let _ = self.fire_event_via_js("message_end", "{}").await;
        }
    }

    async fn on_model_select(&self, _model: &str, _previous_model: Option<&str>) {
        if self
            .load_result
            .handlers
            .iter()
            .any(|h| h.event == "model_select")
        {
            let _ = self.fire_event_via_js("model_select", "{}").await;
        }
    }

    async fn on_compact(&self, _summary: &str, _tokens_before: u64) {
        if self.load_result.handlers.iter().any(|h| h.event == "session_compact") {
            let _ = self.fire_event_via_js("session_compact", "{}").await;
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
            let _ = self
                .fire_event_via_js("tool_execution_start", "{}")
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
            let _ = self
                .fire_event_via_js("tool_execution_end", "{}")
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
            let data = serde_json::json!({ "toolName": tool_name, "args": args });
            let _ = self
                .fire_event_via_js("tool_call", &data.to_string())
                .await;
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
            let _ = self.fire_event_via_js("tool_result", "{}").await;
        }
        HookResult::Continue(())
    }
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
                    JsCommand::ExecuteTool {
                        tool_name,
                        tool_call_id,
                        params,
                        response_tx,
                    } => {
                        let params_json = serde_json::to_string(&params)
                            .unwrap_or_else(|_| "null".into());
                        let script = format!(
                            "(async () => {{
                              const fn = globalThis.__pi.__toolExecutors.get({name:?});
                              if (!fn) throw new Error('Tool not found: ' + {name:?});
                              const result = await fn({params}, {call_id:?});
                              return result;
                            }})()",
                            name = tool_name,
                            params = params_json,
                            call_id = tool_call_id,
                        );
                        match js_runtime.execute_script("<tool-exec>", &script) {
                            Ok(_) => {
                                // Run the event loop to let async callbacks complete.
                                if let Err(e) = js_runtime.run_event_loop()
                                    .await
                                {
                                    let _ = response_tx.send(Err(format!("V8 event loop: {e}")));
                                    continue;
                                }
                                // TODO: extract the actual result from V8.
                                // For now, return a success output.
                                let _ = response_tx.send(Ok(ToolCallOutput {
                                    content: vec![Value::String(
                                        "JS tool executed (result extraction pending)".into(),
                                    )],
                                    details: None,
                                    is_error: false,
                                    terminate: None,
                                }));
                            }
                            Err(e) => {
                                let _ = response_tx.send(Err(format!("V8 execute: {e}")));
                            }
                        }
                    }
                    JsCommand::FireEvent {
                        event,
                        data_json,
                        response_tx,
                    } => {
                        let script = format!(
                            "(async () => {{
                              const handlers = globalThis.__pi.__handlers.get({event:?}) ?? [];
                              for (const h of handlers) {{
                                await h({data});
                              }}
                            }})()",
                            event = event,
                            data = data_json,
                        );
                        match js_runtime.execute_script("<event-fire>", &script) {
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

    /// Shut down the V8 runtime (drop the channel sender and join the thread).
    pub fn shutdown(&mut self) {
        // Dropping cmd_tx will cause the recv loop to exit.
        // The thread will exit after processing remaining commands.
        drop(self.cmd_tx.clone());
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
    use pi_extension_api::{HookRunner, ToolRegistry};

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

        let mut tools = ToolRegistry::new(adapter.source_info.clone());
        adapter.register_tools(&mut tools);
        assert_eq!(tools.into_vec().len(), 1);
    }
}
