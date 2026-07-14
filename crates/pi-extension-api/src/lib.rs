//! `ExtensionAPI` trait — Rust 原生扩展接口。
//!
//! 与原版 TypeScript `ExtensionAPI` 接口保持语义一致。
//! 每个扩展实现此 trait，通过 `ExtensionRegistry` 注册到 agent 运行时。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use serde::{Deserialize, Serialize};

// ============================================================================
// Tool Definition
// ============================================================================

/// Tool definition matching the original TypeScript ToolDefinition interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (used in LLM tool calls).
    pub name: String,
    /// Human-readable label for UI display.
    #[serde(default)]
    pub label: Option<String>,
    /// Description for the LLM.
    #[serde(default)]
    pub description: String,
    /// Optional one-line prompt snippet.
    #[serde(default)]
    pub prompt_snippet: Option<String>,
    /// Optional prompt guidelines for the LLM.
    #[serde(default)]
    pub prompt_guidelines: Option<Vec<String>>,
    /// JSON Schema for tool parameters.
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    /// Shell rendering mode.
    #[serde(default)]
    pub render_shell: Option<String>,
    /// Execution mode: "sequential" or "parallel".
    #[serde(default)]
    pub execution_mode: Option<String>,
}

// ============================================================================
// 事件类型 — 对应原版 ExtensionEvent 联合类型
// ============================================================================

/// 扩展可订阅的所有事件。
#[derive(Debug, Clone)]
pub enum ExtensionEvent {
    ProjectTrust { cwd: String },
    ResourcesDiscover { cwd: String, reason: String },
    SessionStart { reason: String, previous_session_file: Option<String> },
    SessionInfoChanged { name: Option<String> },
    SessionBeforeSwitch { reason: String, target_session_file: Option<String> },
    SessionBeforeFork { entry_id: String, position: String },
    SessionBeforeCompact { reason: String, will_retry: bool },
    SessionCompact { summary: String, tokens_before: u64 },
    SessionShutdown { reason: String, target_session_file: Option<String> },
    SessionBeforeTree { target_id: String },
    SessionTree { new_leaf_id: Option<String>, old_leaf_id: Option<String> },
    Context { messages: Vec<Value> },
    BeforeProviderRequest { payload: Value },
    BeforeProviderHeaders { headers: HashMap<String, String> },
    AfterProviderResponse { status: u16, headers: HashMap<String, String> },
    BeforeAgentStart { prompt: String, system_prompt: String },
    AgentStart,
    AgentEnd { messages: Vec<Value> },
    AgentSettled,
    TurnStart,
    TurnEnd { message: Value, tool_results: Vec<Value> },
    MessageStart { message: Value },
    MessageUpdate { message: Value },
    MessageEnd { message: Value },
    ToolExecutionStart { tool_call_id: String, tool_name: String, args: Value },
    ToolExecutionUpdate { tool_call_id: String, tool_name: String, args: Value, partial_result: Value },
    ToolExecutionEnd { tool_call_id: String, tool_name: String, result: Value, is_error: bool },
    ModelSelect { model: String, previous_model: Option<String> },
    ThinkingLevelSelect { level: String, previous_level: String },
    ToolCall { tool_call_id: String, tool_name: String, input: Value },
    ToolResult { tool_call_id: String, tool_name: String, input: Value, content: Vec<Value>, is_error: bool },
    UserBash { command: String, cwd: String },
    Input { text: String, source: String },
}

/// 事件处理结果。
#[derive(Debug, Clone, Default)]
pub struct EventResult {
    pub block: Option<bool>,
    pub reason: Option<String>,
    pub messages: Option<Vec<Value>>,
    pub system_prompt: Option<String>,
    pub action: Option<String>,
    pub text: Option<String>,
    pub trusted: Option<String>,
    pub remember: bool,
    pub skill_paths: Option<Vec<String>>,
    pub prompt_paths: Option<Vec<String>>,
    pub theme_paths: Option<Vec<String>>,
    pub cancel: Option<bool>,
}

// ============================================================================
// 扩展上下文 — 对应原版 ExtensionContext 接口
// ============================================================================

