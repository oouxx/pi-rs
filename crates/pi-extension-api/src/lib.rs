//! `HookHandler` trait — Rust 原生扩展接口。
//!
//! 基于 ZeroClaw 的 Hook 系统设计。所有扩展实现 `HookHandler` trait，
//! 通过 `ExtensionRegistry` 注册到 agent 运行时。

pub mod hook;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use serde::{Deserialize, Serialize};

pub use hook::{
    CommandRegistry, FlagRegistry, HookHandler, HookResult, HookRunner, RegisteredCommand,
    RegisteredFlag, RegisteredShortcut, RegisteredTool, ShortcutRegistry, ToolRegistry,
};

// ============================================================================
// Tool Definition
// ============================================================================

/// Execute callback for a custom tool.
///
/// Takes (tool_call_id, params, signal) and returns a `ToolCallOutput`.
pub type ToolExecuteFn = Arc<
    dyn Fn(
            String,
            serde_json::Value,
            Option<tokio::sync::watch::Receiver<bool>>,
        )
            -> Pin<Box<dyn Future<Output = Result<ToolCallOutput, Box<dyn std::error::Error + Send + Sync>>> + Send>>
            + Send
            + Sync,
>;

/// Tool definition matching the original TypeScript ToolDefinition interface.
#[derive(Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt_snippet: Option<String>,
    #[serde(default)]
    pub prompt_guidelines: Option<Vec<String>>,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    #[serde(default)]
    pub render_shell: Option<String>,
    #[serde(default)]
    pub execution_mode: Option<String>,
    #[serde(skip)]
    pub execute: Option<ToolExecuteFn>,
}

impl Default for ToolDefinition {
    fn default() -> Self {
        Self {
            name: String::new(),
            label: None,
            description: String::new(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: None,
            execute: None,
        }
    }
}

impl std::fmt::Debug for ToolDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDefinition")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .field("prompt_snippet", &self.prompt_snippet)
            .field("prompt_guidelines", &self.prompt_guidelines)
            .field("parameters", &self.parameters)
            .field("render_shell", &self.render_shell)
            .field("execution_mode", &self.execution_mode)
            .field("execute", &self.execute.as_ref().map(|_| "Some(fn)"))
            .finish()
    }
}

// ============================================================================
// ToolCallOutput
// ============================================================================

/// Output from a tool call handled by an extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallOutput {
    pub content: Vec<Value>,
    pub details: Option<Value>,
    pub is_error: bool,
    pub terminate: Option<bool>,
}

// ============================================================================
// ToolInfo
// ============================================================================

/// Information about a tool for extension use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: Option<Value>,
}

// ============================================================================
// ExtensionContext
// ============================================================================

/// Context provided to extensions for interacting with the agent.
pub struct ExtensionContext {
    pub session_id: String,
    pub is_connected: bool,
    pub ui: ExtensionUIContext,
    pub runtime: RuntimeHandle,
}

impl ExtensionContext {
    pub fn new(
        session_id: String,
        is_connected: bool,
        ui: ExtensionUIContext,
        runtime: RuntimeHandle,
    ) -> Self {
        Self {
            session_id,
            is_connected,
            ui,
            runtime,
        }
    }
}

// ============================================================================
// ExtensionUIContext
// ============================================================================

/// UI context for extensions to interact with the user interface.
#[derive(Clone)]
pub struct ExtensionUIContext {
    pub notify: Arc<dyn Fn(&str, &Value) + Send + Sync>,
    pub set_status: Arc<dyn Fn(&str, &str) + Send + Sync>,
    pub confirm: Arc<dyn Fn(&str, &Value) -> bool + Send + Sync>,
}

// ============================================================================
// RuntimeHandle
// ============================================================================

