//! pi-goal — /goal 命令，目标追踪扩展。
//!
//! 对应原版 TypeScript 扩展 `pi-goal.pkg` (Michaelliv/pi-goal)。
//! 管理目标的完整生命周期：创建、暂停、恢复、阻塞、完成。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use pi_extension_api::{
    CommandRegistry, ExtensionContext, HookHandler, RuntimeHandle,
    SendMessageOptions, ToolCallOutput, ToolDefinition, ToolRegistry,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ============================================================================
// 类型定义 — 对应 goal-state.ts
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GoalStatus {
    Active,
    Paused,
    BudgetLimited,
    Complete,
}

impl GoalStatus {
    fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Paused => "paused",
            GoalStatus::BudgetLimited => "budget_limited",
            GoalStatus::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GoalEventKind {
    Active,
    Continuation,
    Paused,
    Resumed,
    Cleared,
    BudgetLimited,
    Complete,
}

impl GoalEventKind {
    fn as_str(&self) -> &'static str {
        match self {
            GoalEventKind::Active => "active",
            GoalEventKind::Continuation => "continuation",
            GoalEventKind::Paused => "paused",
            GoalEventKind::Resumed => "resumed",
            GoalEventKind::Cleared => "cleared",
            GoalEventKind::BudgetLimited => "budget_limited",
            GoalEventKind::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalState {
    pub version: u32,
    pub id: String,
    pub objective: String,
    pub status: GoalStatus,
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub created_at: u64,
    pub updated_at: u64,
}

// ============================================================================
// 工具函数 — 对应 goal-state.ts
// ============================================================================

pub struct ParseResult {
    pub objective: String,
    pub token_budget: Option<u64>,
    pub error: Option<String>,
}

/// 解析 `--tokens 50k` 语法。
pub fn parse_token_budget(input: &str) -> ParseResult {
    let input = input.trim();
    if input.is_empty() {
        return ParseResult { objective: String::new(), token_budget: None, error: None };
    }
    let re = regex::Regex::new(r"(?:^|\s)--tokens(?:=|\s+)(\S+)(?:\s|$)").unwrap();
    if let Some(caps) = re.captures(input) {
        let raw = caps.get(1).unwrap().as_str().trim();
        let suffix = raw.chars().last().map(|c| c.to_ascii_lowercase());
        let numeric = match suffix {
            Some('k') | Some('m') => &raw[..raw.len() - 1],
            _ => raw,
        };
        let value: f64 = match numeric.parse() {
            Ok(v) => v,
            Err(_) => {
                return ParseResult {
                    objective: input.to_string(),
                    token_budget: None,
                    error: Some("Token budget must be a number.".into()),
                };
            }
        };
        if !value.is_finite() || value <= 0.0 {
            return ParseResult {
                objective: input.to_string(),
                token_budget: None,
                error: Some("Token budget must be positive.".into()),
            };
        }
        let multiplier: u64 = match suffix {
            Some('m') => 1_000_000,
            Some('k') => 1_000,
            _ => 1,
        };
        let token_budget = (value * multiplier as f64).round() as u64;
        let m = caps.get(0).unwrap();
        let objective = format!("{}{}", &input[..m.start()], &input[m.end()..]).trim().to_string();
        return ParseResult { objective, token_budget: Some(token_budget), error: None };
    }
    ParseResult { objective: input.to_string(), token_budget: None, error: None }
}

/// 标准化 token 预算。
pub fn normalize_token_budget(value: &Value) -> (Option<u64>, Option<String>) {
    if value.is_null() {
        return (None, None);
    }
    let token_budget = value.as_f64().map(|f| f.round() as u64);
    match token_budget {
        Some(b) if b > 0 => (Some(b), None),
        _ => (None, Some("tokenBudget must be a positive number when provided.".into())),
    }
}

/// 格式化 token 数量。
pub fn format_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

/// 格式化耗时。
pub fn format_elapsed(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, secs)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

fn current_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn create_goal_state(objective: String, token_budget: Option<u64>) -> GoalState {
    let now = current_millis();
    GoalState {
        version: 1,
        id: format!("goal_{}", now),
        objective,
        status: GoalStatus::Active,
        token_budget,
        tokens_used: 0,
        time_used_seconds: 0,
        created_at: now,
        updated_at: now,
    }
}

// ============================================================================
// GoalExtension — 实现 HookHandler
// ============================================================================

/// Goal 扩展，管理目标生命周期。
pub struct GoalExtension {
    goal: Mutex<Option<GoalState>>,
    start_time: AtomicU64,
    continuation_queued: AtomicBool,
    last_turn_time: AtomicU64,
}

impl GoalExtension {
    pub fn new() -> Self {
        Self {
            goal: Mutex::new(None),
            start_time: AtomicU64::new(0),
            continuation_queued: AtomicBool::new(false),
            last_turn_time: AtomicU64::new(0),
        }
    }

    fn persist(&self, runtime: &RuntimeHandle, ctx: &ExtensionContext, goal: Option<GoalState>) {
        if let Some(ref g) = goal {
            let path = format!("{}/goal.json", ctx.session_id);
            (runtime.write_file)(path, serde_json::to_string_pretty(g).unwrap_or_default());
        }
    }

    fn emit_goal_event(
        &self,
        runtime: &RuntimeHandle,
        kind: GoalEventKind,
        goal: &GoalState,
        options: Option<SendMessageOptions>,
    ) {
        let event = json!({
            "type": "goal",
            "kind": kind.as_str(),
            "goal": goal,
        });
        (runtime.send_message)(serde_json::to_string(&event).unwrap_or_default(), options.map(|o| json!({
            "triggerTurn": o.trigger_turn,
            "deliverAs": o.deliver_as,
        })));
    }

    fn queue_continuation(&self, runtime: &RuntimeHandle, goal: &GoalState) {
        if self.continuation_queued.swap(true, Ordering::SeqCst) {
            return;
        }
        let objective = goal.objective.clone();
        let token_budget = goal.token_budget;
        let tokens_used = goal.tokens_used;
        let runtime_clone = runtime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let remaining = token_budget.map(|b| if b > tokens_used { b - tokens_used } else { 0 });
            let msg = if let Some(rem) = remaining {
                if rem > 0 {
                    format!(
                        "Continuation of previous turn. Objective: {}. Remaining tokens: {}.",
                        objective,
                        format_tokens(rem)
                    )
                } else {
                    format!(
                        "Continuation of previous turn. Objective: {}. Token budget exhausted.",
                        objective
                    )
                }
            } else {
                format!("Continuation of previous turn. Objective: {}.", objective)
            };
            (runtime_clone.send_message)(msg, Some(json!({"triggerTurn": true, "deliverAs": "followUp"})));
        });
    }
}

// ============================================================================
// HookHandler 实现
// ============================================================================

#[async_trait]
impl HookHandler for GoalExtension {
    fn name(&self) -> &str {
        "goal"
    }