/// 扩展事件处理时收到的上下文。
/// 对应原版 `ExtensionContext` 接口。
#[derive(Clone)]
pub struct ExtensionContext {
    pub cwd: String,
    pub has_ui: bool,
    pub ui: ExtensionUIContext,
    /// 运行时操作句柄。
    pub runtime: RuntimeHandle,
}

/// 运行时操作句柄 — 扩展通过此句柄与运行时交互。
/// 对应原版 `pi.sendMessage()` / `pi.appendEntry()` / `pi.getActiveTools()` 等。
#[derive(Clone)]
pub struct RuntimeHandle {
    pub send_message: Arc<dyn Fn(Value, Option<SendMessageOptions>) + Send + Sync>,
    pub send_user_message: Arc<dyn Fn(String, Option<SendUserMessageOptions>) + Send + Sync>,
    pub append_entry: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
    pub get_active_tools: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub set_active_tools: Arc<dyn Fn(Vec<String>) + Send + Sync>,
}

impl RuntimeHandle {
    pub fn noop() -> Self {
        Self {
            send_message: Arc::new(|_, _| {}),
            send_user_message: Arc::new(|_, _| {}),
            append_entry: Arc::new(|_, _| {}),
            get_active_tools: Arc::new(Vec::new),
            set_active_tools: Arc::new(|_| {}),
        }
    }
}

/// UI 操作方法。
/// 对应原版 `ExtensionUIContext` 接口的子集。
#[derive(Clone)]
pub struct ExtensionUIContext {
    pub notify: Arc<dyn Fn(String, &str) + Send + Sync>,
    pub set_status: Arc<dyn Fn(String, Option<String>) + Send + Sync>,
    pub confirm: Arc<dyn Fn(String, String) -> bool + Send + Sync>,
}

// ============================================================================
// 注册类型 — 对应原版 RegisteredTool / RegisteredCommand 等
// ============================================================================

#[derive(Debug, Clone)]
pub struct RegisteredTool {
    pub definition: ToolDefinition,
}

#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegisteredShortcut {
    pub key: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RegisteredFlag {
    pub name: String,
    pub description: Option<String>,
    pub flag_type: String,
    pub default: Option<Value>,
}

// ============================================================================
// 消息选项 — 对应原版 sendMessage / sendUserMessage 选项
// ============================================================================

#[derive(Debug, Clone)]
pub struct SendMessageOptions {
    pub trigger_turn: Option<bool>,
    pub deliver_as: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SendUserMessageOptions {
    pub deliver_as: Option<String>,
}

// ============================================================================
// 执行结果 — 对应原版 ExecResult
// ============================================================================

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

// ============================================================================
// ExtensionAPI trait — 对应原版 TypeScript ExtensionAPI 接口
// ============================================================================

/// 扩展实现此 trait 来注册工具、命令、事件处理器。
///
/// 对应原版 TypeScript 的 `ExtensionAPI` 接口。
/// 所有方法都有默认实现，扩展只需覆盖需要的部分。
#[async_trait]
pub trait ExtensionAPI: Send + Sync {
    // ── 元数据 ──────────────────────────────────────────────────────────

    /// 扩展名称（唯一标识）。
    fn name(&self) -> &'static str;

    // ── 注册方法（加载时调用） ─────────────────────────────────────────

    /// 注册工具。对应原版 `registerTool()`。
    fn register_tools(&self, _registry: &mut ToolRegistry) {}

    /// 注册命令。对应原版 `registerCommand()`。
    fn register_commands(&self, _registry: &mut CommandRegistry) {}

    /// 注册快捷键。对应原版 `registerShortcut()`。
    fn register_shortcuts(&self, _registry: &mut ShortcutRegistry) {}

    /// 注册 CLI flag。对应原版 `registerFlag()`。
    fn register_flags(&self, _registry: &mut FlagRegistry) {}

    // ── 事件处理器 ──────────────────────────────────────────────────────

    /// 事件处理器。对应原版 `on(event, handler)`。
    /// 返回 `Some(result)` 表示处理了该事件，`None` 表示不处理。
    async fn on_event(&self, event: &ExtensionEvent, ctx: &ExtensionContext) -> Option<EventResult> {
        let _ = (event, ctx);
        None
    }

    // ── 工具执行 ────────────────────────────────────────────────────────