/// Handle for extensions to interact with the agent runtime.
#[derive(Clone)]
pub struct RuntimeHandle {
    pub send_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
    pub send_user_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
    pub set_custom_prompt: Arc<dyn Fn(Option<String>) + Send + Sync>,
    pub set_model: Arc<dyn Fn(String) + Send + Sync>,
    pub set_thinking_level: Arc<dyn Fn(String) + Send + Sync>,
    pub set_selected_tools: Arc<dyn Fn(Option<Vec<String>>) + Send + Sync>,
    pub set_allowed_tools: Arc<dyn Fn(Option<Vec<String>>) + Send + Sync>,
    pub set_excluded_tools: Arc<dyn Fn(Option<Vec<String>>) + Send + Sync>,
    pub set_append_system_prompt: Arc<dyn Fn(Option<String>) + Send + Sync>,
    pub set_session_name: Arc<dyn Fn(Option<String>) + Send + Sync>,
    pub set_session_cwd: Arc<dyn Fn(String) + Send + Sync>,
    pub abort: Arc<dyn Fn() + Send + Sync>,
    pub new_session: Arc<dyn Fn(Option<String>) + Send + Sync>,
    pub switch_session: Arc<dyn Fn(String) + Send + Sync>,
    pub fork_session: Arc<dyn Fn(String, String) + Send + Sync>,
    pub compact_session: Arc<dyn Fn(Option<String>) + Send + Sync>,
    pub export_html: Arc<dyn Fn() + Send + Sync>,
    pub get_session_stats: Arc<dyn Fn() -> Value + Send + Sync>,
    pub get_config: Arc<dyn Fn() -> Value + Send + Sync>,
    pub set_config: Arc<dyn Fn(Value) + Send + Sync>,
    pub get_settings: Arc<dyn Fn() -> Value + Send + Sync>,
    pub set_settings: Arc<dyn Fn(Value) + Send + Sync>,
    pub get_theme: Arc<dyn Fn() -> Value + Send + Sync>,
    pub get_env: Arc<dyn Fn(String) -> Option<String> + Send + Sync>,
    pub set_env: Arc<dyn Fn(String, String) + Send + Sync>,
    pub get_cwd: Arc<dyn Fn() -> String + Send + Sync>,
    pub set_cwd: Arc<dyn Fn(String) + Send + Sync>,
    pub get_agent_dir: Arc<dyn Fn() -> String + Send + Sync>,
    pub get_session_dir: Arc<dyn Fn() -> String + Send + Sync>,
    pub get_session_file: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub get_session_id: Arc<dyn Fn() -> String + Send + Sync>,
    pub get_session_name: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub get_model: Arc<dyn Fn() -> String + Send + Sync>,
    pub get_thinking_level: Arc<dyn Fn() -> String + Send + Sync>,
    pub get_selected_tools: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub get_allowed_tools: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub get_excluded_tools: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub get_custom_prompt: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub get_append_system_prompt: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    pub get_messages: Arc<dyn Fn() -> Value + Send + Sync>,
    pub get_tool_definitions: Arc<dyn Fn() -> Value + Send + Sync>,
    pub get_context_usage: Arc<dyn Fn() -> Value + Send + Sync>,
    pub get_config_value: Arc<dyn Fn(String) -> Option<Value> + Send + Sync>,
    pub set_config_value: Arc<dyn Fn(String, Value) + Send + Sync>,
    pub get_setting: Arc<dyn Fn(String) -> Option<Value> + Send + Sync>,
    pub set_setting: Arc<dyn Fn(String, Value) + Send + Sync>,
    pub read_file: Arc<dyn Fn(String) -> Option<String> + Send + Sync>,
    pub write_file: Arc<dyn Fn(String, String) + Send + Sync>,
    pub list_dir: Arc<dyn Fn(String) -> Vec<String> + Send + Sync>,
    pub file_exists: Arc<dyn Fn(String) -> bool + Send + Sync>,
    pub create_dir: Arc<dyn Fn(String) + Send + Sync>,
    pub remove_file: Arc<dyn Fn(String) + Send + Sync>,
    pub run_command: Arc<dyn Fn(String, String) -> Value + Send + Sync>,
    pub open_url: Arc<dyn Fn(String) + Send + Sync>,
    pub log: Arc<dyn Fn(String, String) + Send + Sync>,
}

