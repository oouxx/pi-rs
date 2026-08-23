//! goal 扩展测试：命令解析、状态机、三工具验证链、自动延续、会话内持久化。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)]

use std::sync::{Arc, Mutex};

use super::*;
use pi_extension_api::{ExtensionContext, ExtensionUIContext, RuntimeHandle};

// ============================================================================
// 测试辅助
// ============================================================================

/// 构造捕获 runtime 调用的测试上下文。
struct CtxHarness {
    ctx: ExtensionContext,
    queued: Arc<Mutex<Vec<String>>>,
    entries: Arc<Mutex<Vec<Value>>>,
}

impl CtxHarness {
    fn new() -> Self {
        let queued: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let entries: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let mut handle = RuntimeHandle::noop();
        handle.queue_follow_up = {
            let queued = queued.clone();
            Arc::new(move |msg: String| queued.lock().unwrap().push(msg))
        };
        handle.append_custom_entry = {
            let entries = entries.clone();
            Arc::new(move |t: &str, data: Option<Value>| {
                entries.lock().unwrap().push(json!({ "customType": t, "data": data }));
            })
        };
        handle.get_custom_entries = {
            let entries = entries.clone();
            Arc::new(move || entries.lock().unwrap().clone())
        };
        handle.get_agent_dir = Arc::new(|| "/tmp/goal-test".to_string());
        let ui = ExtensionUIContext::noop();
        let ctx = ExtensionContext::new("test-session".into(), false, ui, handle);
        Self { ctx, queued, entries }
    }
}

fn active_goal() -> GoalState {
    GoalState::create("fix the bug".into(), None)
}

/// 构造带 stop_reason/usage 的 assistant 消息（fire_agent_end 的 messages 形状）。
fn assistant_msg(stop_reason: &str, error: Option<&str>, usage: u64) -> Value {
    let mut msg = json!({
        "role": "assistant",
        "stop_reason": stop_reason,
        "content": [{"type": "text", "text": "work in progress"}],
        "usage": {
            "input": 0,
            "output": usage,
            "total_tokens": usage,
            "cost": {"total": 0.0},
        },
    });
    if let Some(e) = error {
        msg["error_message"] = json!(e);
    }
    msg
}

// ============================================================================
// 命令解析
// ============================================================================

