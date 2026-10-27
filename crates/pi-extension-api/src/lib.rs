//! `HookHandler` trait — Rust 原生扩展接口。
//!
//! 基于 `ZeroClaw` 的 Hook 系统设计。所有扩展实现 `HookHandler` trait，
//! 通过 `ExtensionRegistry` 注册到 agent 运行时。

pub mod hook;
pub mod source_info;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use serde::{Deserialize, Serialize};

pub use hook::{
    ArgumentCompletionsFn, AutocompleteItem, CommandRegistry, CommandRegistration,
    FlagRegistry, HookHandler, HookResult, HookRunner, RegisteredCommand, RegisteredFlag,
    RegisteredShortcut, RegisteredTool, SessionBeforeCompactEvent, SessionBeforeCompactResult,
    ShortcutRegistry, ToolRegistry,
};
pub use source_info::{
    create_builtin_source_info, create_source_info, create_synthetic_source_info,
    SourceInfo, SourceOrigin, SourceScope,
};

/// Tool 运行中的部分结果回调（对齐 TS `onUpdate(partial)`）。
///
/// 扩展可以在子任务/子进程还在跑的时候推一条中间结果，宿主会把它当作
/// `tool_execution_update` 事件转发给 UI（TUI 会刷新该工具块的输出）。
pub type ToolUpdateFn = Arc<dyn Fn(ToolCallOutput) + Send + Sync>;

/// 什么都不做的 `ToolUpdateFn`（headless 模式、单测、未接 UI 的调用）。
#[must_use]
pub fn noop_tool_update() -> ToolUpdateFn {
    Arc::new(|_| {})
}

/// 等中止信号：已中止 → 立即返回；未中止 → 等下一次变更；没有信号或信号
/// 通道已断开（本轮 run 结束）→ 永不返回。
///
/// 通道断开不能算中止，否则会话收尾时会把还在跑的工具误判成 Ctrl+C。
pub async fn wait_aborted(rx: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    let Some(rx) = rx.as_mut() else {
        std::future::pending::<()>().await;
        return;
    };
    if *rx.borrow() {
        return;
    }
    loop {
        match rx.changed().await {
            Ok(()) => {
                if *rx.borrow() {
                    return;
                }
            }
            Err(_) => {
                std::future::pending::<()>().await;
                return;
            }
        }
    }
}

/// 一次工具调用的运行上下文（对齐 TS
/// `execute(toolCallId, params, signal, onUpdate, ctx)`）。
///
/// `handle_tool_call` 的 `tool_name` / `params` / `ctx` 仍作为独立参数传入
/// （保持既有签名习惯），额外能力（中止信号 + 中间结果上报）走这里。
#[derive(Clone)]
pub struct ToolInvocation {
    /// Agent 侧的工具调用 id（对齐 TS `toolCallId`）。
    pub tool_call_id: String,
    /// 中止信号（`watch` 通道，`true` = 已中止）。对齐 TS `signal: AbortSignal`。
    pub abort: Option<tokio::sync::watch::Receiver<bool>>,
    /// 运行中推送部分结果（对齐 TS `onUpdate`）。
    pub on_update: ToolUpdateFn,
}

impl ToolInvocation {
    #[must_use]
    pub fn new(
        tool_call_id: impl Into<String>,
        abort: Option<tokio::sync::watch::Receiver<bool>>,
        on_update: ToolUpdateFn,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            abort,
            on_update,
        }
    }

    /// 无 id / 无信号 / 不上报中间结果的调用（测试与 headless 场景）。
    #[must_use]
    pub fn noop() -> Self {
        Self {
            tool_call_id: String::new(),
            abort: None,
            on_update: noop_tool_update(),
        }
    }

    /// 是否已中止（对齐 TS `signal.aborted`）。
    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.abort.as_ref().is_some_and(|rx| *rx.borrow())
    }

    /// 等中止信号（没有信号时永不返回）。供 `tokio::select!` 使用。
    pub async fn aborted(&mut self) {
        wait_aborted(&mut self.abort).await;
    }

    /// 推送一条中间结果（未接 UI 时是空操作）。
    pub fn update(&self, output: &ToolCallOutput) {
        (self.on_update)(output.clone());
    }
}