    /// 处理工具调用。对应原版 `registerTool()` 中的 `execute`。
    /// 返回 `Some(result)` 表示此扩展处理了该工具，`None` 表示不处理。
    async fn handle_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        let _ = (tool_name, params, ctx);
        None
    }
}

/// 工具调用结果。
#[derive(Debug, Clone)]
pub struct ToolCallOutput {
    pub content: Vec<Value>,
    pub details: Option<Value>,
    pub is_error: bool,
}

// ============================================================================
// 注册表
// ============================================================================

pub struct ToolRegistry {
    pub(crate) tools: Vec<RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self { tools: Vec::new() } }
    pub fn register(&mut self, tool: ToolDefinition) {
        self.tools.push(RegisteredTool { definition: tool });
    }
    pub fn into_vec(self) -> Vec<RegisteredTool> { self.tools }
}

pub struct CommandRegistry {
    pub(crate) commands: Vec<RegisteredCommand>,
}

impl CommandRegistry {
    pub fn new() -> Self { Self { commands: Vec::new() } }
    pub fn register(&mut self, name: &str, description: Option<&str>) {
        self.commands.push(RegisteredCommand {
            name: name.to_string(),
            description: description.map(String::from),
        });
    }
    pub fn into_vec(self) -> Vec<RegisteredCommand> { self.commands }
}

pub struct ShortcutRegistry {
    pub(crate) shortcuts: Vec<RegisteredShortcut>,
}

impl ShortcutRegistry {
    pub fn new() -> Self { Self { shortcuts: Vec::new() } }
    pub fn register(&mut self, key: &str, description: Option<&str>) {
        self.shortcuts.push(RegisteredShortcut {
            key: key.to_string(),
            description: description.map(String::from),
        });
    }
    pub fn into_vec(self) -> Vec<RegisteredShortcut> { self.shortcuts }
}

pub struct FlagRegistry {
    pub(crate) flags: Vec<RegisteredFlag>,
}

impl FlagRegistry {
    pub fn new() -> Self { Self { flags: Vec::new() } }
    pub fn register(&mut self, name: &str, description: Option<&str>, flag_type: &str) {
        self.flags.push(RegisteredFlag {
            name: name.to_string(),
            description: description.map(String::from),
            flag_type: flag_type.to_string(),
            default: None,
        });
    }
    pub fn into_vec(self) -> Vec<RegisteredFlag> { self.flags }
}

// ============================================================================
// ExtensionRegistry — 管理所有已注册的扩展
// ============================================================================

pub struct ExtensionRegistry {
    extensions: Vec<Box<dyn ExtensionAPI>>,
}

impl ExtensionRegistry {
    pub fn new() -> Self { Self { extensions: Vec::new() } }

    pub fn register(&mut self, ext: Box<dyn ExtensionAPI>) {
        self.extensions.push(ext);
    }

    pub fn extensions(&self) -> &[Box<dyn ExtensionAPI>] {
        &self.extensions
    }

    pub fn collect_tools(&mut self) -> Vec<RegisteredTool> {
        let mut all = Vec::new();
        for ext in &mut self.extensions {
            let mut reg = ToolRegistry::new();
            ext.register_tools(&mut reg);
            all.extend(reg.into_vec());
        }
        all
    }

    pub fn collect_commands(&mut self) -> Vec<RegisteredCommand> {
        let mut all = Vec::new();
        for ext in &mut self.extensions {
            let mut reg = CommandRegistry::new();
            ext.register_commands(&mut reg);
            all.extend(reg.into_vec());
        }
        all
    }

    pub fn collect_shortcuts(&mut self) -> Vec<RegisteredShortcut> {
        let mut all = Vec::new();
        for ext in &mut self.extensions {
            let mut reg = ShortcutRegistry::new();
            ext.register_shortcuts(&mut reg);
            all.extend(reg.into_vec());
        }
        all
    }

    pub async fn dispatch_event(&self, event: &ExtensionEvent, ctx: &ExtensionContext) -> Vec<(String, Option<EventResult>)> {
        let mut results = Vec::new();
        for ext in &self.extensions {
            let result = ext.on_event(event, ctx).await;
            results.push((ext.name().to_string(), result));
        }
        results
    }
}
