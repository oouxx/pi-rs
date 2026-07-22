//! Hook 系统 — 参考 ZeroClaw 的 HookHandler + HookRunner 设计。
//!
//! 两种事件类型：
//! - **Void hooks**（并行、fire-and-forget）：通知型事件，所有 handler 同时执行
//! - **Modifying hooks**（按 priority 顺序执行，可 Cancel）：拦截型事件，
//!   高 priority 先执行，返回 `HookResult::Cancel` 时后续 handler 不执行
//!
//! 所有方法都有默认空实现，扩展只需实现关心的。

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{ExtensionContext, ToolCallOutput, ToolDefinition};

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
    pub fn is_cancel(&self) -> bool {
        matches!(self, HookResult::Cancel(_))
    }

    pub fn is_continue(&self) -> bool {
        matches!(self, HookResult::Continue(_))
    }
}

// ============================================================================
// Tool/Command/Shortcut registries (moved from lib.rs for co-location)
// ============================================================================

/// A tool registered by an extension.
#[derive(Debug, Clone)]
pub struct RegisteredTool {
    pub name: String,
    pub definition: ToolDefinition,
}

/// A command registered by an extension.
#[derive(Clone)]
/** A command registered by an extension. */
pub struct RegisteredCommand {
    pub name: String,
    pub description: String,
    pub execute: Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
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
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, name: &str, definition: ToolDefinition) {
        self.tools.push(RegisteredTool {
            name: name.to_string(),
            definition,
        });
    }

    pub fn into_vec(self) -> Vec<RegisteredTool> {
        self.tools
    }
}