impl std::fmt::Debug for ToolInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolInvocation")
            .field("tool_call_id", &self.tool_call_id)
            .field("abort", &self.abort.is_some())
            .field("on_update", &"<fn>")
            .finish()
    }
}

/// 扩展工具的自定义渲染输入（对齐 TS `renderCall(args, theme, ctx)` +
/// `renderResult(result, { expanded }, theme, ctx)`）。
#[derive(Clone, Debug, Default)]
pub struct ToolRenderContext {
    /// 工具名。
    pub tool_name: String,
    /// 当前会话 cwd（对齐 TS `ctx.cwd`）。
    pub cwd: String,
    /// 是否展开视图（TS `{ expanded }`；TUI 的 Ctrl+O）。
    pub expanded: bool,
    /// 工具参数（TS `renderCall` 的 `args`）。
    pub args: serde_json::Value,
    /// 结果文本（工具返回的 content 拼成的纯文本；未完成时为空）。
    pub output: String,
    /// 结构化 `details`（未完成时为 `None`）。
    pub details: Option<serde_json::Value>,
    /// 结果是不是错误结果（未完成时为 `false`）。
    pub is_error: bool,
}

/// 文本的语义样式。扩展只说"这段是什么语义"，具体颜色由 TUI 按当前主题解析
/// （对齐 TS 的 `theme.fg("<token>", text)`，但不把颜色值泄漏给扩展）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolTextStyle {
    /// 普通输出文本（TS `toolOutput`）。
    #[default]
    Default,
    /// 次要说明（TS `muted`）。
    Muted,
    /// 最弱提示（TS `dim`）。
    Dim,
    /// 强调（TS `accent`）。
    Accent,
    /// 警告（TS `warning`）。
    Warning,
    /// 错误（TS `error`）。
    Error,
    /// 成功（TS `success`）。
    Success,
    /// 粗体标题（TS `toolTitle` + `bold`）。
    Bold,
}

/// 一段带语义样式的文本（行内可混排不同样式，对齐 TS 的 `Text` 多 span）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolRenderSpan {
    pub style: ToolTextStyle,
    pub text: String,
}

impl ToolRenderSpan {
    #[must_use]
    pub fn new(style: ToolTextStyle, text: impl Into<String>) -> Self {
        Self {
            style,
            text: text.into(),
        }
    }

    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(ToolTextStyle::Default, text)
    }
}

/// 渲染块：按顺序排布（对齐 TS `Container.addChild` 的组件序列）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolRenderBlock {
    /// 一行文本（可分段着色）。
    Line(Vec<ToolRenderSpan>),
    /// 空行（TS `Spacer(1)`）。
    Spacer,
    /// Markdown 块（TS `Markdown` 组件；终稿用它）。
    Markdown(String),
}

impl ToolRenderBlock {
    /// 单段文本的一行。
    #[must_use]
    pub fn line(text: impl Into<String>) -> Self {
        Self::Line(vec![ToolRenderSpan::plain(text)])
    }

    /// 指定语义样式的单段文本行。
    #[must_use]
    pub fn styled(style: ToolTextStyle, text: impl Into<String>) -> Self {
        Self::Line(vec![ToolRenderSpan::new(style, text)])
    }

    /// 多段文本的一行。
    #[must_use]
    pub fn spans(spans: Vec<ToolRenderSpan>) -> Self {
        Self::Line(spans)
    }
}

/// 扩展工具的自定义渲染输出。
///
/// 模型刻意保持"标题行 + 有序块列表"（而不是 TS 的任意组件树）：换行、
/// 宽度、截断、Markdown 渲染都由 pi-tui 统一负责，扩展只决定文字内容与语义
/// 样式，既能表达原版布局（分节标题、空行、每步一组），又不会把 TUI 布局
/// 能力泄漏进扩展。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolRenderOutput {
    /// 标题行（TS `theme.fg("toolTitle", theme.bold(...))` 那条）。
    pub title: Vec<ToolRenderSpan>,
    /// 标题之下的有序内容块。
    pub blocks: Vec<ToolRenderBlock>,
}