#[test]
fn test_parse_goal_command_variants() {
    assert_eq!(parse_goal_command(""), Ok(GoalCommand::Show));
    assert_eq!(parse_goal_command("status"), Ok(GoalCommand::Show));
    assert_eq!(parse_goal_command("pause"), Ok(GoalCommand::Pause));
    assert_eq!(parse_goal_command("resume"), Ok(GoalCommand::Resume));
    assert_eq!(parse_goal_command("clear"), Ok(GoalCommand::Clear));
    assert_eq!(parse_goal_command("stop"), Ok(GoalCommand::Clear));
    assert_eq!(
        parse_goal_command("edit fix the lints"),
        Ok(GoalCommand::Edit { objective: "fix the lints".into(), token_budget: None })
    );
    assert_eq!(
        parse_goal_command("--tokens 100k add tests"),
        Ok(GoalCommand::Start { objective: "add tests".into(), token_budget: Some(100_000) })
    );
    assert!(parse_goal_command("--tokens 5x add tests").is_err());
    assert!(parse_goal_command("pause extra").is_err());
    // 引号分词
    assert_eq!(
        parse_goal_command(r#"edit "two words" goal"#),
        Ok(GoalCommand::Edit { objective: "two words goal".into(), token_budget: None })
    );
}

#[test]
fn test_parse_token_budget() {
    assert_eq!(parse_token_budget("100k"), Some(100_000));
    assert_eq!(parse_token_budget("1m"), Some(1_000_000));
    assert_eq!(parse_token_budget("500"), Some(500));
    assert_eq!(parse_token_budget("0"), None);
    assert_eq!(parse_token_budget("abc"), None);
    assert_eq!(parse_token_budget("-5k"), None);
}

// ============================================================================
// 状态机 / 工具函数
// ============================================================================

#[test]
fn test_transition_budget_limit() {
    let goal = GoalState {
        tokens_used: 10_000,
        token_budget: Some(10_000),
        ..GoalState::create("x".into(), Some(10_000))
    };
    // 请求 active 但预算耗尽 → budget_limited
    let next = goal.transition(GoalStatus::Active, 0);
    assert_eq!(next.status, GoalStatus::BudgetLimited);
    assert!(next.waiting.is_none());
}

#[test]
fn test_next_instance_rotates_guard_id() {
    let g = GoalState::create("x".into(), None);
    let next = g.clone().next_instance();
    assert_ne!(g.id, next.id);
}

#[test]
fn test_goal_id_rejection() {
    let g = GoalState::create("x".into(), None);
    assert_eq!(goal_id_rejection_reason(&g, ""), Some("missing goal_id".into()));
    assert_eq!(
        goal_id_rejection_reason(&g, &"a".repeat(MAX_GOAL_ID_LENGTH + 1)),
        Some("goal_id is too long".into())
    );
    assert_eq!(goal_id_rejection_reason(&g, "other-id"), Some("goal_id does not match the active goal".into()));
    assert_eq!(goal_id_rejection_reason(&g, &g.id), None);
}

#[test]
fn test_contradictory_summary_detection() {
    assert!(is_contradictory_summary("the goal is not complete"));
    assert!(is_contradictory_summary("tests still fail"));
    assert!(is_contradictory_summary("Not done yet"));
    assert!(!is_contradictory_summary("all requirements verified by tests passing"));
    assert!(!is_contradictory_summary("implemented and verified"));
}

#[test]
fn test_usage_limit_error_detection() {
    assert!(is_usage_limited_interruption("You've hit your usage limit"));
    assert!(is_usage_limited_interruption("Insufficient quota for this request"));
    assert!(is_usage_limited_interruption("out of credits"));
    assert!(!is_usage_limited_interruption("server error 500"));
}

#[test]
fn test_retryable_interruption_detection() {
    assert!(is_retryable_interruption("rate limit exceeded"));
    assert!(is_retryable_interruption("overloaded"));
    assert!(is_retryable_interruption("connection refused"));
    // usage limit 不属于 retryable
    assert!(!is_retryable_interruption("usage limit reached"));
    // 硬错误不 retryable
    assert!(!is_retryable_interruption("invalid api key"));
}

#[test]
fn test_fingerprint_stable_and_distinct() {
    let m1 = vec![assistant_msg("stop", None, 1)];
    let m2 = vec![assistant_msg("stop", None, 1)];
    let m3 = vec![json!({
        "role": "assistant",
        "stop_reason": "stop",
        "content": [{"type": "text", "text": "DIFFERENT work"}],
    })];
    let f1 = fingerprint_visible_output(&m1);
    let f2 = fingerprint_visible_output(&m2);
    let f3 = fingerprint_visible_output(&m3);
    assert_eq!(f1, f2, "identical output must fingerprint identically");
    assert_ne!(f1, f3, "different output must differ");
    // 工具轮 → 空指纹
    let tool_msg = vec![json!({
        "role": "assistant",
        "content": [{"type": "toolCall", "name": "bash"}],
    })];
    assert!(fingerprint_visible_output(&tool_msg).is_empty());
}

// ============================================================================
// 工具验证链
// ============================================================================

#[tokio::test]
async fn test_goal_complete_success() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    let output = ext
        .handle_tool_call(
            GOAL_COMPLETE_TOOL,
            json!({ "goal_id": goal.id, "summary": "all requirements implemented and verified" }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(!output.is_error);
    assert_eq!(output.terminate, Some(true));
    let text = output.content[0]["text"].as_str().unwrap();
    assert!(text.starts_with("Goal complete:"), "got: {text}");
    // 会话持久化完成状态
    assert_eq!(h.entries.lock().unwrap().len(), 1);
    let saved = &h.entries.lock().unwrap()[0];
    assert_eq!(saved["customType"], "goal-state");
    assert_eq!(saved["data"]["goal"]["status"], "complete");
    assert_eq!(saved["data"]["goal"]["completion_summary"], "all requirements implemented and verified");
    // 完成后扩展内部清空
    assert!(ext.goal_snapshot().is_none());
}

#[tokio::test]
async fn test_goal_complete_rejections() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);

    // 无 goal
    let none = GoalExtension::new();
    let out = none
        .handle_tool_call(GOAL_COMPLETE_TOOL, json!({ "goal_id": "x", "summary": "done" }), &h.ctx)
        .await
        .expect("handled");
    assert!(out.is_error);
    assert!(out.content[0]["text"].as_str().unwrap().contains("no active goal"));

    // stale goal_id
    let out = ext
        .handle_tool_call(GOAL_COMPLETE_TOOL, json!({ "goal_id": "wrong", "summary": "done" }), &h.ctx)
        .await
        .expect("handled");
    eprintln!("STALE DEBUG: {:?}", out);
    assert!(out.is_error);
    assert!(out.content[0]["text"].as_str().unwrap().contains("goal_id does not match"));

    // 矛盾 summary
    let out = ext
        .handle_tool_call(
            GOAL_COMPLETE_TOOL,
            json!({ "goal_id": goal.id, "summary": "the goal is not complete" }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(out.is_error);
    assert!(out.content[0]["text"].as_str().unwrap().contains("summary says the goal is not complete"));

    // 非 active
    let paused = GoalState {
        status: GoalStatus::Paused,
        ..active_goal()
    };
    let ext2 = set_goal(&paused);
    let out = ext2
        .handle_tool_call(
            GOAL_COMPLETE_TOOL,
            json!({ "goal_id": paused.id, "summary": "done" }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(out.is_error);
    assert!(out.content[0]["text"].as_str().unwrap().contains("not active"));
}

#[tokio::test]
async fn test_goal_blocked_requires_repeated_turns() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);

    // repeated_turns < 3 → 拒绝
    let out = ext
        .handle_tool_call(
            GOAL_BLOCKED_TOOL,
            json!({ "goal_id": goal.id, "reason": "needs approval", "evidence": "rejected twice", "repeated_turns": 2 }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(out.is_error);
    assert!(out.content[0]["text"].as_str().unwrap().contains("repeated_turns must be at least 3"));

    // 缺 reason
    let out = ext
        .handle_tool_call(
            GOAL_BLOCKED_TOOL,
            json!({ "goal_id": goal.id, "reason": "", "evidence": "x", "repeated_turns": 3 }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(out.is_error);
    assert!(out.content[0]["text"].as_str().unwrap().contains("reason is empty"));

    // 合法 → blocked
    let out = ext
        .handle_tool_call(
            GOAL_BLOCKED_TOOL,
            json!({ "goal_id": goal.id, "reason": "needs user approval", "evidence": "same rejection across 3 turns", "repeated_turns": 3 }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(!out.is_error);
    assert_eq!(out.terminate, Some(true));
    assert_eq!(ext.goal_snapshot().unwrap().status, GoalStatus::Blocked);
}

#[tokio::test]
async fn test_goal_wait_and_clamp() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);

    // 低于 MIN → 钳制
    let out = ext
        .handle_tool_call(
            GOAL_WAIT_TOOL,
            json!({ "goal_id": goal.id, "reason": "awaiting review", "resume_after_ms": 1 }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(!out.is_error);
    let text = out.content[0]["text"].as_str().unwrap();
    assert!(text.contains("clamped to 10000"), "text: {text}");
    let saved = ext.goal_snapshot().unwrap();
    assert!(saved.is_active());
    let w = saved.waiting.as_ref().expect("waiting");
    assert_eq!(w.requested_ms, Some(1));
    assert!(w.resume_at.is_some());

    // 已在 waiting → 拒绝
    let out2 = ext
        .handle_tool_call(
            GOAL_WAIT_TOOL,
            json!({ "goal_id": goal.id, "reason": "again" }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(out2.is_error);
    assert!(out2.content[0]["text"].as_str().unwrap().contains("already waiting"));
}

#[tokio::test]
async fn test_goal_tools_no_active_goal() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    for (tool, params) in [
        (GOAL_COMPLETE_TOOL, json!({ "goal_id": "x", "summary": "d" })),
        (GOAL_BLOCKED_TOOL, json!({ "goal_id": "x", "reason": "r", "evidence": "e", "repeated_turns": 3 })),
        (GOAL_WAIT_TOOL, json!({ "goal_id": "x", "reason": "r" })),
    ] {
        let out = ext.handle_tool_call(tool, params, &h.ctx).await.expect("handled");
        assert!(out.is_error, "{tool} must reject without a goal");
    }
}

// ===========================================================================
// 自动延续状态机
// ===========================================================================

fn begin_goal_run(ext: &GoalExtension, goal_id: &str, origin: RunOrigin) {
    *ext.inner.pending_run.lock().unwrap() = Some(PendingRun {
        goal_id: Some(goal_id.to_string()),
        origin: Some(origin),
    });
    ext.inner.begin_agent_run();
}

fn set_goal(goal: &GoalState) -> GoalExtension {
    let ext = GoalExtension::new();
    ext.set_goal_for_test(Some(goal.clone()));
    ext
}

#[tokio::test]
async fn test_agent_end_normal_requests_continuation() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    // 标记 run 归属（模拟 continuation 触发的 run）。
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);

    let msgs = vec![assistant_msg("stop", None, 100)];
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;

    // usage 累计
    assert_eq!(ext.goal_snapshot().unwrap().tokens_used, 100);
    // continuation 待发（agent_settled 时消费）
    ext.on_agent_settled(Some(&h.ctx)).await;
    assert_eq!(h.queued.lock().unwrap().len(), 1);
    let prompt = &h.queued.lock().unwrap()[0];
    assert!(prompt.contains("Continue the active /goal"), "prompt: {prompt}");
}

#[tokio::test]
async fn test_agent_end_error_blocks_goal() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);

    let msgs = vec![assistant_msg("error", Some("invalid api key"), 10)];
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;

    let state = ext.goal_snapshot().unwrap();
    assert_eq!(state.status, GoalStatus::Blocked);
    assert!(state.terminal_reason.is_some());
    // blocked 后不再延续
    ext.on_agent_settled(Some(&h.ctx)).await;
    assert_eq!(h.queued.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn test_agent_end_usage_limited() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);

    let msgs = vec![assistant_msg("error", Some("You've hit your usage limit"), 0)];
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;

    assert_eq!(ext.goal_snapshot().unwrap().status, GoalStatus::UsageLimited);
}

#[tokio::test]
async fn test_agent_end_aborted_pauses() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);

    let msgs = vec![assistant_msg("aborted", None, 0)];
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;
    assert_eq!(ext.goal_snapshot().unwrap().status, GoalStatus::Paused);
}

#[tokio::test]
async fn test_automatic_turn_limit_pauses() {
    let h = CtxHarness::new();
    let goal = GoalState {
        automatic_model_turns: 24,
        ..active_goal()
    };
    let ext = GoalExtension::new()
        .with_automatic_turns(Some(25));
    ext.set_goal_for_test(Some(goal.clone()));
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);

    let msgs = vec![assistant_msg("stop", None, 0)];
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;

    let state = ext.goal_snapshot().unwrap();
    assert_eq!(state.status, GoalStatus::Paused);
    assert_eq!(state.safety_pause_cause.as_deref(), Some("continuation_limit"));
    // 不再延续
    ext.on_agent_settled(Some(&h.ctx)).await;
    assert_eq!(h.queued.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn test_no_progress_repeat_detection() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);

    // 三轮相同输出（无工具）→ no_progress 暂停
    let msgs = vec![assistant_msg("stop", None, 0)];
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;
    assert_eq!(ext.goal_snapshot().unwrap().tool_free_repeat_count, 1);
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;
    assert_eq!(ext.goal_snapshot().unwrap().tool_free_repeat_count, 2);
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;

    let state = ext.goal_snapshot().unwrap();
    assert_eq!(state.status, GoalStatus::Paused);
    assert_eq!(state.safety_pause_cause.as_deref(), Some("no_progress"));
}

// ===========================================================================
// /goal 命令
// ===========================================================================

#[tokio::test]
async fn test_goal_command_start_and_persist() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    ext.inner.handle_command("fix the lints", Some(&h.ctx));

    let goal = ext.goal_snapshot().expect("goal started");
    assert_eq!(goal.text, "fix the lints");
    assert_eq!(goal.status, GoalStatus::Active);
    // 起始 prompt 已同步入队（由 settled 循环消费启动 run）
    assert_eq!(h.queued.lock().unwrap().len(), 1);
    let prompt = &h.queued.lock().unwrap()[0];
    assert!(prompt.contains("Goal mode is active"), "prompt: {prompt}");
    // 已持久化到会话
    assert_eq!(h.entries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_goal_command_pause_resume_clear() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    ext.inner.handle_command("start something", Some(&h.ctx));
    // pause
    ext.inner.handle_command("pause", Some(&h.ctx));
    assert_eq!(ext.goal_snapshot().unwrap().status, GoalStatus::Paused);

    // resume → 新 guard id + 发 resume prompt
    let old_id = ext.goal_snapshot().unwrap().id;
    ext.inner.handle_command("resume", Some(&h.ctx));
    let g = ext.goal_snapshot().unwrap();
    assert_eq!(g.status, GoalStatus::Active);
    assert_ne!(g.id, old_id, "resume must rotate the guard id");
    let last = h.queued.lock().unwrap().last().unwrap().clone();
    assert!(last.contains("explicitly resumed"), "prompt: {last}");

    // clear
    ext.inner.handle_command("clear", Some(&h.ctx));
    assert!(ext.goal_snapshot().is_none());
}

// ===========================================================================
// 会话持久化恢复
// ===========================================================================

#[tokio::test]
async fn test_load_goal_from_session_restores() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    // 会话里已有 goal-state entry（模拟重启恢复）。
    let goal = active_goal();
    h.entries.lock().unwrap().push(json!({
        "customType": "goal-state",
        "data": { "goal": goal },
    }));

    // 首次有 ctx 的事件触发恢复
    let out = ext
        .handle_tool_call(
            GOAL_COMPLETE_TOOL,
            json!({ "goal_id": goal.id, "summary": "restored and completed" }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(!out.is_error, "restored goal must be usable: {out:?}");
}

// ===========================================================================
// 设置（pi-goal.json）与 TUI 菜单
// ===========================================================================

/// 独立临时 agent 目录（每个测试唯一，避免并行互踩）。
fn temp_agent_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "pi-goal-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    dir.display().to_string()
}

/// 脚本化 UI：select/input/confirm 按队列依次返回答案，notify 记录消息。
struct MenuHarness {
    ctx: ExtensionContext,
    selects: Arc<Mutex<std::collections::VecDeque<Option<String>>>>,
    inputs: Arc<Mutex<std::collections::VecDeque<Option<String>>>>,
    confirms: Arc<Mutex<std::collections::VecDeque<bool>>>,
    notified: Arc<Mutex<Vec<String>>>,
}

impl MenuHarness {
    fn new(agent_dir: &str) -> Self {
        let selects: Arc<Mutex<std::collections::VecDeque<Option<String>>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let inputs: Arc<Mutex<std::collections::VecDeque<Option<String>>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let confirms: Arc<Mutex<std::collections::VecDeque<bool>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let notified: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut handle = RuntimeHandle::noop();
        let dir = agent_dir.to_string();
        handle.get_agent_dir = Arc::new(move || dir.clone());
        handle.queue_follow_up = Arc::new(|_| {});
        handle.append_custom_entry = Arc::new(|_, _| {});
        handle.get_custom_entries = Arc::new(Vec::new);
        let ui = ExtensionUIContext {
            notify: {
                let n = notified.clone();
                Arc::new(move |msg: &str, _| n.lock().unwrap().push(msg.to_string()))
            },
            set_status: Arc::new(|_, _| {}),
            confirm: {
                let c = confirms.clone();
                Arc::new(move |_, _| c.lock().unwrap().pop_front().unwrap_or(false))
            },
            select: {
                let s = selects.clone();
                Arc::new(move |_, _, _| {
                    let v = s.lock().unwrap().pop_front().unwrap_or(None);
                    Box::pin(async move { v })
                })
            },
            input: {
                let i = inputs.clone();
                Arc::new(move |_, _, _| {
                    let v = i.lock().unwrap().pop_front().unwrap_or(None);
                    Box::pin(async move { v })
                })
            },
            set_widget: Arc::new(|_, _, _| {}),
            set_title: Arc::new(|_| {}),
            set_editor_text: Arc::new(|_| {}),
        };
        let mut ctx = ExtensionContext::new("test-session".into(), false, ui, handle);
        ctx.set_mode("tui");
        Self { ctx, selects, inputs, confirms, notified }
    }

    fn select(&mut self, value: &str) {
        self.selects.lock().unwrap().push_back(Some(value.to_string()));
    }

    fn select_none(&mut self) {
        self.selects.lock().unwrap().push_back(None);
    }

    fn input(&mut self, value: &str) {
        self.inputs.lock().unwrap().push_back(Some(value.to_string()));
    }

    fn confirm(&mut self, value: bool) {
        self.confirms.lock().unwrap().push_back(value);
    }

    fn notified_contains(&self, needle: &str) -> bool {
        self.notified.lock().unwrap().iter().any(|m| m.contains(needle))
    }
}

/// 通过注册的 /goal 命令入口执行（对齐真实调用路径）。
async fn run_goal_command(ext: &GoalExtension, args: &str, ctx: &ExtensionContext) {
    let mut reg = pi_extension_api::CommandRegistry::new(
        pi_extension_api::create_builtin_source_info("goal-test"),
    );
    ext.register_commands(&mut reg);
    let cmd = reg
        .into_vec()
        .into_iter()
        .find(|c| c.name == "goal")
        .expect("goal command");
    (cmd.execute)(args.to_string(), Some(ctx)).await;
}

#[tokio::test]
async fn test_settings_default_and_roundtrip() {
    let dir = temp_agent_dir("settings-roundtrip");
    // 文件缺失 → 默认值。
    let defaults = read_goal_settings(&dir);
    assert_eq!(defaults.continuation_limits.automatic_turns, Some(25));
    assert_eq!(defaults.continuation_limits.no_progress_turns, Some(3));
    assert_eq!(defaults.tool_visibility, "after-first-goal");
    assert!(!defaults.rpc.enabled);

    // 保存 → 读回一致。
    let mut custom = defaults.clone();
    custom.continuation_limits.automatic_turns = None; // Unlimited
    custom.continuation_limits.no_progress_turns = Some(7);
    custom.tool_visibility = "always".to_string();
    custom.rpc.enabled = true;
    save_goal_settings(&dir, &custom).expect("save");
    let loaded = read_goal_settings(&dir);
    assert_eq!(loaded, custom, "round-trip must preserve settings");
    // 文件内容形状对齐原版（camelCase + null = unlimited）。
    let raw = std::fs::read_to_string(goal_settings_path(&dir)).expect("file");
    assert!(raw.contains("\"automaticTurns\": null"), "raw: {raw}");
    assert!(raw.contains("\"noProgressTurns\": 7"), "raw: {raw}");
    assert!(raw.contains("\"toolVisibility\": \"always\""), "raw: {raw}");
}

#[tokio::test]
async fn test_settings_invalid_falls_back_to_defaults() {
    let dir = temp_agent_dir("settings-invalid");
    std::fs::write(goal_settings_path(&dir), "{ not json !").expect("write");
    let settings = read_goal_settings(&dir);
    assert_eq!(settings.continuation_limits.automatic_turns, Some(25));
}

/// 设置文件里的 automaticTurns 必须驱动安全暂停（2 轮即停，而非默认 25）。
#[tokio::test]
async fn test_settings_automatic_turns_wire_into_safety_pause() {
    let dir = temp_agent_dir("settings-wired");
    let mut settings = GoalSettings::default();
    settings.continuation_limits.automatic_turns = Some(2);
    save_goal_settings(&dir, &settings).expect("save");

    let h = CtxHarness::new();
    // 让 ctx 的 agent_dir 指向测试目录。
    let mut ctx = h.ctx.clone();
    let dir2 = dir.clone();
    ctx.runtime.get_agent_dir = Arc::new(move || dir2.clone());
    let ext = GoalExtension::new();
    let goal = active_goal();
    ext.set_goal_for_test(Some(goal.clone()));

    // 第一轮 agent_end：1 次自动轮 → 不暂停。
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);
    let mut msg = assistant_msg("stop", None, 10);
    msg["content"] = json!([{"type": "text", "text": "progress"}]);
    ext.on_agent_end(&[msg.clone()], Some(&ctx)).await;
    assert_eq!(ext.goal_snapshot().unwrap().status, GoalStatus::Active);

    // 第二轮：2 次自动轮 → 达到上限 → 暂停（continuation_limit）。
    begin_goal_run(&ext, &goal.id, RunOrigin::Automatic);
    let mut msg2 = assistant_msg("stop", None, 10);
    msg2["content"] = json!([{"type": "text", "text": "more progress"}]);
    ext.on_agent_end(&[msg2], Some(&ctx)).await;
    let g = ext.goal_snapshot().unwrap();
    assert_eq!(g.status, GoalStatus::Paused, "2-turn limit from settings");
    assert_eq!(g.safety_pause_cause.as_deref(), Some("continuation_limit"));
}

/// TUI 模式：/goal（无参数）打开菜单；选择 Start a goal… → 输入目标 → 启动。
#[tokio::test]
async fn test_goal_menu_start_flow() {
    let dir = temp_agent_dir("menu-start");
    let mut h = MenuHarness::new(&dir);
    h.select("Start a goal…");
    h.input("fix the menu bug");
    h.select_none(); // 菜单循环下一次 select → Esc 关闭
    let ext = GoalExtension::new();
    run_goal_command(&ext, "", &h.ctx).await;

    let goal = ext.goal_snapshot().expect("goal started via menu");
    assert_eq!(goal.text, "fix the menu bug");
    assert_eq!(goal.status, GoalStatus::Active);
    // 菜单打开时通知了状态标题。
    assert!(h.notified_contains("Goal · No goal"), "menu state notified");
}

/// TUI 模式：菜单 Pause → Resume → Clear（带确认）。
#[tokio::test]
async fn test_goal_menu_pause_resume_clear_flow() {
    let dir = temp_agent_dir("menu-pause");
    let mut h = MenuHarness::new(&dir);
    let ext = GoalExtension::new();
    let goal = active_goal();
    ext.set_goal_for_test(Some(goal.clone()));

    h.select("Pause goal");
    h.select("Resume goal");
    h.select("Clear goal…");
    h.confirm(true);
    h.select_none();
    run_goal_command(&ext, "", &h.ctx).await;

    // Pause → Resume → Clear 全链路：最终无 goal。
    assert!(ext.goal_snapshot().is_none(), "cleared via menu");
    assert!(h.notified_contains("Goal cleared"), "clear notified");
}

/// TUI 模式：菜单 Settings… → Automatic-work limit → Unlimited → 落盘。
#[tokio::test]
async fn test_goal_menu_settings_flow() {
    let dir = temp_agent_dir("menu-settings");
    let mut h = MenuHarness::new(&dir);
    h.select("Settings…");
    h.select("Automatic-work limit: 25 responses");
    h.select("Unlimited");
    h.select("Back");
    h.select_none();
    let ext = GoalExtension::new();
    run_goal_command(&ext, "", &h.ctx).await;

    let settings = read_goal_settings(&dir);
    assert_eq!(
        settings.continuation_limits.automatic_turns,
        None,
        "Unlimited persisted"
    );
    assert!(h.notified_contains("Goal settings saved"), "save notified");
}

/// 模式门控：非 TUI 模式 /goal（无参数）走文本状态；TUI 模式走菜单。
#[tokio::test]
async fn test_goal_command_mode_gating() {
    let dir = temp_agent_dir("mode-gating");
    // 非 TUI：bare /goal → 文本状态（notify），不弹 select。
    let mut h = MenuHarness::new(&dir);
    h.ctx.set_mode("cli");
    let ext = GoalExtension::new();
    run_goal_command(&ext, "", &h.ctx).await;
    assert!(
        h.notified_contains("No active goal"),
        "cli mode shows text status"
    );
    assert!(h.selects.lock().unwrap().is_empty(), "no menu in cli mode");

    // TUI：bare /goal → 菜单（select 被调用）。
    let mut h2 = MenuHarness::new(&dir);
    h2.select_none();
    let ext2 = GoalExtension::new();
    run_goal_command(&ext2, "", &h2.ctx).await;
    assert!(
        !h2.selects.lock().unwrap().is_empty() || h2.notified_contains("Goal ·"),
        "tui mode opens the menu"
    );

    // TUI：/goal status 仍走文本子命令。
    let h3 = MenuHarness::new(&dir);
    let ext3 = GoalExtension::new();
    run_goal_command(&ext3, "status", &h3.ctx).await;
    assert!(
        h3.notified_contains("No active goal"),
        "status subcommand stays text"
    );
    assert!(h3.selects.lock().unwrap().is_empty(), "no menu for status");
}
