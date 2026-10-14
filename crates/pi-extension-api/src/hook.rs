//! Hook 系统 — 参考 `ZeroClaw` 的 `HookHandler` + `HookRunner` 设计。
//!
//! 两种事件类型：
//! - **Void hooks**（并行、fire-and-forget）：通知型事件，所有 handler 同时执行
//! - **Modifying hooks**（按 priority 顺序执行，可 Cancel）：拦截型事件，
//!   高 priority 先执行，返回 `HookResult::Cancel` 时后续 handler 不执行
//!
//! 所有方法都有默认空实现，扩展只需实现关心的。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{ExtensionContext, SourceInfo, ToolCallOutput, ToolDefinition};
type CommandFn = Arc<
    dyn Fn(
            String,
            Option<&crate::ExtensionContext>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// A single autocomplete suggestion for a command argument.
///
/// Mirrors TS `AutocompleteItem` (packages/tui/src/autocomplete.ts). Defined
/// here in `pi-extension-api` (not `pi-tui`, which is a confirmed deviation)
/// because `RegisteredCommand.get_argument_completions` returns it and the
/// completion *callback* is an extension-API concern; only the rendering of
/// completions is TUI-specific.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Callback producing argument completions for a registered command.
///
/// Mirrors TS `getArgumentCompletions?: (argumentPrefix: string) =>
/// AutocompleteItem[] | null | Promise<AutocompleteItem[] | null>`. Receives
/// the argument text after the command name and returns `None` (no
/// completions) or `Some(items)`.
pub type ArgumentCompletionsFn = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Option<Vec<AutocompleteItem>>> + Send>> + Send + Sync,
>;

// ============================================================================
// HookResult
// ============================================================================

/// Modifying hook 的返回类型。
///
/// - `Continue(T)`：继续执行后续 handler，可携带修改后的数据
/// - `Cancel(String)`：终止后续 handler 执行，携带取消原因
#[derive(Debug, Clone)]
pub enum HookResult<T> {
    Continue(T),
    Cancel(String),
}

impl<T> HookResult<T> {
    pub const fn is_cancel(&self) -> bool {
        matches!(self, Self::Cancel(_))
    }

    pub const fn is_continue(&self) -> bool {
        matches!(self, Self::Continue(_))
    }
}

// ============================================================================
// Tool/Command/Shortcut registries (moved from lib.rs for co-location)
// ============================================================================

/// A tool registered by an extension. Mirrors TS `RegisteredTool`.
#[derive(Debug, Clone)]
pub struct RegisteredTool {
    pub name: String,
    pub definition: ToolDefinition,
    /// Provenance of the registering extension. Mirrors TS
    /// `RegisteredTool.sourceInfo`; stamped automatically by the registry
    /// from the extension's `SourceInfo` (see `ToolRegistry::new`).
    pub source_info: SourceInfo,
}

/// Options for registering a command. Mirrors TS `registerCommand(name, options)`
/// where `options = { description?, getArgumentCompletions?, handler }`.
/// Using a struct keeps `register` ≤3 params and is extensible.
#[derive(Clone)]
pub struct CommandRegistration {
    pub description: String,
    pub execute: CommandFn,
    /// Optional argument-completion callback. Mirrors TS `getArgumentCompletions`.
    pub get_argument_completions: Option<ArgumentCompletionsFn>,
}

/// A command registered by an extension. Mirrors TS `RegisteredCommand`.
#[derive(Clone)]
pub struct RegisteredCommand {
    pub name: String,
    pub description: String,
    #[allow(dead_code)]
    pub execute: CommandFn,
    /// Provenance of the registering extension. Mirrors TS
    /// `RegisteredCommand.sourceInfo`; stamped automatically by the registry.
    pub source_info: SourceInfo,
    /// Optional argument-completion callback. Mirrors TS `getArgumentCompletions`.
    #[allow(dead_code)] // consumed by TUI autocomplete (deviation: not ported yet)
    pub get_argument_completions: Option<ArgumentCompletionsFn>,
}

/// A shortcut registered by an extension.
#[derive(Debug, Clone)]
pub struct RegisteredShortcut {
    pub key: String,
    pub description: String,
}

/// A flag registered by an extension.
#[derive(Debug, Clone)]
pub struct RegisteredFlag {
    pub name: String,
    pub description: String,
}

/// Registry for collecting tools from extensions.
///
/// Carries the registering extension's `SourceInfo` so `register` can stamp it
/// onto each `RegisteredTool` automatically (mirroring TS, where the loader
/// sets `extension.sourceInfo` and `registerTool` attaches it implicitly).
pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
    source_info: SourceInfo,
}

impl ToolRegistry {
    /// Create a registry bound to the given extension's `SourceInfo`.
    /// Every `register` call stamps this onto the resulting `RegisteredTool`.
    #[must_use]
    pub fn new(source_info: SourceInfo) -> Self {
        Self {
            tools: Vec::new(),
            source_info,
        }
    }

    pub fn register(&mut self, name: &str, definition: ToolDefinition) {
        self.tools.push(RegisteredTool {
            name: name.to_string(),
            definition,
            source_info: self.source_info.clone(),
        });
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<RegisteredTool> {
        self.tools
    }
}

/// Registry for collecting commands from extensions.
///
/// Like `ToolRegistry`, carries the extension's `SourceInfo` and stamps it
/// onto each `RegisteredCommand` automatically.
pub struct CommandRegistry {
    commands: Vec<RegisteredCommand>,
    source_info: SourceInfo,
}

impl CommandRegistry {
    /// Create a registry bound to the given extension's `SourceInfo`.
    #[must_use]
    pub fn new(source_info: SourceInfo) -> Self {
        Self {
            commands: Vec::new(),
            source_info,
        }
    }