/// 扩展工具的自定义渲染回调。返回 `None` = 用内置 fallback 渲染。
pub type ToolRenderFn = Arc<dyn Fn(&ToolRenderContext) -> Option<ToolRenderOutput> + Send + Sync>;

// ============================================================================
// Tool Definition
// ============================================================================

/// Execute callback for a custom tool.
///
/// Takes (`tool_call_id`, params, signal) and returns a `ToolCallOutput`.
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

/// Tool definition matching the original TypeScript `ToolDefinition` interface.
#[derive(Clone, Serialize, Deserialize, Default)]
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
    /// Optional provider-side constrained sampling config (match TS
    /// `ToolDefinition.constrainedSampling`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<pi_ai::types::ConstrainedSamplingConfig>,
    #[serde(default)]
    pub render_shell: Option<String>,
    #[serde(default)]
    pub execution_mode: Option<String>,
    #[serde(skip)]
    pub execute: Option<ToolExecuteFn>,
    /// 自定义渲染（对齐 TS `renderCall` / `renderResult`）：TUI 渲染该工具的
    /// 块时调用。`None` = 用内置 fallback 渲染。
    #[serde(skip)]
    pub render: Option<ToolRenderFn>,
    /// Provenance of the tool definition (builtin / extension / sdk),
    /// matching TS `ToolDefinitionEntry.sourceInfo`.
    #[serde(skip)]
    pub source_info: Option<SourceInfo>,
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
            .field("render", &self.render.as_ref().map(|_| "Some(fn)"))
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
    /// Prompt guidelines for the tool, matching TS `ToolInfo.promptGuidelines`.
    pub prompt_guidelines: Option<Vec<String>>,
    /// Provenance of the tool definition, matching TS `ToolInfo.sourceInfo`.
    pub source_info: Option<SourceInfo>,
}

// ============================================================================
// ExtensionContext
// ============================================================================

/// Context provided to extensions for interacting with the agent.
#[derive(Clone)]
pub struct ExtensionContext {
    pub session_id: String,
    pub is_connected: bool,
    /// 运行模式（对齐 TS `ctx.mode`）：`"tui"`（interactive TUI）、
    /// `"cli"`、`"headless"`。扩展据此决定是否展示 TUI 交互菜单。
    /// 默认 `"cli"`，interactive 模式启动时
    /// 通过 [`Self::set_mode`] 置为 `"tui"`。
    pub mode: String,
    /// 是否绑定了真实 UI（对齐 TS `ctx.hasUI` = `uiContext !== noOpUIContext`）。
    ///
    /// 宿主在换绑真实 UI（interactive TUI、带 UI 的客户端）时置为 `true`；
    /// headless/print 保持 `false`，扩展据此决定要不要弹确认框。
    pub has_ui: bool,
    /// 当前项目是否受信任（对齐 TS `ctx.isProjectTrusted()`）。
    ///
    /// 由宿主按 `resolve_project_trusted`（显式 override → 无需信任的项目
    /// 资源 → 已存决策 → `defaultTrust` 策略）算出；需要询问而当时没有 UI
    /// 时保守取 `false`。
    pub is_project_trusted: bool,
    pub ui: ExtensionUIContext,
    pub runtime: RuntimeHandle,
}

impl ExtensionContext {
    #[must_use]
    pub const fn new(
        session_id: String,
        is_connected: bool,
        ui: ExtensionUIContext,
        runtime: RuntimeHandle,
    ) -> Self {
        Self {
            session_id,
            is_connected,
            mode: String::new(),
            has_ui: false,
            is_project_trusted: false,
            ui,
            runtime,
        }
    }

    /// 设置运行模式（`"tui"` / `"cli"` / `"headless"`）。
    pub fn set_mode(&mut self, mode: impl Into<String>) {
        self.mode = mode.into();
    }