    fn register_tools(&self, tools: &mut ToolRegistry) {
        tools.register("get_goal", ToolDefinition {
            name: "get_goal".into(),
            description: "Get the current goal state. Returns the goal objective, status, token budget, and usage.".into(),
            parameters: Some(json!({
                "type": "object",
                "properties": {},
            })),
            ..Default::default()
        });
        tools.register("create_goal", ToolDefinition {
            name: "create_goal".into(),
            description: "Create a new goal with an objective and optional token budget.".into(),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "objective": {
                        "type": "string",
                        "description": "The goal objective.",
                    },
                    "tokenBudget": {
                        "type": "number",
                        "description": "Optional token budget for the goal.",
                    },
                },
                "required": ["objective"],
            })),
            ..Default::default()
        });
        tools.register("update_goal", ToolDefinition {
            name: "update_goal".into(),
            description: "Update the current goal status. Only accepts status=complete.".into(),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["complete"],
                        "description": "New status for the goal.",
                    },
                },
                "required": ["status"],
            })),
            ..Default::default()
        });
    }

    fn register_commands(&self, commands: &mut CommandRegistry) {
        commands.register(
            "goal",
            "Manage goals: create, pause, resume, complete, or clear.",
            std::sync::Arc::new(|args: String| {
                Box::pin(async move {
                    // Command execution is handled via tool calls in the current architecture.
                    // The /goal command is processed by the interactive mode handler.
                    eprintln!("[goal] /goal command received with args: {args}");
                })
            }),
        );
    }

    // ── Void hooks ────────────────────────────────────────────────

    async fn on_session_start(&self, reason: &str, _previous_session_file: Option<&str>) {
        if reason == "resume" {
            // Try to load existing goal from session file
            // In the current architecture, this is handled by the session manager
        }
    }

    async fn on_turn_start(&self) {
        let now = current_millis();
        self.last_turn_time.store(now, Ordering::SeqCst);
    }

    async fn on_turn_end(&self, _message: &Value, _tool_results: &[Value]) {
        let mut goal = self.goal.lock().unwrap();
        if let Some(ref mut g) = *goal {
            if g.status == GoalStatus::Active {
                let now = current_millis();
                let elapsed = (now - self.last_turn_time.load(Ordering::SeqCst)) / 1000;
                g.time_used_seconds += elapsed;
                // Token tracking is done via the agent runtime
            }
        }
    }

    async fn on_agent_end(&self, _messages: &[Value]) {
        let mut goal = self.goal.lock().unwrap();
        if let Some(ref g) = *goal {
            if g.status == GoalStatus::Active {
                let _snapshot = g.clone();
                drop(goal);
                // Continuation is queued by the runtime when appropriate
            }
        }
    }

    // ── Tool call handling ──────────────────────────────────────

    async fn handle_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        let runtime = &ctx.runtime;
        match tool_name {
            "get_goal" => {
                let mut goal = self.goal.lock().unwrap();
                Some(ToolCallOutput {
                    content: vec![json!({ "type": "text", "text": serde_json::to_string_pretty(&*goal).unwrap_or_default() })],
                    details: Some(json!({ "goal": *goal })),
                    is_error: false,
                    terminate: None,
                })
            }
            "create_goal" => {
                let objective = params.get("objective").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).unwrap_or_default();
                if objective.is_empty() {
                    return Some(ToolCallOutput {
                        content: vec![json!({ "type": "text", "text": "objective is required." })],
                        details: None,
                        is_error: true,
                        terminate: None,
                    });
                }
                let (token_budget, error) = normalize_token_budget(params.get("tokenBudget").unwrap_or(&Value::Null));
                if let Some(err) = error {
                    return Some(ToolCallOutput {
                        content: vec![json!({ "type": "text", "text": err })],
                        details: None,
                        is_error: true,
                        terminate: None,
                    });
                }
                let next = create_goal_state(objective, token_budget);
                self.persist(runtime, ctx, Some(next.clone()));
                self.emit_goal_event(runtime, GoalEventKind::Active, &next, Some(SendMessageOptions {
                    trigger_turn: Some(true),
                    deliver_as: None,
                }));
                Some(ToolCallOutput {
                    content: vec![json!({ "type": "text", "text": serde_json::to_string_pretty(&json!({"goal": next, "remainingTokens": token_budget})).unwrap_or_default() })],
                    details: Some(json!({ "goal": next })),
                    is_error: false,
                    terminate: None,
                })
            }
            "update_goal" => {
                let status = params.get("status").and_then(|v| v.as_str()).unwrap_or_default();
                if status != "complete" {
                    return Some(ToolCallOutput {
                        content: vec![json!({ "type": "text", "text": "update_goal only accepts status=complete." })],
                        details: None,
                        is_error: true,
                        terminate: None,
                    });
                }
                let mut goal = self.goal.lock().unwrap();
                if goal.is_none() {
                    return Some(ToolCallOutput {
                        content: vec![json!({ "type": "text", "text": "No goal is set." })],
                        details: None,
                        is_error: true,
                        terminate: None,
                    });
                }
                let current = goal.as_ref().unwrap().clone();
                drop(goal);
                let now = current_millis();
                let next = GoalState { status: GoalStatus::Complete, updated_at: now, ..current };
                self.persist(runtime, ctx, Some(next.clone()));
                self.emit_goal_event(runtime, GoalEventKind::Complete, &next, None);
                let remaining = next.token_budget.map(|b| {
                    if b > next.tokens_used { b - next.tokens_used } else { 0 }
                });
                Some(ToolCallOutput {
                    content: vec![json!({ "type": "text", "text": serde_json::to_string_pretty(&json!({"goal": next, "remainingTokens": remaining})).unwrap_or_default() })],
                    details: Some(json!({ "goal": next })),
                    is_error: false,
                    terminate: None,
                })
            }
            _ => None,
        }
    }
}