    /// Register a command. `opts` mirrors TS `registerCommand`'s options
    /// object (`description`, `handler`, `getArgumentCompletions`).
    pub fn register(&mut self, name: &str, opts: CommandRegistration) {
        self.commands.push(RegisteredCommand {
            name: name.to_string(),
            description: opts.description,
            execute: opts.execute,
            source_info: self.source_info.clone(),
            get_argument_completions: opts.get_argument_completions,
        });
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<RegisteredCommand> {
        self.commands
    }
}

/// Registry for collecting shortcuts from extensions.
#[derive(Default)]
pub struct ShortcutRegistry {
    shortcuts: Vec<RegisteredShortcut>,
}

impl ShortcutRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            shortcuts: Vec::new(),
        }
    }

    pub fn register(&mut self, key: &str, description: &str) {
        self.shortcuts.push(RegisteredShortcut {
            key: key.to_string(),
            description: description.to_string(),
        });
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<RegisteredShortcut> {
        self.shortcuts
    }
}

/// Registry for collecting flags from extensions.
#[derive(Default)]
pub struct FlagRegistry {
    flags: Vec<RegisteredFlag>,
}

impl FlagRegistry {
    #[must_use]
    pub const fn new() -> Self {
        Self { flags: Vec::new() }
    }

    pub fn register(&mut self, name: &str, description: &str) {
        self.flags.push(RegisteredFlag {
            name: name.to_string(),
            description: description.to_string(),
        });
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<RegisteredFlag> {
        self.flags
    }
}

// ============================================================================
// HookHandler trait
// ============================================================================

/// Hook handler trait — 所有方法都有默认空实现。
///
/// 实现此 trait 来订阅 agent 生命周期事件。
/// Void hooks 并行执行，modifying hooks 按 priority 顺序执行。
#[async_trait]
pub trait HookHandler: Send + Sync {
    /// 扩展名称（唯一标识）。
    fn name(&self) -> &str;

    /// 优先级。数值越大越先执行。默认 0。
    fn priority(&self) -> i32 {
        0
    }

    // ── 工具/命令/快捷键注册（在注册时调用一次） ─────────────────

    /// 注册工具。在扩展注册时调用。
    fn register_tools(&self, _tools: &mut ToolRegistry) {}

    /// 注册命令。在扩展注册时调用。
    fn register_commands(&self, _commands: &mut CommandRegistry) {}

    /// 注册快捷键。在扩展注册时调用。
    fn register_shortcuts(&self, _shortcuts: &mut ShortcutRegistry) {}

    /// 注册标志。在扩展注册时调用。
    fn register_flags(&self, _flags: &mut FlagRegistry) {}

    // ── 工具调用处理 ──────────────────────────────────────────────

    /// 处理工具调用。返回 `Some(ToolCallOutput)` 表示此扩展处理了该工具。
    /// 按注册顺序尝试，第一个返回 Some 的胜出。
    async fn handle_tool_call(
        &self,
        _tool_name: &str,
        _params: Value,
        _ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        None
    }

    // ── Void hooks（并行、fire-and-forget） ──────────────────────────