    /// 标记是否绑定了真实 UI（对齐 TS `ctx.hasUI`）。
    pub fn set_has_ui(&mut self, has_ui: bool) {
        self.has_ui = has_ui;
    }

    /// 设置项目信任状态（对齐 TS `ctx.isProjectTrusted()`）。
    pub fn set_project_trusted(&mut self, trusted: bool) {
        self.is_project_trusted = trusted;
    }
}

// ============================================================================
// ExtensionUIContext
// ============================================================================

/// UI context for extensions to interact with the user interface.
///
/// Dialog methods (`select`, `input`) return a future that resolves with the
/// user's choice; fire-and-forget methods (`notify`, `set_status`, …) send a
/// request and return immediately. Matching TS `ExtensionUIContext`.
#[derive(Clone)]
#[allow(clippy::type_complexity)]
pub struct ExtensionUIContext {
    pub notify: Arc<dyn Fn(&str, &Value) + Send + Sync>,
    pub set_status: Arc<dyn Fn(&str, &str) + Send + Sync>,
    pub confirm: Arc<dyn Fn(&str, &Value) -> bool + Send + Sync>,
    /// Show a selector and return the user's choice (matching TS `ui.select`).
    pub select: Arc<
        dyn Fn(
                &str,
                &[String],
                Option<&Value>,
            ) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>
            + Send
            + Sync,
    >,
    /// Show a text input dialog (matching TS `ui.input`).
    pub input: Arc<
        dyn Fn(
                &str,
                Option<&str>,
                Option<&Value>,
            ) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>
            + Send
            + Sync,
    >,
    /// Show a multi-line editor dialog (matching TS `ui.editor(title, prefill?)`).
    pub editor: Arc<
        dyn Fn(
                &str,
                Option<&str>,
            ) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>
            + Send
            + Sync,
    >,
    /// Set a widget above/below the editor (matching TS `ui.setWidget`).
    /// `opts` may carry `{"placement": "aboveEditor"|"belowEditor"}`.
    pub set_widget: Arc<dyn Fn(&str, Option<&[String]>, Option<&Value>) + Send + Sync>,
    /// Replace the spinner/working text (matching TS `ui.setWorkingMessage`).
    /// `None` restores the default.
    pub set_working_message: Arc<dyn Fn(Option<&str>) + Send + Sync>,
    /// Show/hide the spinner line while busy (TS `ui.setWorkingVisible`).
    pub set_working_visible: Arc<dyn Fn(bool) + Send + Sync>,
    /// Current editor text (TS `ui.getEditorText`). Headless/RPC modes return
    /// an empty string — TS does the same because the host owns the editor.
    pub get_editor_text: Arc<dyn Fn() -> String + Send + Sync>,
    /// Set the terminal window/tab title (matching TS `ui.setTitle`).
    pub set_title: Arc<dyn Fn(&str) + Send + Sync>,
    /// Set the text in the input editor (matching TS `ui.setEditorText`).
    pub set_editor_text: Arc<dyn Fn(&str) + Send + Sync>,
}

impl ExtensionUIContext {
    /// A no-op UI context: dialogs resolve with `None`/`false`, fire-and-forget
    /// methods do nothing. Used by headless modes that don't wire a UI.
    #[must_use]
    pub fn noop() -> Self {
        Self {
            notify: Arc::new(|_, _| {}),
            set_status: Arc::new(|_, _| {}),
            confirm: Arc::new(|_, _| false),
            select: Arc::new(|_, _, _| Box::pin(async { None })),
            input: Arc::new(|_, _, _| Box::pin(async { None })),
            editor: Arc::new(|_, _| Box::pin(async { None })),
            set_widget: Arc::new(|_, _, _| {}),
            set_working_message: Arc::new(|_| {}),
            set_working_visible: Arc::new(|_| {}),
            get_editor_text: Arc::new(String::new),
            set_title: Arc::new(|_| {}),
            set_editor_text: Arc::new(|_| {}),
        }
    }
}

// ============================================================================
// RuntimeHandle
// ============================================================================