/// Registry for collecting commands from extensions.
#[derive(Default)]
pub struct CommandRegistry {
    commands: Vec<RegisteredCommand>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }

    pub fn register(
        &mut self,
        name: &str,
        description: &str,
        execute: Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
    ) {
        self.commands.push(RegisteredCommand {
            name: name.to_string(),
            description: description.to_string(),
            execute,
        });
    }

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
    pub fn new() -> Self {
        Self { shortcuts: Vec::new() }
    }

    pub fn register(&mut self, key: &str, description: &str) {
        self.shortcuts.push(RegisteredShortcut {
            key: key.to_string(),
            description: description.to_string(),
        });
    }

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
    pub fn new() -> Self {
        Self { flags: Vec::new() }
    }

    pub fn register(&mut self, name: &str, description: &str) {
        self.flags.push(RegisteredFlag {
            name: name.to_string(),
            description: description.to_string(),
        });
    }

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
    async fn on_session_start(&self, _reason: &str, _previous_session_file: Option<&str>) {}

    /// Session 关闭时触发。
    async fn on_session_shutdown(&self, _reason: &str, _target_session_file: Option<&str>) {}

    /// Session 信息变更时触发。
    async fn on_session_info_changed(&self, _name: Option<&str>) {}

    /// Agent 开始处理时触发。
    async fn on_agent_start(&self) {}

    /// Agent 处理完成时触发。
    async fn on_agent_end(&self, _messages: &[Value]) {}

    /// Agent 进入空闲状态时触发。
    async fn on_agent_settled(&self) {}

    /// Turn 开始时触发。
    async fn on_turn_start(&self) {}

    /// Turn 结束时触发。
    async fn on_turn_end(&self, _message: &Value, _tool_results: &[Value]) {}

    /// 消息开始时触发。
    async fn on_message_start(&self, _message: &Value) {}

    /// 消息更新时触发。
    async fn on_message_update(&self, _message: &Value) {}

    /// 消息结束时触发。
    async fn on_message_end(&self, _message: &Value) {}

    /// 工具执行开始时触发。
    async fn on_tool_execution_start(&self, _tool_call_id: &str, _tool_name: &str, _args: &Value) {}

    /// 工具执行结束时触发。
    async fn on_tool_execution_end(
        &self,
        _tool_call_id: &str,
        _tool_name: &str,
        _result: &Value,
        _is_error: bool,
    ) {
    }

    /// 模型切换时触发。
    async fn on_model_select(&self, _model: &str, _previous_model: Option<&str>) {}

    /// Thinking level 切换时触发。
    async fn on_thinking_level_select(&self, _level: &str, _previous_level: &str) {}

    /// 压缩完成时触发。
    async fn on_compact(&self, _summary: &str, _tokens_before: u64) {}

    /// 树导航完成时触发。
    async fn on_tree(&self, _new_leaf_id: Option<&str>, _old_leaf_id: Option<&str>) {}

    /// 资源发现时触发。
    async fn on_resources_discover(&self, _cwd: &str, _reason: &str) {}

    /// Project trust 变更时触发。
    async fn on_project_trust(&self, _cwd: &str) {}

    /// 上下文消息更新时触发。
    async fn on_context(&self, _messages: &[Value]) {}

    /// 用户 bash 执行时触发。
    async fn on_user_bash(&self, _command: &str, _cwd: &str) {}

    // ── Modifying hooks（按 priority 顺序执行，可 Cancel） ──────────

    /// 工具调用前触发。可修改工具名称和参数，或取消调用。
    async fn before_tool_call(
        &self,
        _tool_name: String,
        _args: Value,
    ) -> HookResult<(String, Value)> {
        HookResult::Continue((_tool_name, _args))
    }

    /// 工具调用后触发。可修改结果。
    async fn after_tool_call(
        &self,
        _tool_name: &str,
        _result: &Value,
        _is_error: bool,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Agent 开始前触发。可修改 prompt 和 system_prompt，或取消。
    async fn before_agent_start(
        &self,
        _prompt: String,
        _system_prompt: String,
    ) -> HookResult<(String, String)> {
        HookResult::Continue((_prompt, _system_prompt))
    }

    /// 用户输入时触发。可修改输入文本，或取消。
    async fn on_input(&self, _text: String, _source: String) -> HookResult<String> {
        HookResult::Continue(_text)
    }

    /// Provider 请求前触发。可修改 payload。
    async fn before_provider_request(&self, _payload: &Value) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Provider 请求头前触发。可修改 headers。
    async fn before_provider_headers(
        &self,
        _headers: HashMap<String, String>,
    ) -> HookResult<HashMap<String, String>> {
        HookResult::Continue(_headers)
    }

    /// Provider 响应后触发。可修改响应。
    async fn after_provider_response(
        &self,
        _status: u16,
        _headers: HashMap<String, String>,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session 切换前触发。可取消切换。
    async fn before_session_switch(
        &self,
        _reason: String,
        _target_session_file: Option<String>,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session fork 前触发。可取消 fork。
    async fn before_session_fork(
        &self,
        _entry_id: String,
        _position: String,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session 压缩前触发。可取消压缩。
    async fn before_session_compact(
        &self,
        _reason: String,
        _will_retry: bool,
    ) -> HookResult<()> {
        HookResult::Continue(())
    }

    /// Session 树导航前触发。可取消导航。
    async fn before_session_tree(&self, _target_id: String) -> HookResult<()> {
        HookResult::Continue(())
    }
}

// ============================================================================
// HookRunner
// ============================================================================

/// 管理所有注册的 HookHandler，按 priority 排序分发事件。
pub struct HookRunner {
    handlers: Vec<Box<dyn HookHandler>>,
}

impl HookRunner {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Register a hook handler. Maintains priority-sorted order.
    pub fn register(&mut self, handler: Box<dyn HookHandler>) {
        self.handlers.push(handler);
        self.handlers.sort_by(|a, b| b.priority().cmp(&a.priority()));
    }

    /// Check if any handlers are registered.
    pub fn has_handlers(&self) -> bool {
        !self.handlers.is_empty()
    }

    /// Number of registered handlers.
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// Access handlers (for tool/command/shortcut collection).
    pub fn handlers(&self) -> &[Box<dyn HookHandler>] {
        &self.handlers
    }

    // ── Void hook dispatch (parallel, fire-and-forget) ──────────────

    /// Fire `on_session_start` to all handlers (parallel).
    pub async fn fire_session_start(&self, reason: &str, previous_session_file: Option<&str>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_session_start(reason, previous_session_file))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_session_shutdown` to all handlers (parallel).
    pub async fn fire_session_shutdown(&self, reason: &str, target_session_file: Option<&str>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_session_shutdown(reason, target_session_file))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_session_info_changed` to all handlers (parallel).
    pub async fn fire_session_info_changed(&self, name: Option<&str>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_session_info_changed(name))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_agent_start` to all handlers (parallel).
    pub async fn fire_agent_start(&self) {
        let futures: Vec<_> = self.handlers.iter().map(|h| h.on_agent_start()).collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_agent_end` to all handlers (parallel).
    pub async fn fire_agent_end(&self, messages: &[Value]) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_agent_end(messages))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_agent_settled` to all handlers (parallel).
    pub async fn fire_agent_settled(&self) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_agent_settled())
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_turn_start` to all handlers (parallel).
    pub async fn fire_turn_start(&self) {
        let futures: Vec<_> = self.handlers.iter().map(|h| h.on_turn_start()).collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_turn_end` to all handlers (parallel).
    pub async fn fire_turn_end(&self, message: &Value, tool_results: &[Value]) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_turn_end(message, tool_results))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_message_start` to all handlers (parallel).
    pub async fn fire_message_start(&self, message: &Value) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_message_start(message))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_message_update` to all handlers (parallel).
    pub async fn fire_message_update(&self, message: &Value) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_message_update(message))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_message_end` to all handlers (parallel).
    pub async fn fire_message_end(&self, message: &Value) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_message_end(message))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_tool_execution_start` to all handlers (parallel).
    pub async fn fire_tool_execution_start(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        args: &Value,
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_tool_execution_start(tool_call_id, tool_name, args))
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
    ) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_tool_execution_end(tool_call_id, tool_name, result, is_error))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_model_select` to all handlers (parallel).
    pub async fn fire_model_select(&self, model: &str, previous_model: Option<&str>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_model_select(model, previous_model))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_thinking_level_select` to all handlers (parallel).
    pub async fn fire_thinking_level_select(&self, level: &str, previous_level: &str) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_thinking_level_select(level, previous_level))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_compact` to all handlers (parallel).
    pub async fn fire_compact(&self, summary: &str, tokens_before: u64) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_compact(summary, tokens_before))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_tree` to all handlers (parallel).
    pub async fn fire_tree(&self, new_leaf_id: Option<&str>, old_leaf_id: Option<&str>) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_tree(new_leaf_id, old_leaf_id))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_resources_discover` to all handlers (parallel).
    pub async fn fire_resources_discover(&self, cwd: &str, reason: &str) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_resources_discover(cwd, reason))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_project_trust` to all handlers (parallel).
    pub async fn fire_project_trust(&self, cwd: &str) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_project_trust(cwd))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_context` to all handlers (parallel).
    pub async fn fire_context(&self, messages: &[Value]) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_context(messages))
            .collect();
        futures::future::join_all(futures).await;
    }

    /// Fire `on_user_bash` to all handlers (parallel).
    pub async fn fire_user_bash(&self, command: &str, cwd: &str) {
        let futures: Vec<_> = self
            .handlers
            .iter()
            .map(|h| h.on_user_bash(command, cwd))
            .collect();
        futures::future::join_all(futures).await;
    }

    // ── Modifying hook dispatch (sequential, priority-ordered, cancellable) ──

    /// Run `before_tool_call` handlers in priority order. Returns Cancel on first cancel.
    pub async fn run_before_tool_call(
        &self,
        tool_name: String,
        args: Value,
    ) -> HookResult<(String, Value)> {
        let mut current = (tool_name, args);
        for handler in &self.handlers {
            match handler.before_tool_call(current.0.clone(), current.1.clone()).await {
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
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler.after_tool_call(tool_name, result, is_error).await {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `before_agent_start` handlers in priority order.
    pub async fn run_before_agent_start(
        &self,
        prompt: String,
        system_prompt: String,
    ) -> HookResult<(String, String)> {
        let mut current = (prompt, system_prompt);
        for handler in &self.handlers {
            match handler
                .before_agent_start(current.0.clone(), current.1.clone())
                .await
            {
                HookResult::Continue((p, s)) => {
                    current = (p, s);
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Run `on_input` handlers in priority order.
    pub async fn run_on_input(&self, text: String, source: String) -> HookResult<String> {
        let mut current = text;
        for handler in &self.handlers {
            match handler.on_input(current, source.clone()).await {
                HookResult::Continue(t) => {
                    current = t;
                }
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(current)
    }

    /// Run `before_provider_request` handlers in priority order.
    pub async fn run_before_provider_request(&self, payload: &Value) -> HookResult<()> {
        for handler in &self.handlers {
            match handler.before_provider_request(payload).await {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `before_provider_headers` handlers in priority order.
    pub async fn run_before_provider_headers(
        &self,
        headers: HashMap<String, String>,
    ) -> HookResult<HashMap<String, String>> {
        let mut current = headers;
        for handler in &self.handlers {
            match handler.before_provider_headers(current).await {
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
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler.after_provider_response(status, headers.clone()).await {
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
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .before_session_switch(reason.clone(), target_session_file.clone())
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
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .before_session_fork(entry_id.clone(), position.clone())
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
    ) -> HookResult<()> {
        for handler in &self.handlers {
            match handler
                .before_session_compact(reason.clone(), will_retry)
                .await
            {
                HookResult::Continue(()) => {}
                HookResult::Cancel(reason) => return HookResult::Cancel(reason),
            }
        }
        HookResult::Continue(())
    }

    /// Run `before_session_tree` handlers in priority order.
    pub async fn run_before_session_tree(&self, target_id: &str) -> HookResult<()> {
        for handler in &self.handlers {
            match handler.before_session_tree(target_id.to_string()).await {
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
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn void_hooks_fire_to_all_handlers() {
        let count = Arc::new(AtomicU32::new(0));

        struct CountingHook {
            name: String,
            count: Arc<AtomicU32>,
        }

        #[async_trait]
        impl HookHandler for CountingHook {
            fn name(&self) -> &str {
                &self.name
            }
            async fn on_agent_start(&self) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let mut runner = HookRunner::new();
        runner.register(Box::new(CountingHook {
            name: "hook1".into(),
            count: count.clone(),
        }));
        runner.register(Box::new(CountingHook {
            name: "hook2".into(),
            count: count.clone(),
        }));

        runner.fire_agent_start().await;
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
            async fn before_tool_call(&self, name: String, args: Value) -> HookResult<(String, Value)> {
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

        let result = runner
            .run_before_tool_call("shell".into(), serde_json::json!({"cmd": "ls"}))
            .await;

        assert!(result.is_cancel());
        assert_eq!(count1.load(Ordering::SeqCst), 1);
        assert_eq!(count2.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_runner_void_hooks_are_noops() {
        let runner = HookRunner::new();
        runner.fire_agent_start().await;
        runner.fire_session_start("test", None).await;
    }

    #[tokio::test]
    async fn empty_runner_modifying_hooks_pass_through() {
        let runner = HookRunner::new();
        let result = runner
            .run_before_tool_call("shell".into(), serde_json::json!({"cmd": "ls"}))
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
            async fn on_agent_start(&self) {
                self.execution_order.lock().unwrap().push(self.name.clone());
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

        runner.fire_agent_start().await;

        let order = order.lock().unwrap();
        assert_eq!(order.len(), 3);
        assert!(order.contains(&"low".to_string()));
        assert!(order.contains(&"medium".to_string()));
        assert!(order.contains(&"high".to_string()));
    }
}