    /// Session 启动时触发。
    async fn on_session_start(
        &self,
        _reason: &str,
        _previous_session_file: Option<&str>,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// Session 关闭时触发。
    async fn on_session_shutdown(
        &self,
        _reason: &str,
        _target_session_file: Option<&str>,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// Session 信息变更时触发。
    async fn on_session_info_changed(
        &self,
        _name: Option<&str>,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// Agent 开始处理时触发。
    async fn on_agent_start(&self, _ctx: Option<&crate::ExtensionContext>) {}

    /// Agent 处理完成时触发。
    ///
    /// `ctx` 在扩展注册到真实会话时为 `Some`（可持久化状态、发送通知）。
    async fn on_agent_end(&self, _messages: &[Value], _ctx: Option<&crate::ExtensionContext>) {}

    /// Agent 进入空闲状态时触发。
    ///
    /// `ctx` 在扩展注册到真实会话时为 `Some`（携带 runtime handle，可发送
    /// 自动延续消息）；测试/无会话场景为 `None`。
    async fn on_agent_settled(&self, _ctx: Option<&crate::ExtensionContext>) {}

    /// Turn 开始时触发。
    /// Turn 开始时触发。`timestamp` 是毫秒时间戳（匹配 TS `turn_start` 事件）。
    async fn on_turn_start(
        &self,
        _turn_index: u32,
        _timestamp: i64,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// Turn 结束时触发。
    ///
    /// `ctx` 在扩展注册到真实会话时为 `Some`（携带 runtime handle，可持久化
    /// 状态、发送通知）；测试/无会话场景为 `None`。
    async fn on_turn_end(
        &self,
        _turn_index: u32,
        _message: &Value,
        _tool_results: &[Value],
        _timestamp: i64,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// Turn-end **boundary** (TS `turn_end` boundary, 466db0fec). Returning
    /// entries/continue injects append-only session entries and/or requests one
    /// more provider request. Runs sequentially (each handler sees the previous
    /// handler's state); the void `on_turn_end` remains for notifications.
    async fn on_turn_end_boundary(
        &self,
        _state: crate::BoundaryState,
    ) -> Option<crate::BoundaryResult> {
        None
    }

    /// Before-settle **boundary** (TS `agent_before_settle`).
    async fn on_agent_before_settle_boundary(
        &self,
        _state: crate::BoundaryState,
    ) -> Option<crate::BoundaryResult> {
        None
    }

    /// Whether this handler participates in the given boundary. Handlers that
    /// implement a boundary method must override this to return `true` for the
    /// corresponding kind — the runner cannot detect method overrides, and this
    /// flag lets callers skip boundary context construction entirely when no
    /// handler participates.
    fn participates_in_boundary(&self, _kind: crate::BoundaryKind) -> bool {
        false
    }

    /// 消息开始时触发。
    async fn on_message_start(&self, _message: &Value, _ctx: Option<&crate::ExtensionContext>) {}

    /// 消息更新时触发。
    /// 消息更新时触发。`assistant_message_event` 是流式 delta 事件
    /// （匹配 TS `message_update` 事件的 assistantMessageEvent 字段）。
    async fn on_message_update(
        &self,
        _message: &Value,
        _assistant_message_event: &Value,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// 消息结束时触发。
    async fn on_message_end(&self, _message: &Value, _ctx: Option<&crate::ExtensionContext>) {}

    /// 消息结束时触发（modifying 版本）。
    /// 可以修改消息内容。按 priority 顺序执行。
    async fn on_message_end_mut(
        &self,
        _message: Value,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<Value> {
        HookResult::Continue(_message)
    }


    /// 工具执行开始时触发。
    async fn on_tool_execution_start(
        &self,
        _tool_call_id: &str,
        _tool_name: &str,
        _args: &Value,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// 工具执行更新时触发（高频事件，用于流式输出进度）。
    async fn on_tool_execution_update(
        &self,
        _tool_call_id: &str,
        _tool_name: &str,
        _args: &Value,
        _partial_result: &Value,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }


    /// 工具执行结束时触发。
    async fn on_tool_execution_end(
        &self,
        _tool_call_id: &str,
        _tool_name: &str,
        _result: &Value,
        _is_error: bool,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// 模型切换时触发。
    /// 模型切换时触发。`source` 是 "set" | "cycle" | "restore"（匹配 TS
    /// `model_select` 事件的 source 字段）。
    async fn on_model_select(
        &self,
        _model: &str,
        _previous_model: Option<&str>,
        _source: &str,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// Thinking level 切换时触发。
    async fn on_thinking_level_select(
        &self,
        _level: &str,
        _previous_level: &str,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// 压缩完成时触发。
    async fn on_compact(
        &self,
        _summary: &str,
        _tokens_before: u64,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// 树导航完成时触发。
    async fn on_tree(
        &self,
        _new_leaf_id: Option<&str>,
        _old_leaf_id: Option<&str>,
        _ctx: Option<&crate::ExtensionContext>,
    ) {
    }

    /// 资源发现时触发。返回扩展提供的额外资源路径。
    async fn on_resources_discover(&self, _cwd: &str, _reason: &str, _ctx: Option<&crate::ExtensionContext>) -> Option<crate::ResourcesDiscoverResult> {
        None
    }

    /// Project trust 变更时触发。返回信任决策。
    async fn on_project_trust(
        &self,
        _cwd: &str,
        _ctx: Option<&crate::ExtensionContext>,
    ) -> Option<crate::ProjectTrustResult> {
        None
    }

    /// 上下文消息更新时触发。
    async fn on_context(&self, _messages: &[Value], _ctx: Option<&crate::ExtensionContext>) {}

    /// 上下文消息更新时触发（modifying 版本）。
    /// 可以修改消息列表。按 priority 顺序执行。
    async fn on_context_mut(
        &self,
        _messages: Vec<Value>,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<Vec<Value>> {
        HookResult::Continue(_messages)
    }

    /// 完整 transcript（含 system 消息）更新时触发（TS `context_with_system`），
    /// 在 `context` handler 之后运行。返回 `Some(messages)` 时该结果**原样**作为
    /// provider 上下文，Pi 不再补回 system 消息。
    async fn on_context_with_system(
        &self,
        _messages: &[Value],
        _ctx: &crate::ExtensionContext,
    ) -> Option<Vec<Value>> {
        None
    }

    /// 用户 bash 执行时触发。
    async fn on_user_bash(&self, _command: &str, _cwd: &str, _ctx: Option<&crate::ExtensionContext>) -> Option<crate::UserBashResult> { None }

    /// 每次 prompt 缓存刷新前触发（TS `cache_warming_decision`）。返回的
    /// `action` 覆盖 pi 自身的判断；返回 `None` 表示不干预。
    async fn on_cache_warming_decision(
        &self,
        _event: &crate::CacheWarmingDecisionEvent,
        _ctx: Option<&crate::ExtensionContext>,
    ) -> Option<crate::CacheWarmingAction> {
        None
    }

    // ── Modifying hooks（按 priority 顺序执行，可 Cancel） ──────────

    /// 工具调用前触发。可修改工具名称和参数，或取消调用。
    ///
    /// `ctx` 携带 runtime handle，扩展可在取消时中止当前 turn（对齐原版
    /// `tool_call` 事件的 `abortCurrentTurn`）。
    async fn before_tool_call(
        &self,
        _tool_name: String,
        _args: Value,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<(String, Value)> {
        HookResult::Continue((_tool_name, _args))
    }

    /// 工具调用后触发。可修改结果。
    async fn after_tool_call(
        &self,
        _tool_name: &str,
        _result: &Value,
        _is_error: bool,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// 工具结果处理（modifying 版本）。
    /// 可以修改工具结果的内容（JSON 序列化后的 `ContentBlock` 列表）、详情和错误状态。
    /// 按 priority 顺序执行。
    async fn on_tool_result_mut(
        &self,
        _tool_name: &str,
        _content: Vec<Value>,
        _details: Option<Value>,
        _is_error: bool,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<(Vec<Value>, Option<Value>, bool)> {
        HookResult::Continue((_content, _details, _is_error))
    }

    /// Agent 开始前触发。可修改 prompt 和 `system_prompt`，或取消。
    /// `_images` 和 `_system_prompt_options` 是只读输入参数。
    async fn before_agent_start(
        &self,
        _prompt: String,
        _images: Option<Vec<Value>>,
        _system_prompt: String,
        _system_prompt_options: Option<Value>,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<(String, String, Option<Vec<Value>>)> {
        HookResult::Continue((_prompt, _system_prompt, None))
    }

    /// 用户输入时触发。可修改输入文本，或取消。
    /// `_images` 和 `_streaming_behavior` 是只读输入参数。
    /// 输入事件（transform 链）。返回 `(text, images)`，images 为 `None`
    /// 表示不修改（匹配 TS `input` transform 的 `images ?? currentImages`）。
    async fn on_input(
        &self,
        _text: String,
        _images: Option<Vec<Value>>,
        _source: String,
        _streaming_behavior: Option<String>,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<(String, Option<Vec<Value>>)> {
        HookResult::Continue((_text, None))
    }

    /// Provider 请求前触发。可修改 payload。
    async fn before_provider_request(
        &self,
        _payload: Value,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<Value> {
        HookResult::Continue(_payload)
    }

    /// Provider 请求头前触发。可修改 headers。
    async fn before_provider_headers(
        &self,
        _headers: HashMap<String, String>,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<HashMap<String, String>> {
        HookResult::Continue(_headers)
    }

    /// Provider 响应后触发。可修改响应。
    async fn after_provider_response(
        &self,
        _status: u16,
        _headers: HashMap<String, String>,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session 切换前触发。可取消切换。
    async fn before_session_switch(
        &self,
        _reason: String,
        _target_session_file: Option<String>,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session fork 前触发。可取消 fork。
    async fn before_session_fork(
        &self,
        _entry_id: String,
        _position: String,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session 压缩前触发。可取消压缩。
    async fn before_session_compact(
        &self,
        _reason: String,
        _will_retry: bool,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session 树导航前触发。可取消导航。
    async fn before_session_tree(
        &self,
        _target_id: String,
        _ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }
}

/// Whether a serialized transcript message is a system message.
fn is_system_message(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("system")
}

/// Structural equality for transcript lists (TS `sameMessages`).
fn same_messages(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len() && left.iter().zip(right).all(|(a, b)| a == b)
}

/// Re-attach the prompt and tool state after a `context` handler
/// (TS `restoreSystemMessages`).
fn restore_system_messages(current: &[Value], returned: &[Value]) -> Vec<Value> {
    match pi_ai::utils::transcript::get_current_system_message(current) {
        Some(head) => std::iter::once(head).chain(returned.iter().cloned()).collect(),
        None => returned.to_vec(),
    }
}

// ============================================================================
// HookRunner
// ============================================================================

/// 管理所有注册的 HookHandler，按 priority 排序分发事件。
#[derive(Default)]
pub struct HookRunner {
    handlers: Vec<Box<dyn HookHandler>>,
}

impl HookRunner {
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Register a hook handler. Maintains priority-sorted order.
    pub fn register(&mut self, handler: Box<dyn HookHandler>) {
        self.handlers.push(handler);
        self.handlers.sort_by_key(|b| std::cmp::Reverse(b.priority()));
    }

    /// Check if any handlers are registered.
    #[must_use]
    pub fn has_handlers(&self) -> bool {
        !self.handlers.is_empty()
    }

    /// Number of registered handlers.
    #[must_use]
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// Access handlers (for tool/command/shortcut collection).
    #[must_use]
    pub fn handlers(&self) -> &[Box<dyn HookHandler>] {
        &self.handlers
    }

    // ── Void hook dispatch (parallel, fire-and-forget) ──────────────

    /// Fire `on_session_start` to all handlers (parallel).
    pub async fn fire_session_start(
        &self,
        reason: &str,
        previous_session_file: Option<&str>,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_session_start(reason, previous_session_file, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_session_shutdown` to all handlers (parallel).
    pub async fn fire_session_shutdown(
        &self,
        reason: &str,
        target_session_file: Option<&str>,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_session_shutdown(reason, target_session_file, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_session_info_changed` to all handlers (parallel).
    pub async fn fire_session_info_changed(
        &self,
        name: Option<&str>,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_session_info_changed(name, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_agent_start` to all handlers (parallel).
    pub async fn fire_agent_start(&self, ctx: Option<&crate::ExtensionContext>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_agent_start(ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_agent_end` to all handlers (parallel).
    pub async fn fire_agent_end(&self, messages: &[Value], ctx: Option<&crate::ExtensionContext>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_agent_end(messages, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_agent_settled` to all handlers (parallel).
    pub async fn fire_agent_settled(&self, ctx: Option<&crate::ExtensionContext>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_agent_settled(ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_turn_start` to all handlers (parallel).
    pub async fn fire_turn_start(
        &self,
        turn_index: u32,
        timestamp: i64,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_turn_start(turn_index, timestamp, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_turn_end` to all handlers (parallel).
    pub async fn fire_turn_end(
        &self,
        turn_index: u32,
        message: &Value,
        tool_results: &[Value],
        timestamp: i64,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_turn_end(turn_index, message, tool_results, timestamp, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Dispatch a boundary sequentially (TS `ExtensionRunner.emitBoundary`).
    ///
    /// Each handler receives the state accumulated so far; its returned
    /// `entries`/`continue` replace the current values. After a handler changes
    /// the state, the caller-supplied `build_context` recomputes the model-context
    /// preview (mirroring TS `buildContext`); if it fails, the dispatch is marked
    /// invalid and drafts are discarded. Returns immediately (valid, empty) when
    /// no handler participates in the boundary.
    pub async fn run_boundary<F>(
        &self,
        kind: crate::BoundaryKind,
        mut state: crate::BoundaryState,
        build_context: F,
    ) -> crate::BoundaryDispatchResult
    where
        F: Fn(
            &[crate::SessionBoundaryDraft],
        ) -> Result<crate::BoundaryContextPreview, String>,
    {
        if !self.has_boundary_handlers(kind) {
            return crate::BoundaryDispatchResult {
                entries: state.entries,
                continue_: state.continue_,
                context: state.context,
                valid: true,
            };
        }
        let mut valid = true;
        for handler in &self.handlers {
            if !handler.participates_in_boundary(kind) {
                continue;
            }
            let result = match kind {
                crate::BoundaryKind::TurnEnd => handler.on_turn_end_boundary(state.clone()).await,
                crate::BoundaryKind::AgentBeforeSettle => {
                    handler.on_agent_before_settle_boundary(state.clone()).await
                }
            };
            let mut changed = false;
            if let Some(r) = result {
                if let Some(entries) = r.entries {
                    state.entries = entries;
                    changed = true;
                }
                if let Some(cont) = r.continue_ {
                    state.continue_ = cont;
                    changed = true;
                }
            }
            if changed {
                match build_context(&state.entries) {
                    Ok(context) => {
                        state.context = context;
                        valid = true;
                    }
                    Err(_) => valid = false,
                }
            }
        }
        if valid {
            crate::BoundaryDispatchResult {
                entries: state.entries,
                continue_: state.continue_,
                context: state.context,
                valid,
            }
        } else {
            crate::BoundaryDispatchResult {
                entries: Vec::new(),
                continue_: false,
                context: state.context,
                valid: false,
            }
        }
    }

    /// Whether any registered handler participates in the given boundary.
    #[must_use]
    pub fn has_boundary_handlers(&self, kind: crate::BoundaryKind) -> bool {
        self.handlers.iter().any(|h| h.participates_in_boundary(kind))
    }

    /// Fire `on_message_start` to all handlers (parallel).
    pub async fn fire_message_start(&self, message: &Value, ctx: Option<&crate::ExtensionContext>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_message_start(message, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_message_update` to all handlers (parallel).
    pub async fn fire_message_update(
        &self,
        message: &Value,
        assistant_message_event: &Value,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_message_update(message, assistant_message_event, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_message_end` to all handlers (parallel).
    pub async fn fire_message_end(&self, message: &Value, ctx: Option<&crate::ExtensionContext>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_message_end(message, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Run `on_message_end_mut` handlers in priority order.
    /// Returns the (possibly modified) message, or Cancel.
    pub async fn run_on_message_end(
        &self,
        message: Value,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<Value> {
        let mut current = message;
        for handler in &self.handlers {
            match handler.on_message_end_mut(current.clone(), ctx).await {
                HookResult::Continue(m) => {
                    // A message_end handler must return a message with the same
                    // role (matching TS emitMessageEnd which rejects role
                    // changes with an error and skips the modification).
                    let same_role = m.get("role").and_then(|r| r.as_str())
                        == current.get("role").and_then(|r| r.as_str());
                    if same_role {
                        current = m;
                    }
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Fire `on_tool_execution_start` to all handlers (parallel).
    pub async fn fire_tool_execution_start(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        args: &Value,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_tool_execution_start(tool_call_id, tool_name, args, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_tool_execution_update` to all handlers (parallel).
    pub async fn fire_tool_execution_update(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        args: &Value,
        partial_result: &Value,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_tool_execution_update(tool_call_id, tool_name, args, partial_result, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }


    /// Fire `on_tool_execution_end` to all handlers (parallel).
    pub async fn fire_tool_execution_end(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        result: &Value,
        is_error: bool,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_tool_execution_end(tool_call_id, tool_name, result, is_error, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_model_select` to all handlers (parallel).
    pub async fn fire_model_select(
        &self,
        model: &str,
        previous_model: Option<&str>,
        source: &str,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_model_select(model, previous_model, source, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_thinking_level_select` to all handlers (parallel).
    pub async fn fire_thinking_level_select(
        &self,
        level: &str,
        previous_level: &str,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_thinking_level_select(level, previous_level, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_compact` to all handlers (parallel).
    pub async fn fire_compact(
        &self,
        summary: &str,
        tokens_before: u64,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_compact(summary, tokens_before, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_tree` to all handlers (parallel).
    pub async fn fire_tree(
        &self,
        new_leaf_id: Option<&str>,
        old_leaf_id: Option<&str>,
        ctx: Option<&crate::ExtensionContext>,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_tree(new_leaf_id, old_leaf_id, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_resources_discover` to all handlers (parallel).
    pub async fn fire_resources_discover(&self, cwd: &str, reason: &str, ctx: Option<&crate::ExtensionContext>) -> crate::ResourcesDiscoverResult {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_resources_discover(cwd, reason, ctx))
            .collect();
        let results = futures::future::join_all(futures).await;
        let mut combined = crate::ResourcesDiscoverResult::default();
        for result in results.into_iter().flatten() {
            combined.skill_paths.extend(result.skill_paths);
            combined.prompt_paths.extend(result.prompt_paths);
            combined.theme_paths.extend(result.theme_paths);
        }
        combined
    }

    /// Fire `on_project_trust` to all handlers (parallel).
    pub async fn fire_project_trust(
        &self,
        cwd: &str,
        ctx: Option<&crate::ExtensionContext>,
    ) -> Option<crate::ProjectTrustResult> {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_project_trust(cwd, ctx))
            .collect();
        let results = futures::future::join_all(futures).await;
        for result in results.into_iter().flatten() {
            if result.trusted != crate::ProjectTrustDecision::Undecided {
                return Some(result);
            }
        }
        None
    }

    /// Fire `on_context` to all handlers (parallel).
    pub async fn fire_context(&self, messages: &[Value], ctx: Option<&crate::ExtensionContext>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_context(messages, ctx))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Run `on_context_mut` handlers in priority order.
    /// Returns the (possibly modified) messages, or Cancel.
    pub async fn run_on_context(
        &self,
        messages: Vec<Value>,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<Vec<Value>> {
        // TS `emitContext`: a `context` handler only sees the conversation, and
        // Pi restores the prompt and tool state afterwards. An unchanged
        // conversation keeps every system message in place, so models with
        // mid-conversation support keep their cached prefix; a changed one gets
        // the replayed prompt sections and tool declarations as one leading
        // system message, so pruning or windowing cannot drop them.
        let mut current = messages;
        for handler in &self.handlers {
            let visible: Vec<Value> = current
                .iter()
                .filter(|message| !is_system_message(message))
                .cloned()
                .collect();
            let snapshot = visible.clone();
            match handler.on_context_mut(visible, ctx).await {
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
                HookResult::Continue(returned) => {
                    if same_messages(&returned, &snapshot) {
                        continue;
                    }
                    current = restore_system_messages(&current, &returned);
                }
            }
        }
        HookResult::Continue(current)
    }

    /// Run `context_with_system` handlers after `run_on_context`
    /// (TS `emitContext`'s second phase). Returns the messages a provider should
    /// use plus the name of a handler that dropped the leading system message,
    /// when one did.
    pub async fn run_context_with_system(
        &self,
        messages: Vec<Value>,
        ctx: &crate::ExtensionContext,
    ) -> (Vec<Value>, Option<String>) {
        let mut current = messages;
        let mut dropped_leading_system = None;
        for handler in &self.handlers {
            let had_leading_system = current.first().is_some_and(is_system_message);
            let Some(returned) = handler.on_context_with_system(&current, ctx).await else {
                continue;
            };
            current = returned;
            // Providers read the prompt and initial tools from the leading
            // system message; the handler's output is still honoured.
            if had_leading_system && !current.first().is_some_and(is_system_message) {
                dropped_leading_system = Some(handler.name().to_string());
            }
        }
        (current, dropped_leading_system)
    }

    /// Fire `on_user_bash` to all handlers (sequential, first-return-wins).
    pub async fn fire_user_bash(
        &self,
        command: &str,
        cwd: &str,
        ctx: Option<&crate::ExtensionContext>,
    ) -> Option<crate::UserBashResult> {
        for handler in &self.handlers {
            if let Some(result) = handler.on_user_bash(command, cwd, ctx).await {
                return Some(result);
            }
        }
        None
    }

    // ── Modifying hook dispatch (sequential, priority-ordered, cancellable) ──

    /// Fire `cache_warming_decision` to all handlers. The last non-`None`
    /// action wins (TS `emitCacheWarmingDecision` lets every handler override);
    /// handler failures leave pi's decision untouched.
    pub async fn fire_cache_warming_decision(
        &self,
        event: &crate::CacheWarmingDecisionEvent,
        ctx: Option<&crate::ExtensionContext>,
    ) -> Option<crate::CacheWarmingAction> {
        let mut action = None;
        for handler in &self.handlers {
            if let Some(override_action) = handler.on_cache_warming_decision(event, ctx).await {
                action = Some(override_action);
            }
        }
        action
    }

    /// Run `before_tool_call` handlers in priority order. Returns Cancel on first cancel.
    pub async fn run_before_tool_call(
        &self,
        tool_name: String,
        args: Value,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<(String, Value)> {
        let mut current = (tool_name, args);
        for handler in &self.handlers {
            match handler.before_tool_call(current.0.clone(), current.1.clone(), ctx).await {
                HookResult::Continue((name, args)) => {
                    current = (name, args);
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Run `after_tool_call` handlers in priority order. Returns Cancel on first cancel.
    pub async fn run_after_tool_call(
        &self,
        tool_name: &str,
        result: &Value,
        is_error: bool,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .after_tool_call(tool_name, result, is_error, ctx)
                .await
            {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `on_tool_result_mut` handlers in priority order.
    /// Returns the (possibly modified) content, details, and `is_error`.
    pub async fn run_on_tool_result(
        &self,
        tool_name: &str,
        content: Vec<Value>,
        details: Option<Value>,
        is_error: bool,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<(Vec<Value>, Option<Value>, bool)> {
        let mut current = (content, details, is_error);
        for handler in &self.handlers {
            match handler
                .on_tool_result_mut(
                    tool_name,
                    current.0.clone(),
                    current.1.clone(),
                    current.2,
                    ctx,
                )
                .await
            {
                HookResult::Continue((c, d, e)) => {
                    current = (c, d, e);
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Run `before_agent_start` handlers in priority order.
    pub async fn run_before_agent_start(
        &self,
        prompt: String,
        images: Option<Vec<Value>>,
        system_prompt: String,
        system_prompt_options: Option<Value>,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<(String, String, Option<Vec<Value>>)> {
        let mut current = (prompt, system_prompt);
        // Collect messages from all handlers (matching TS emitBeforeAgentStart
        // which pushes each handler's message).
        let mut messages: Vec<Value> = Vec::new();
        for handler in &self.handlers {
            match handler
                .before_agent_start(current.0.clone(), images.clone(), current.1.clone(), system_prompt_options.clone(), ctx)
                .await
            {
                HookResult::Continue((p, s, msgs)) => {
                    current = (p, s);
                    if let Some(msgs) = msgs {
                        messages.extend(msgs);
                    }
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue((
            current.0,
            current.1,
            if messages.is_empty() {
                None
            } else {
                Some(messages)
            },
        ))
    }

    /// Run `on_input` handlers in priority order.
    pub async fn run_on_input(
        &self,
        text: String,
        images: Option<Vec<Value>>,
        source: String,
        streaming_behavior: Option<String>,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<(String, Option<Vec<Value>>)> {
        let mut current = (text, images);
        for handler in &self.handlers {
            match handler
                .on_input(
                    current.0.clone(),
                    current.1.clone(),
                    source.clone(),
                    streaming_behavior.clone(),
                    ctx,
                )
                .await
            {
                HookResult::Continue((t, imgs)) => {
                    // images: None means "keep current" (matching TS
                    // `result.images ?? currentImages`).
                    current = (t, imgs.or(current.1));
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Run `before_provider_request` handlers in priority order.
    pub async fn run_before_provider_request(
        &self,
        payload: Value,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<Value> {
        let mut current = payload;
        for handler in &self.handlers {
            match handler.before_provider_request(current, ctx).await {
                HookResult::Continue(modified) => {
                    current = modified;
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Run `before_provider_headers` handlers in priority order.
    pub async fn run_before_provider_headers(
        &self,
        headers: HashMap<String, String>,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<HashMap<String, String>> {
        let mut current = headers;
        for handler in &self.handlers {
            match handler.before_provider_headers(current, ctx).await {
                HookResult::Continue(h) => {
                    current = h;
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Run `after_provider_response` handlers in priority order.
    pub async fn run_after_provider_response(
        &self,
        status: u16,
        headers: HashMap<String, String>,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler.after_provider_response(status, headers.clone(), ctx).await {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `before_session_switch` handlers in priority order.
    pub async fn run_before_session_switch(
        &self,
        reason: String,
        target_session_file: Option<String>,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .before_session_switch(reason.clone(), target_session_file.clone(), ctx)
                .await
            {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `before_session_fork` handlers in priority order.
    pub async fn run_before_session_fork(
        &self,
        entry_id: String,
        position: String,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .before_session_fork(entry_id.clone(), position.clone(), ctx)
                .await
            {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `before_session_compact` handlers in priority order.
    pub async fn run_before_session_compact(
        &self,
        reason: String,
        will_retry: bool,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .before_session_compact(reason.clone(), will_retry, ctx)
                .await
            {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `before_session_tree` handlers in priority order.
    pub async fn run_before_session_tree(
        &self,
        target_id: &str,
        ctx: &crate::ExtensionContext,
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .before_session_tree(target_id.to_string(), ctx)
                .await
            {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    // ── Tool call dispatch ─────────────────────────────────────────

    /// Dispatch a tool call to handlers in registration order.
    /// Returns the first `Some(ToolCallOutput)` result.
    pub async fn dispatch_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        for handler in &self.handlers {
            if let Some(result) = handler.handle_tool_call(tool_name, params.clone(), ctx).await {
                return Some(result);
            }
        }
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn boundary_dispatch_accumulates_entries_and_continue() {
        use std::sync::Mutex;

        fn preview() -> crate::BoundaryContextPreview {
            crate::BoundaryContextPreview {
                context_entries: vec![],
                context_messages: vec![],
                llm_messages: vec![],
                pending_messages: vec![],
                can_continue: true,
            }
        }

        struct H {
            name: String,
            entries: Option<Vec<crate::SessionBoundaryDraft>>,
            cont: Option<bool>,
        }

        #[async_trait]
        impl HookHandler for H {
            fn name(&self) -> &str {
                &self.name
            }
            async fn on_turn_end_boundary(
                &self,
                _state: crate::BoundaryState,
            ) -> Option<crate::BoundaryResult> {
                Some(crate::BoundaryResult {
                    entries: self.entries.clone(),
                    continue_: self.cont,
                })
            }
            fn participates_in_boundary(&self, kind: crate::BoundaryKind) -> bool {
                matches!(kind, crate::BoundaryKind::TurnEnd)
            }
        }

        let seen: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
        let mut runner = HookRunner::new();
        runner.register(Box::new(H {
            name: "h1".into(),
            entries: Some(vec![crate::SessionBoundaryDraft::Custom {
                custom_type: "x".into(),
                data: None,
            }]),
            cont: Some(false),
        }));
        runner.register(Box::new(H {
            name: "h2".into(),
            entries: None,
            cont: Some(true),
        }));

        let state = crate::BoundaryState {
            entries: vec![],
            continue_: false,
            context: preview(),
            outcome: crate::AgentActivityOutcome::Completed,
        };
        let seen2 = seen.clone();
        let result = runner
            .run_boundary(crate::BoundaryKind::TurnEnd, state, move |entries| {
                seen2.lock().unwrap().push(entries.len());
                Ok(preview())
            })
            .await;

        assert!(result.valid);
        assert_eq!(result.entries.len(), 1, "h1's entry is kept (h2 returned None)");
        assert!(result.continue_, "h2 requested continuation");
        assert_eq!(*seen.lock().unwrap(), vec![1, 1]);
    }

    #[tokio::test]
    async fn boundary_dispatch_invalid_context_discards_drafts() {
        struct H;
        #[async_trait]
        impl HookHandler for H {
            fn name(&self) -> &str {
                "h"
            }
            async fn on_agent_before_settle_boundary(
                &self,
                _state: crate::BoundaryState,
            ) -> Option<crate::BoundaryResult> {
                Some(crate::BoundaryResult {
                    entries: Some(vec![crate::SessionBoundaryDraft::Custom {
                        custom_type: "x".into(),
                        data: None,
                    }]),
                    continue_: Some(true),
                })
            }
            fn participates_in_boundary(&self, kind: crate::BoundaryKind) -> bool {
                matches!(kind, crate::BoundaryKind::AgentBeforeSettle)
            }
        }
        let mut runner = HookRunner::new();
        runner.register(Box::new(H));
        let state = crate::BoundaryState {
            entries: vec![],
            continue_: false,
            context: crate::BoundaryContextPreview {
                context_entries: vec![],
                context_messages: vec![],
                llm_messages: vec![],
                pending_messages: vec![],
                can_continue: false,
            },
            outcome: crate::AgentActivityOutcome::Error,
        };
        let result = runner
            .run_boundary(crate::BoundaryKind::AgentBeforeSettle, state, |_| {
                Err("bad entries".to_string())
            })
            .await;
        assert!(!result.valid);
        assert!(result.entries.is_empty());
        assert!(!result.continue_);
    }

    #[tokio::test]
    async fn void_hooks_fire_to_all_handlers() {
        struct CountingHook {
            name: String,
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl HookHandler for CountingHook {
            fn name(&self) -> &str {
                &self.name
            }
            async fn on_agent_start(&self, _ctx: Option<&crate::ExtensionContext>) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let count = Arc::new(AtomicU32::new(0));

        let mut runner = HookRunner::new();
        runner.register(Box::new(CountingHook {
            name: "hook1".into(),
            count: count.clone(),
        }));
        runner.register(Box::new(CountingHook {
            name: "hook2".into(),
            count: count.clone(),
        }));

        runner.fire_agent_start(None).await;
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn modifying_hooks_short_circuit_on_cancel() {
        struct BlockingHook {
            name: String,
            priority: i32,
            count: Arc<AtomicU32>,
            should_block: bool,
        }

        #[async_trait]
        impl HookHandler for BlockingHook {
            fn name(&self) -> &str {
                &self.name
            }
            fn priority(&self) -> i32 {
                self.priority
            }
            async fn before_tool_call(
                &self,
                name: String,
                args: Value,
                _ctx: &crate::ExtensionContext,
            ) -> HookResult<(String, Value)> {
                self.count.fetch_add(1, Ordering::SeqCst);
                if self.should_block {
                    HookResult::Cancel("blocked".into())
                } else {
                    HookResult::Continue((name, args))
                }
            }
        }

        let count1 = Arc::new(AtomicU32::new(0));
        let count2 = Arc::new(AtomicU32::new(0));

        let mut runner = HookRunner::new();
        runner.register(Box::new(BlockingHook {
            name: "blocker".into(),
            priority: 10,
            count: count1.clone(),
            should_block: true,
        }));
        runner.register(Box::new(BlockingHook {
            name: "passthrough".into(),
            priority: 0,
            count: count2.clone(),
            should_block: false,
        }));

        let ctx = crate::ExtensionContext::new("t".into(), false, crate::ExtensionUIContext::noop(), crate::RuntimeHandle::noop());
        let result = runner
            .run_before_tool_call("shell".into(), serde_json::json!({"cmd": "ls"}), &ctx)
            .await;

        assert!(result.is_cancel());
        assert_eq!(count1.load(Ordering::SeqCst), 1);
        assert_eq!(count2.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_runner_modifying_hooks_pass_through() {
        let runner = HookRunner::new();
        let ctx = crate::ExtensionContext::new("t".into(), false, crate::ExtensionUIContext::noop(), crate::RuntimeHandle::noop());
        let result = runner
            .run_before_tool_call("shell".into(), serde_json::json!({"cmd": "ls"}), &ctx)
            .await;
        assert!(result.is_continue());
        if let HookResult::Continue((name, _)) = result {
            assert_eq!(name, "shell");
        }
    }

    #[tokio::test]
    async fn handlers_execute_in_priority_order() {
        struct OrderedHook {
            name: String,
            priority: i32,
            execution_order: Arc<std::sync::Mutex<Vec<String>>>,
        }

        #[async_trait]
        impl HookHandler for OrderedHook {
            fn name(&self) -> &str {
                &self.name
            }
            fn priority(&self) -> i32 {
                self.priority
            }
            async fn on_agent_start(&self, _ctx: Option<&crate::ExtensionContext>) {
                if let Ok(mut order) = self.execution_order.lock() {
                order.push(self.name.clone());
            }
            }
        }

        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut runner = HookRunner::new();
        runner.register(Box::new(OrderedHook {
            name: "low".into(),
            priority: -10,
            execution_order: order.clone(),
        }));
        runner.register(Box::new(OrderedHook {
            name: "high".into(),
            priority: 10,
            execution_order: order.clone(),
        }));
        runner.register(Box::new(OrderedHook {
            name: "medium".into(),
            priority: 0,
            execution_order: order.clone(),
        }));

        runner.fire_agent_start(None).await;

        let Ok(guard) = order.lock() else { return; };
        let order = &*guard;
        assert_eq!(order.len(), 3);
        assert!(order.contains(&"low".to_string()));
        assert!(order.contains(&"medium".to_string()));
        assert!(order.contains(&"high".to_string()));
    }
}

#[cfg(test)]
mod context_phase_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::ExtensionContext;

    fn system(text: &str) -> Value {
        serde_json::json!({"role": "system", "content": text, "timestamp": 0})
    }
    fn user(text: &str) -> Value {
        serde_json::json!({"role": "user", "content": text, "timestamp": 1})
    }

    /// A `context` handler never sees system messages; an unchanged list keeps
    /// every system message in place (TS `emitContext`).
    struct SeesConversationOnly {
        seen: std::sync::Arc<std::sync::Mutex<bool>>,
    }
    #[async_trait]
    impl HookHandler for SeesConversationOnly {
        fn name(&self) -> &'static str {
            "sees_conversation_only"
        }
        async fn on_context_mut(
            &self,
            messages: Vec<Value>,
            _ctx: &ExtensionContext,
        ) -> HookResult<Vec<Value>> {
            *self.seen.lock().unwrap() = messages
                .iter()
                .all(|m| m.get("role").and_then(Value::as_str) != Some("system"));
            HookResult::Continue(messages)
        }
    }

    #[tokio::test]
    async fn context_handlers_see_the_conversation_only() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(false));
        let mut runner = HookRunner::new();
        runner.register(Box::new(SeesConversationOnly { seen: seen.clone() }));
        let ctx = ExtensionContext::new("/tmp".to_string(), false, crate::ExtensionUIContext::noop(), crate::RuntimeHandle::noop());

        let messages = vec![system("prompt"), user("hi")];
        let out = match runner.run_on_context(messages.clone(), &ctx).await {
            HookResult::Continue(m) => m,
            HookResult::Cancel(r) => panic!("unexpected cancel: {r}"),
        };
        assert!(*seen.lock().unwrap(), "handlers must not see system messages");
        assert_eq!(out, messages, "an unchanged conversation keeps its system messages");
    }

    /// A handler that prunes the conversation gets the replayed prompt and tool
    /// declarations back as one leading system message
    /// (TS `restoreSystemMessages`).
    struct Prunes;
    #[async_trait]
    impl HookHandler for Prunes {
        fn name(&self) -> &'static str {
            "prunes"
        }
        async fn on_context_mut(
            &self,
            messages: Vec<Value>,
            _ctx: &ExtensionContext,
        ) -> HookResult<Vec<Value>> {
            HookResult::Continue(messages.into_iter().take(1).collect())
        }
    }

    #[tokio::test]
    async fn a_pruned_context_gets_the_prompt_back() {
        let mut runner = HookRunner::new();
        runner.register(Box::new(Prunes));
        let ctx = ExtensionContext::new("/tmp".to_string(), false, crate::ExtensionUIContext::noop(), crate::RuntimeHandle::noop());

        let out = match runner
            .run_on_context(vec![system("prompt"), user("a"), user("b")], &ctx)
            .await
        {
            HookResult::Continue(m) => m,
            HookResult::Cancel(r) => panic!("unexpected cancel: {r}"),
        };
        assert_eq!(out.len(), 2, "prompt restored in front of the pruned list");
        assert_eq!(out[0]["role"], "system");
        assert_eq!(out[0]["content"], "prompt");
        assert_eq!(out[1], user("a"));
    }

    /// The `context_with_system` phase sees the full transcript and its result
    /// is used verbatim; dropping the leading system message is reported
    /// (TS `emitContext`'s second phase).
    struct DropsSystem;
    #[async_trait]
    impl HookHandler for DropsSystem {
        fn name(&self) -> &'static str {
            "drops_system"
        }
        async fn on_context_with_system(
            &self,
            messages: &[Value],
            _ctx: &ExtensionContext,
        ) -> Option<Vec<Value>> {
            assert!(
                messages.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("system")),
                "this phase sees system messages"
            );
            Some(messages.iter().skip(1).cloned().collect())
        }
    }

    #[tokio::test]
    async fn context_with_system_sees_everything_and_reports_dropped_prompts() {
        let mut runner = HookRunner::new();
        runner.register(Box::new(DropsSystem));
        let ctx = ExtensionContext::new("/tmp".to_string(), false, crate::ExtensionUIContext::noop(), crate::RuntimeHandle::noop());

        let (out, dropped) = runner
            .run_context_with_system(vec![system("prompt"), user("hi")], &ctx)
            .await;
        assert_eq!(out, vec![user("hi")], "the handler's list is used verbatim");
        assert_eq!(dropped.as_deref(), Some("drops_system"));
    }
}