/// 会话 custom entry 写入回调（避免 type_complexity 警告）。
type CustomEntryFn = Arc<dyn Fn(&str, Option<Value>) + Send + Sync>;

/// Handle for extensions to interact with the agent runtime.
#[derive(Clone)]
pub struct RuntimeHandle {
    pub send_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
    pub send_user_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
    /// 同步入队一条 follow-up 消息（agent 空闲/已 settled 时也会被消费，
    /// 用于扩展的自动延续）。`trigger_turn` 语义
    /// 与 `send_message` 的 `deliverAs: "followUp"` 一致，但入队是同步的，
    /// 不依赖异步 task 的调度时序。
    pub queue_follow_up: Arc<dyn Fn(String) + Send + Sync>,
    /// 追加一条 custom entry 到会话历史（对齐原版扩展的
    /// `sessionManager.appendCustomEntry`；扩展自己的状态靠它跨
    /// compaction 保留、重启可恢复）。
    pub append_custom_entry: CustomEntryFn,
    /// 读取会话中全部 custom entries（`{customType, data}` 列表，同步）。
    pub get_custom_entries: Arc<dyn Fn() -> Vec<Value> + Send + Sync>,
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
    /// Register a provider config (match TS extension `registerProvider`,
    /// #019e4ad68). The payload is `{ "providerId": "...", "config": {...} }`
    /// (TS passes the provider id separately from the display-name config);
    /// returns an error for structurally invalid registrations (match TS,
    /// which throws).
    pub register_provider: Arc<dyn Fn(Value) -> Result<(), String> + Send + Sync>,
}

impl RuntimeHandle {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        send_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
        send_user_message: Arc<dyn Fn(String, Option<Value>) + Send + Sync>,
        queue_follow_up: Arc<dyn Fn(String) + Send + Sync>,
        append_custom_entry: CustomEntryFn,
        get_custom_entries: Arc<dyn Fn() -> Vec<Value> + Send + Sync>,
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
        register_provider: Arc<dyn Fn(Value) -> Result<(), String> + Send + Sync>,
    ) -> Self {
        Self {
            send_message,
            send_user_message,
            queue_follow_up,
            append_custom_entry,
            get_custom_entries,
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
            register_provider,
        }
    }

    /// Create a no-op `RuntimeHandle` for testing.
    pub fn noop() -> Self {
        Self::new(
            Arc::new(|_, _| {}),
            Arc::new(|_, _| {}),
            Arc::new(|_| {}),
            Arc::new(|_, _| {}),
            Arc::new(Vec::new),
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
            Arc::new(String::new),
            Arc::new(|_| {}),
            Arc::new(String::new),
            Arc::new(String::new),
            Arc::new(|| None),
            Arc::new(String::new),
            Arc::new(|| None),
            Arc::new(String::new),
            Arc::new(String::new),
            Arc::new(Vec::new),
            Arc::new(Vec::new),
            Arc::new(Vec::new),
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
            Arc::new(|_| Ok(())),
        )
    }
}

// ============================================================================
// ResourcesDiscoverResult
// ============================================================================

/// Result from a  event handler.
/// Extensions return paths to additional resources they provide.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesDiscoverResult {
    #[serde(default)]
    pub skill_paths: Vec<String>,
    #[serde(default)]
    pub prompt_paths: Vec<String>,
    #[serde(default)]
    pub theme_paths: Vec<String>,
}

// ============================================================================
// ProjectTrustResult
// ============================================================================

/// Result from a  event handler.
/// Extensions can decide whether a project should be trusted.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustResult {
    /// Whether the project is trusted.
    pub trusted: ProjectTrustDecision,
    #[serde(default)]
    pub remember: bool,
}

/// Trust decision from an extension.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectTrustDecision {
    Yes,
    No,
    Undecided,
}

// ============================================================================
// UserBashResult
// ============================================================================