impl RuntimeHandle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        send_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
        send_user_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
        set_custom_prompt: Arc<dyn Fn(Option<String>) + Send + Sync>,
        set_model: Arc<dyn Fn(String) + Send + Sync>,
        set_thinking_level: Arc<dyn Fn(String) + Send + Sync>,
        set_selected_tools: Arc<dyn Fn(Option<Vec<String>>) + Send + Sync>,
        set_allowed_tools: Arc<dyn Fn(Option<Vec<String>>) + Send + Sync>,
        set_excluded_tools: Arc<dyn Fn(Option<Vec<String>>) + Send + Sync>,
        set_append_system_prompt: Arc<dyn Fn(Option<String>) + Send + Sync>,
        set_session_name: Arc<dyn Fn(Option<String>) + Send + Sync>,
        set_session_cwd: Arc<dyn Fn(String) + Send + Sync>,
        abort: Arc<dyn Fn() + Send + Sync>,
        new_session: Arc<dyn Fn(Option<String>) + Send + Sync>,
        switch_session: Arc<dyn Fn(String) + Send + Sync>,
        fork_session: Arc<dyn Fn(String, String) + Send + Sync>,
        compact_session: Arc<dyn Fn(Option<String>) + Send + Sync>,
        export_html: Arc<dyn Fn() + Send + Sync>,
        get_session_stats: Arc<dyn Fn() -> Value + Send + Sync>,
        get_config: Arc<dyn Fn() -> Value + Send + Sync>,
        set_config: Arc<dyn Fn(Value) + Send + Sync>,
        get_settings: Arc<dyn Fn() -> Value + Send + Sync>,
        set_settings: Arc<dyn Fn(Value) + Send + Sync>,
        get_theme: Arc<dyn Fn() -> Value + Send + Sync>,
        get_env: Arc<dyn Fn(String) -> Option<String> + Send + Sync>,
        set_env: Arc<dyn Fn(String, String) + Send + Sync>,
        get_cwd: Arc<dyn Fn() -> String + Send + Sync>,
        set_cwd: Arc<dyn Fn(String) + Send + Sync>,
        get_agent_dir: Arc<dyn Fn() -> String + Send + Sync>,
        get_session_dir: Arc<dyn Fn() -> String + Send + Sync>,
        get_session_file: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        get_session_id: Arc<dyn Fn() -> String + Send + Sync>,
        get_session_name: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        get_model: Arc<dyn Fn() -> String + Send + Sync>,
        get_thinking_level: Arc<dyn Fn() -> String + Send + Sync>,
        get_selected_tools: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        get_allowed_tools: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        get_excluded_tools: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        get_custom_prompt: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        get_append_system_prompt: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        get_messages: Arc<dyn Fn() -> Value + Send + Sync>,
        get_tool_definitions: Arc<dyn Fn() -> Value + Send + Sync>,
        get_context_usage: Arc<dyn Fn() -> Value + Send + Sync>,
        get_config_value: Arc<dyn Fn(String) -> Option<Value> + Send + Sync>,
        set_config_value: Arc<dyn Fn(String, Value) + Send + Sync>,
        get_setting: Arc<dyn Fn(String) -> Option<Value> + Send + Sync>,
        set_setting: Arc<dyn Fn(String, Value) + Send + Sync>,
        read_file: Arc<dyn Fn(String) -> Option<String> + Send + Sync>,
        write_file: Arc<dyn Fn(String, String) + Send + Sync>,
        list_dir: Arc<dyn Fn(String) -> Vec<String> + Send + Sync>,
        file_exists: Arc<dyn Fn(String) -> bool + Send + Sync>,
        create_dir: Arc<dyn Fn(String) + Send + Sync>,
        remove_file: Arc<dyn Fn(String) + Send + Sync>,
        run_command: Arc<dyn Fn(String, String) -> Value + Send + Sync>,
        open_url: Arc<dyn Fn(String) + Send + Sync>,
        log: Arc<dyn Fn(String, String) + Send + Sync>,
    ) -> Self {
        Self {
            send_message,
            send_user_message,
            set_custom_prompt,
            set_model,
            set_thinking_level,
            set_selected_tools,
            set_allowed_tools,
            set_excluded_tools,
            set_append_system_prompt,
            set_session_name,
            set_session_cwd,
            abort,
            new_session,
            switch_session,
            fork_session,
            compact_session,
            export_html,
            get_session_stats,
            get_config,
            set_config,
            get_settings,
            set_settings,
            get_theme,
            get_env,
            set_env,
            get_cwd,
            set_cwd,
            get_agent_dir,
            get_session_dir,
            get_session_file,
            get_session_id,
            get_session_name,
            get_model,
            get_thinking_level,
            get_selected_tools,
            get_allowed_tools,
            get_excluded_tools,
            get_custom_prompt,
            get_append_system_prompt,
            get_messages,
            get_tool_definitions,
            get_context_usage,
            get_config_value,
            set_config_value,
            get_setting,
            set_setting,
            read_file,
            write_file,
            list_dir,
            file_exists,
            create_dir,
            remove_file,
            run_command,
            open_url,
            log,
        }
    }

    /// Create a no-op RuntimeHandle for testing.
    pub fn noop() -> Self {
        Self::new(
            Arc::new(|_, _| {}),
            Arc::new(|_, _| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|| {}),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_, _| {}),
            Arc::new(|_| {}),
            Arc::new(|| {}),
            Arc::new(|| Value::Null),
            Arc::new(|| Value::Null),
            Arc::new(|_| {}),
            Arc::new(|| Value::Null),
            Arc::new(|_| {}),
            Arc::new(|| Value::Null),
            Arc::new(|_| None),
            Arc::new(|_, _| {}),
            Arc::new(|| String::new()),
            Arc::new(|_| {}),
            Arc::new(|| String::new()),
            Arc::new(|| String::new()),
            Arc::new(|| None),
            Arc::new(|| String::new()),
            Arc::new(|| None),
            Arc::new(|| String::new()),
            Arc::new(|| String::new()),
            Arc::new(|| vec![]),
            Arc::new(|| vec![]),
            Arc::new(|| vec![]),
            Arc::new(|| None),
            Arc::new(|| None),
            Arc::new(|| Value::Null),
            Arc::new(|| Value::Null),
            Arc::new(|| Value::Null),
            Arc::new(|_| None),
            Arc::new(|_, _| {}),
            Arc::new(|_| None),
            Arc::new(|_, _| {}),
            Arc::new(|_| None),
            Arc::new(|_, _| {}),
            Arc::new(|_| vec![]),
            Arc::new(|_| false),
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Arc::new(|_, _| Value::Null),
            Arc::new(|_| {}),
            Arc::new(|_, _| {}),
        )
    }
}