/// Result from a `user_bash` event handler. Must set exactly one of
/// `operations` or `result` (TS `UserBashEventResult` union). Invalid results
/// and unsupported `operations` fail closed (the session does not fall back to
/// local execution).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserBashResult {
    /// Custom operations to use for execution (e.g., custom exec function).
    /// If set, the session will use these operations instead of the default bash execution.
    #[serde(default)]
    pub operations: Option<serde_json::Value>,
    /// Full replacement: extension handled execution, use this result.
    /// If set, the session will use this result directly.
    #[serde(default)]
    pub result: Option<serde_json::Value>,
}

// ============================================================================
// Boundary events (turn_end / agent_before_settle)
// ============================================================================

/// Outcome of a completed turn/run (TS `AgentActivityOutcome`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentActivityOutcome {
    Completed,
    Aborted,
    Error,
}

/// Append-only session entry draft an extension may add at a boundary
/// (TS `SessionBoundaryDraft`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum SessionBoundaryDraft {
    #[serde(rename = "custom", rename_all = "camelCase")]
    Custom {
        custom_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    #[serde(rename = "custom_message", rename_all = "camelCase")]
    CustomMessage {
        custom_type: String,
        content: serde_json::Value,
        display: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },
    #[serde(rename = "context_edit", rename_all = "camelCase")]
    ContextEdit {
        target_id: String,
        replacement: serde_json::Value,
    },
    #[serde(rename = "compaction", rename_all = "camelCase")]
    Compaction {
        summary: String,
        /// `None`/`null` creates a self-retaining compaction.
        #[serde(default)]
        first_kept_entry_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<serde_json::Value>,
    },
}

/// Which boundary is being dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryKind {
    TurnEnd,
    AgentBeforeSettle,
}

/// Model-context preview handed to boundary handlers (TS
/// `BoundaryContextPreview`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryContextPreview {
    pub context_entries: Vec<serde_json::Value>,
    pub context_messages: Vec<serde_json::Value>,
    pub llm_messages: Vec<serde_json::Value>,
    pub pending_messages: Vec<serde_json::Value>,
    pub can_continue: bool,
}

/// Which way pi leans on a prompt cache refresh (TS `CacheWarmingAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheWarmingAction {
    /// Send the refresh.
    Warm,
    /// Stop warming until the next real request.
    Stop,
}

/// Fired before each prompt cache refresh (TS `CacheWarmingDecisionEvent`).
/// Everything else an extension might want (model, idle state, context size)
/// is on the context.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecisionEvent {
    /// Event discriminator, always `"cache_warming_decision"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Price of this refresh.
    pub warm_cost: f64,
    /// Extra price of the next real request if the entry is lost.
    pub miss_cost: f64,
    /// Estimated chance the cache is used again before it expires.
    pub continuation_probability: f64,
    /// Pi's own decision, which the handler may override.
    pub action: CacheWarmingAction,
}

/// Mutable boundary state visible to each handler (TS `BoundaryState`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryState {
    pub entries: Vec<SessionBoundaryDraft>,
    #[serde(rename = "continue")]
    pub continue_: bool,
    pub context: BoundaryContextPreview,
    pub outcome: AgentActivityOutcome,
}

/// Result returned by a boundary handler (TS `BoundaryResult`). Returning
/// `None` (or all-`None` fields) leaves the state unchanged.
#[derive(Debug, Clone, Default)]
pub struct BoundaryResult {
    pub entries: Option<Vec<SessionBoundaryDraft>>,
    pub continue_: Option<bool>,
}

/// Outcome of a boundary dispatch (TS `BoundaryDispatchResult`).
#[derive(Debug, Clone)]
pub struct BoundaryDispatchResult {
    pub entries: Vec<SessionBoundaryDraft>,
    pub continue_: bool,
    pub context: BoundaryContextPreview,
    pub valid: bool,
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
    #[must_use]
    pub const fn new(sender: tokio::sync::mpsc::UnboundedSender<Value>) -> Self {
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
#[must_use]
    pub fn new() -> Self {
        Self {
            hook_runner: HookRunner::new(),
            tools: Vec::new(),
            commands: Vec::new(),
            shortcuts: Vec::new(),
            flags: Vec::new(),
        }
    }

    /// Register a `HookHandler` with the given provenance (`source_info`).
    ///
    /// `source_info` is stamped onto every tool/command the handler
    /// registers, mirroring TS where the loader sets `extension.sourceInfo`
    /// and `registerTool`/`registerCommand` attach it implicitly. Built-in
    /// extensions pass `create_builtin_source_info(name)`; a future runtime
    /// loader would pass the extension file's `SourceInfo`.
    pub fn register(&mut self, handler: Box<dyn HookHandler>, source_info: SourceInfo) {
        // Collect tools
        let mut tool_reg = ToolRegistry::new(source_info.clone());
        handler.register_tools(&mut tool_reg);
        self.tools.extend(tool_reg.into_vec());

        // Collect commands
        let mut cmd_reg = CommandRegistry::new(source_info.clone());
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

    /// Access the `HookRunner` for event dispatch.
    #[must_use]
    pub const fn hook_runner(&self) -> &HookRunner {
        &self.hook_runner
    }

    /// Check if any handlers are registered.
    #[must_use]
    pub fn has_handlers(&self) -> bool {
        self.hook_runner.has_handlers()
    }

    /// Number of registered handlers.
    #[must_use]
    pub fn handler_count(&self) -> usize {
        self.hook_runner.handler_count()
    }

    /// Get collected tools.
    #[must_use]
    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
    }

    /// Get collected commands.
    #[must_use]
    pub fn commands(&self) -> &[RegisteredCommand] {
        &self.commands
    }

    /// Get collected shortcuts.
    #[must_use]
    pub fn shortcuts(&self) -> &[RegisteredShortcut] {
        &self.shortcuts
    }

    /// Get collected flags.
    #[must_use]
    pub fn flags(&self) -> &[RegisteredFlag] {
        &self.flags
    }

    /// Tools that cannot be sent to a provider: an extension tool must declare
    /// an object parameter schema (TS #9300). Returns one message per offending
    /// tool so a loader can report them.
    #[must_use]
    pub fn validate_tools(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for tool in &self.tools {
            let valid = tool
                .definition
                .parameters
                .as_ref()
                .is_some_and(serde_json::Value::is_object);
            if !valid {
                errors.push(format!(
                    "Tool \"{}\" must define an object parameter schema.",
                    tool.definition.name
                ));
            }
        }
        errors
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use async_trait::async_trait;
    use super::*;

    #[test]
    fn test_extension_registry_collect_tools() {
        use hook::{HookHandler, ToolRegistry};

        struct ToolHandler;

        #[async_trait]
        impl HookHandler for ToolHandler {
            fn name(&self) -> &'static str {
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
        reg.register(Box::new(ToolHandler), create_builtin_source_info("tool_handler"));

        let tools = reg.collect_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "test_tool");
    }

    /// `source_info` passed to `ExtensionRegistry::register` is stamped onto
    /// every registered tool/command automatically (mirrors TS where the
    /// loader's `extension.sourceInfo` is attached by `registerTool`/
    /// `registerCommand` implicitly).
    #[test]
    fn test_source_info_stamped_onto_tools_and_commands() {
        use hook::{CommandRegistration, HookHandler, ToolRegistry};

        struct ProvenanceHandler;

        #[async_trait]
        impl HookHandler for ProvenanceHandler {
            fn name(&self) -> &'static str {
                "provenance"
            }
            fn register_tools(&self, tools: &mut ToolRegistry) {
                tools.register("t", ToolDefinition {
                    name: "t".into(),
                    description: "t".into(),
                    ..Default::default()
                });
            }
            fn register_commands(&self, commands: &mut hook::CommandRegistry) {
                commands.register("c", CommandRegistration {
                    description: "c".into(),
                    execute: std::sync::Arc::new(|_, _| Box::pin(async {})),
                    get_argument_completions: None,
                });
            }
        }

        let mut reg = ExtensionRegistry::new();
        let si = create_source_info(
            "/ext/x.ts".into(),
            "local".into(),
            SourceScope::Project,
            SourceOrigin::Package,
            Some("/ext".into()),
        );
        reg.register(Box::new(ProvenanceHandler), si.clone());

        let tools = reg.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].source_info, si);