// ============================================================================
// EventPublisher
// ============================================================================

/// Publisher for session-level events, used by extensions to emit events
/// to the UI layer.
#[derive(Clone)]
pub struct EventPublisher {
    sender: tokio::sync::mpsc::UnboundedSender<Value>,
}

impl EventPublisher {
    pub fn new(sender: tokio::sync::mpsc::UnboundedSender<Value>) -> Self {
        Self { sender }
    }

    pub fn publish(&self, event: Value) {
        let _ = self.sender.send(event);
    }
}

// ============================================================================
// SendMessageOptions / SendUserMessageOptions
// ============================================================================

/// Options for sending a custom message from an extension.
#[derive(Debug, Clone)]
pub struct SendMessageOptions {
    pub trigger_turn: Option<bool>,
    pub deliver_as: Option<String>,
}

/// Options for sending a user message from an extension.
#[derive(Debug, Clone)]
pub struct SendUserMessageOptions {
    pub deliver_as: Option<String>,
}

// ============================================================================
// ExtensionRegistry
// ============================================================================

/// Registry for extensions. Wraps a `HookRunner` and provides
/// tool/command/shortcut collection.
///
/// This is the single entry point for all extension-related operations.
pub struct ExtensionRegistry {
    hook_runner: HookRunner,
    tools: Vec<RegisteredTool>,
    commands: Vec<RegisteredCommand>,
    shortcuts: Vec<RegisteredShortcut>,
    flags: Vec<RegisteredFlag>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            hook_runner: HookRunner::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            shortcuts: Vec::new(),
            flags: Vec::new(),
        }
    }

    /// Register a HookHandler. This collects tools/commands/shortcuts/flags
    /// from the handler immediately.
    pub fn register(&mut self, handler: Box<dyn HookHandler>) {
        // Collect tools
        let mut tool_reg = ToolRegistry::new();
        handler.register_tools(&mut tool_reg);
        self.tools.extend(tool_reg.into_vec());

        // Collect commands
        let mut cmd_reg = CommandRegistry::new();
        handler.register_commands(&mut cmd_reg);
        self.commands.extend(cmd_reg.into_vec());

        // Collect shortcuts
        let mut shortcut_reg = ShortcutRegistry::new();
        handler.register_shortcuts(&mut shortcut_reg);
        self.shortcuts.extend(shortcut_reg.into_vec());

        // Collect flags
        let mut flag_reg = FlagRegistry::new();
        handler.register_flags(&mut flag_reg);
        self.flags.extend(flag_reg.into_vec());

        // Register with hook runner for event dispatch
        self.hook_runner.register(handler);
    }

    /// Access the HookRunner for event dispatch.
    pub fn hook_runner(&self) -> &HookRunner {
        &self.hook_runner
    }

    /// Check if any handlers are registered.
    pub fn has_handlers(&self) -> bool {
        self.hook_runner.has_handlers()
    }

    /// Number of registered handlers.
    pub fn handler_count(&self) -> usize {
        self.hook_runner.handler_count()
    }

    /// Get collected tools.
    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
    }

    /// Get collected commands.
    pub fn commands(&self) -> &[RegisteredCommand] {
        &self.commands
    }

    /// Get collected shortcuts.
    pub fn shortcuts(&self) -> &[RegisteredShortcut] {
        &self.shortcuts
    }

    /// Get collected flags.
    pub fn flags(&self) -> &[RegisteredFlag] {
        &self.flags
    }

    /// Consume and return collected tools.
    pub fn collect_tools(&mut self) -> Vec<RegisteredTool> {
        std::mem::take(&mut self.tools)
    }

    /// Consume and return collected commands.
    pub fn collect_commands(&mut self) -> Vec<RegisteredCommand> {
        std::mem::take(&mut self.commands)
    }

    /// Consume and return collected shortcuts.
    pub fn collect_shortcuts(&mut self) -> Vec<RegisteredShortcut> {
        std::mem::take(&mut self.shortcuts)
    }

    /// Consume and return collected flags.
    pub fn collect_flags(&mut self) -> Vec<RegisteredFlag> {
        std::mem::take(&mut self.flags)
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use super::*;

    #[test]
    fn test_tool_definition_default() {
        let def = ToolDefinition::default();
        assert_eq!(def.name, "");
        assert!(def.execute.is_none());
    }

    #[test]
    fn test_extension_registry_empty() {
        let reg = ExtensionRegistry::new();
        assert!(!reg.has_handlers());
        assert_eq!(reg.handler_count(), 0);
        assert!(reg.tools().is_empty());
        assert!(reg.commands().is_empty());
        assert!(reg.shortcuts().is_empty());
    }

    #[test]
    fn test_extension_registry_register_handler() {
        use hook::HookHandler;

        struct TestHandler;

        #[async_trait]
        impl HookHandler for TestHandler {
            fn name(&self) -> &str {
                "test"
            }
        }

        let mut reg = ExtensionRegistry::new();
        reg.register(Box::new(TestHandler));

        assert!(reg.has_handlers());
        assert_eq!(reg.handler_count(), 1);
    }

    #[test]
    fn test_extension_registry_collect_tools() {
        use hook::{HookHandler, ToolRegistry};

        struct ToolHandler;

        #[async_trait]
        impl HookHandler for ToolHandler {
            fn name(&self) -> &str {
                "tool_handler"
            }

            fn register_tools(&self, tools: &mut ToolRegistry) {
                tools.register("test_tool", ToolDefinition {
                    name: "test_tool".into(),
                    description: "A test tool".into(),
                    ..Default::default()
                });
            }
        }

        let mut reg = ExtensionRegistry::new();
        reg.register(Box::new(ToolHandler));

        let tools = reg.collect_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
    }
}