        let cmds = reg.commands();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].source_info, si);
        assert!(cmds[0].get_argument_completions.is_none());
    }

    /// `get_argument_completions` callback is carried through registration
    /// and is invokable (returns `Some` items). Mirrors TS
    /// `RegisteredCommand.getArgumentCompletions`.
    #[tokio::test]
    async fn test_command_argument_completions_callback() {
        use hook::{CommandRegistry, CommandRegistration, HookHandler};

        struct CompHandler;

        #[async_trait]
        impl HookHandler for CompHandler {
            fn name(&self) -> &'static str {
                "comp"
            }
            fn register_commands(&self, commands: &mut CommandRegistry) {
                let cb: ArgumentCompletionsFn = std::sync::Arc::new(|prefix: String| {
                    Box::pin(async move {
                        if prefix.is_empty() {
                            None
                        } else {
                            Some(vec![
                                AutocompleteItem {
                                    value: format!("{prefix}-a"),
                                    label: format!("A for {prefix}"),
                                    description: None,
                                },
                            ])
                        }
                    })
                });
                commands.register("comp", CommandRegistration {
                    description: "comp".into(),
                    execute: std::sync::Arc::new(|_, _| Box::pin(async {})),
                    get_argument_completions: Some(cb),
                });
            }
        }

        let mut reg = ExtensionRegistry::new();
        reg.register(Box::new(CompHandler), create_builtin_source_info("comp"));

        let cmds = reg.commands();
        assert_eq!(cmds.len(), 1);
        let Some(completions) = cmds[0].get_argument_completions.as_ref() else {
            panic!("completions callback should be present");
        };
        let items = completions("foo".to_string()).await;
        assert_eq!(items.as_ref().map(|v| v.len()), Some(1));
        assert_eq!(items.as_ref().map(|v| v[0].value.as_str()), Some("foo-a"));

        let empty = completions(String::new()).await;
        assert!(empty.is_none());
    }

    /// `ToolInvocation`：中止信号语义（对齐 TS `AbortSignal`）——已中止立即返回，
    /// 未中止时等信号；信号通道断开（run 结束）不算中止。
    #[tokio::test]
    async fn tool_invocation_abort_semantics() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut invocation = ToolInvocation::new("call-1", Some(rx), noop_tool_update());

        assert!(!invocation.is_aborted());
        // 未中止：future 挂起，不会自己返回。
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), invocation.aborted())
                .await
                .is_err()
        );

        tx.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), invocation.aborted())
            .await
            .expect("abort wakes the waiter");
        assert!(invocation.is_aborted());

        // 已中止的接收端立刻返回。
        tokio::time::timeout(std::time::Duration::from_millis(50), invocation.aborted())
            .await
            .expect("already aborted returns immediately");
    }

    /// 信号通道被 drop（本轮 run 结束）不能误报成"已中止"，否则长跑工具会
    /// 在会话收尾时被莫名打断。
    #[tokio::test]
    async fn dropped_abort_sender_is_not_abort() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut invocation = ToolInvocation::new("call-1", Some(rx), noop_tool_update());
        drop(tx);
        assert!(!invocation.is_aborted());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), invocation.aborted())
                .await
                .is_err(),
            "sender dropped must not resolve as aborted"
        );
    }

    /// 未接 UI 时 `update` 是空操作；接上时收到同一份输出。
    #[test]
    fn tool_invocation_update_forwards_output() {
        let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let invocation = ToolInvocation::new(
            "call-1",
            None,
            Arc::new(move |output: ToolCallOutput| {
                sink.lock().unwrap().push(
                    output.content[0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }),
        );
        let output = ToolCallOutput {
            content: vec![serde_json::json!({ "type": "text", "text": "partial" })],
            details: None,
            is_error: false,
            terminate: None,
        };
        invocation.update(&output);
        invocation.update(&output);
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["partial".to_string(), "partial".to_string()]
        );

        // noop：调用不 panic
        ToolInvocation::noop().update(&output);
    }
}
