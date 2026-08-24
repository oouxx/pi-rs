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
    aborted: Arc<AtomicBool>,
}

impl CtxHarness {
    fn new() -> Self {
        let queued: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let entries: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let aborted: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
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
        handle.abort = {
            let aborted = aborted.clone();
            Arc::new(move || aborted.store(true, Ordering::SeqCst))
        };
        let ui = ExtensionUIContext::noop();
        let ctx = ExtensionContext::new("test-session".into(), false, ui, handle);
        Self { ctx, queued, entries, aborted }
    }
}

fn active_goal() -> GoalState {
    GoalState::create("fix the bug".into())
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
        Ok(GoalCommand::Edit { objective: "fix the lints".into() })
    );
    assert!(parse_goal_command("pause extra").is_err());
    // 引号分词
    assert_eq!(
        parse_goal_command(r#"edit "two words" goal"#),
        Ok(GoalCommand::Edit { objective: "two words goal".into() })
    );
}

// ============================================================================
// 状态机 / 工具函数
// ============================================================================

#[test]
fn test_next_instance_rotates_guard_id() {
    let g = GoalState::create("x".into());
    let next = g.clone().next_instance();
    assert_ne!(g.id, next.id);
}

#[test]
fn test_goal_id_rejection() {
    let g = GoalState::create("x".into());
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
// 对齐修复回归测试（GOAL_TS_COMPARISON.md B/C 类）
// ===========================================================================

/// B2：resume 清零自动轮数/无进展计数/指纹/安全原因（否则 resume 后下一轮
/// agent_end 会立刻再次暂停）。
#[tokio::test]
async fn test_resume_resets_safety_epoch() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    let goal = GoalState {
        status: GoalStatus::Paused,
        tool_free_repeat_count: 3,
        last_tool_free_output_fingerprint: Some("abc".to_string()),
        safety_pause_cause: Some("no_progress".to_string()),
        ..active_goal()
    };
    ext.set_goal_for_test(Some(goal.clone()));
    ext.inner.handle_command("resume", Some(&h.ctx));
    let g = ext.goal_snapshot().unwrap();
    assert_eq!(g.status, GoalStatus::Active);
    assert_eq!(g.tool_free_repeat_count, 0);
    assert!(g.last_tool_free_output_fingerprint.is_none());
    assert!(g.safety_pause_cause.is_none());
    let prompt = &h.queued.lock().unwrap()[0];
    // C2：prompt 用真实 stoppedStatus。
    assert!(prompt.contains("paused /goal"), "prompt: {prompt}");
}

/// C3：/goal resume 唤醒 waiting goal（专用 prompt + 清 wait）。
#[tokio::test]
async fn test_resume_waiting_goal() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    let goal = GoalState {
        waiting: Some(GoalWaiting {
            reason: "awaiting review".into(),
            resume_at: Some(current_millis() + 60_000),
            requested_ms: None,
        }),
        ..active_goal()
    };
    ext.set_goal_for_test(Some(goal.clone()));
    ext.inner.handle_command("resume", Some(&h.ctx));
    let g = ext.goal_snapshot().unwrap();
    assert!(g.waiting.is_none(), "wait cleared");
    assert_eq!(g.status, GoalStatus::Active);
    let prompt = &h.queued.lock().unwrap()[0];
    assert!(
        prompt.contains("was waiting for an external event"),
        "waiting resume prompt: {prompt}"
    );
    assert!(prompt.contains("<goal_wait_reason>"), "prompt: {prompt}");
    assert!(prompt.contains("awaiting review"), "prompt: {prompt}");
}

/// C4：edit 保持非 active 状态（paused 编辑后仍 paused，不发 prompt）。
#[tokio::test]
async fn test_edit_keeps_paused_status() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    let goal = GoalState {
        status: GoalStatus::Paused,
        safety_pause_cause: Some("no_progress".to_string()),
        ..active_goal()
    };
    ext.set_goal_for_test(Some(goal.clone()));
    ext.inner.handle_command("edit new objective", Some(&h.ctx));
    let g = ext.goal_snapshot().unwrap();
    assert_eq!(g.status, GoalStatus::Paused, "edited paused goal stays paused");
    assert_eq!(g.text, "new objective");
    assert_eq!(h.queued.lock().unwrap().len(), 0, "no prompt for non-active edit");
    // 安全原因保留（原版 editGoal 只对 active 重置）。
    assert_eq!(g.safety_pause_cause.as_deref(), Some("no_progress"));
}

/// C5：clear 持久化 {goal: null} + 清状态栏 + 无 goal 也清。
#[tokio::test]
async fn test_clear_persists_null_and_clears_status() {
    let h = CtxHarness::new();
    let statuses: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let s = statuses.clone();
    let mut ctx = h.ctx.clone();
    ctx.ui.set_status = Arc::new(move |_key, value| s.lock().unwrap().push(value.to_string()));
    let ext = GoalExtension::new();
    let goal = active_goal();
    ext.set_goal_for_test(Some(goal.clone()));
    ext.inner.handle_command("clear", Some(&ctx));
    assert!(ext.goal_snapshot().is_none());
    // 最后一条持久化 entry 是 {goal: null}。
    let entries = h.entries.lock().unwrap();
    let last = entries.last().unwrap();
    assert_eq!(last["data"]["goal"], serde_json::Value::Null, "clear persists null");
    let before = entries.len();
    drop(entries);
    assert!(
        statuses.lock().unwrap().last().map(|s| s.is_empty()).unwrap_or(false),
        "status bar cleared"
    );
    // 无 goal 时 clear 也清持久化。
    ext.inner.handle_command("clear", Some(&ctx));
    assert_eq!(h.entries.lock().unwrap().len(), before + 1);
}

/// C10：工具 details 用 snake_case（goal_id / repeated_turns / resume_after_ms）。
#[tokio::test]
async fn test_tool_details_snake_case() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    let out = ext
        .handle_tool_call(
            GOAL_BLOCKED_TOOL,
            json!({ "goal_id": goal.id, "reason": "r", "evidence": "e", "repeated_turns": 3 }),
            &h.ctx,
        )
        .await
        .expect("handled");
    let details = out.details.expect("details");
    assert_eq!(details["goal_id"], goal.id);
    assert_eq!(details["repeated_turns"], 3);
    assert!(details.get("goalId").is_none(), "camelCase must not leak: {details}");

    // 独立 harness：避免前一次调用持久化的 goal-state entry 被恢复。
    let h2 = CtxHarness::new();
    let goal2 = active_goal();
    let ext2 = set_goal(&goal2);
    let out2 = ext2
        .handle_tool_call(
            GOAL_WAIT_TOOL,
            json!({ "goal_id": goal2.id, "reason": "w", "resume_after_ms": 1 }),
            &h2.ctx,
        )
        .await
        .expect("handled");
    let d2 = out2.details.clone().expect("details");
    assert_eq!(d2["goal_id"], goal2.id);
    assert_eq!(d2["requested_resume_after_ms"], 1);
    assert_eq!(d2["resume_after_ms"], 10_000);
    assert!(d2["resume_at"].is_i64(), "resume_at present: {d2}");
    assert!(d2.get("goalId").is_none(), "camelCase must not leak: {d2}");
    assert!(d2.get("resumeAfterMs").is_none(), "camelCase must not leak: {d2}");
}

/// A3：goal 停止后 before_tool_call 阻塞所有工具 + abort；恢复后放行。
#[tokio::test]
async fn test_stale_tool_calls_blocked() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    let goal = active_goal();
    ext.set_goal_for_test(Some(goal.clone()));
    // 先有 ctx 事件（记住 ctx）。
    let _ = ext
        .handle_tool_call(GOAL_WAIT_TOOL, json!({ "goal_id": "x" }), &h.ctx)
        .await;
    // 正常时放行。
    let res = ext
        .before_tool_call("bash".into(), json!({}), &h.ctx)
        .await;
    assert!(res.is_continue(), "no goal stop → pass through");

    // 暂停 goal（置 stale flag）。
    ext.inner.handle_command("pause", Some(&h.ctx));
    assert!(ext.inner.stale_blocked.load(Ordering::SeqCst));
    let res = ext
        .before_tool_call("bash".into(), json!({}), &h.ctx)
        .await;
    assert!(res.is_cancel(), "stale block must reject all tools");
    assert!(h.aborted.load(Ordering::SeqCst), "stale block must abort the turn");

    // resume 后放行。
    ext.inner.handle_command("resume", Some(&h.ctx));
    let res = ext
        .before_tool_call("bash".into(), json!({}), &h.ctx)
        .await;
    assert!(res.is_continue(), "resume clears stale block");
}

/// A4：/goal pause 中止当前 turn（原版 abortCurrentTurn）。
#[tokio::test]
async fn test_pause_aborts_turn() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    ext.set_goal_for_test(Some(active_goal()));
    ext.inner.handle_command("pause", Some(&h.ctx));
    assert!(h.aborted.load(Ordering::SeqCst), "pause must abort current turn");
}

/// B5：用户输入唤醒 waiting → run 归属 goal（manual）→ agent_end 记 usage
/// 并续发 continuation。
#[tokio::test]
async fn test_user_input_wakes_waiting_and_continues() {
    let h = CtxHarness::new();
    let ext = GoalExtension::new();
    let goal = GoalState {
        waiting: Some(GoalWaiting {
            reason: "waiting for external".into(),
            resume_at: Some(current_millis() + 60_000),
            requested_ms: None,
        }),
        ..active_goal()
    };
    ext.set_goal_for_test(Some(goal.clone()));
    // 先有 ctx 事件（before_agent_start 用 last_ctx）。
    let _ = ext
        .handle_tool_call(GOAL_COMPLETE_TOOL, json!({ "goal_id": "x" }), &h.ctx)
        .await;
    // 用户输入 → 清 wait + 重置安全纪元。
    let res = ext
        .on_input("status? ".to_string(), None, "user".into(), None)
        .await;
    assert!(res.is_continue());
    assert!(ext.goal_snapshot().unwrap().waiting.is_none());

    // before_agent_start：run 归属 goal（manual）。
    let res = ext
        .before_agent_start("status?".into(), None, "sys".into(), None)
        .await;
    if let HookResult::Continue((_, sp, _)) = res {
        // A1：每次 run 注入 goal 系统 prompt。
        assert!(sp.contains("Active /goal:"), "injected system prompt: {sp}");
        assert!(sp.contains(&format!("<goal_id>\n{}\n</goal_id>", goal.id)), "{sp}");
    } else {
        panic!("before_agent_start must continue");
    }

    // agent_start：pending → agent_run（真实事件序）。
    ext.on_agent_start().await;
    // agent_end：续发 continuation。
    let msgs = vec![assistant_msg("stop", None, 50)];
    ext.on_agent_end(&msgs, Some(&h.ctx)).await;
    ext.on_agent_settled(Some(&h.ctx)).await;
    assert_eq!(h.queued.lock().unwrap().len(), 1, "woken goal continues");
}

/// C7：goalSummary 含 Waiting / Resume deadline / Commands 行。
#[test]
fn test_goal_summary_waiting_lines() {
    let goal = GoalState {
        waiting: Some(GoalWaiting {
            reason: "awaiting".into(),
            resume_at: Some(1_700_000_000_000),
            requested_ms: None,
        }),
        ..active_goal()
    };
    let s = goal_summary(&goal);
    assert!(s.contains("Waiting: awaiting"), "{s}");
    assert!(s.contains("Resume deadline: "), "{s}");
    assert!(s.contains("Commands: /goal resume"), "{s}");
}

/// C8：waiting reason 剥终端转义/折叠空白；paused 显示安全原因。
#[test]
fn test_format_status_sanitized_and_pause_cause() {
    let goal = GoalState {
        waiting: Some(GoalWaiting {
            reason: "  a\u{1b}[31mred\u{1b}[0m   reason\n  ".into(),
            resume_at: None,
            requested_ms: None,
        }),
        ..active_goal()
    };
    let s = format_status(&goal);
    assert!(s.contains("waiting ared reason"), "sanitized: {s}");
    assert!(!s.contains("\u{1b}"), "escape stripped: {s}");

    let paused = GoalState {
        status: GoalStatus::Paused,
        safety_pause_cause: Some("no_progress".into()),
        ..active_goal()
    };
    assert!(format_status(&paused).contains("paused · no_progress"), "cause shown");
    let plain = GoalState {
        status: GoalStatus::Paused,
        ..active_goal()
    };
    assert_eq!(format_status(&plain), "paused");
}

/// C12：goal_blocked 的 terminalReason 只存 reason（不含 evidence）。
#[tokio::test]
async fn test_blocked_terminal_reason_is_reason_only() {
    let h = CtxHarness::new();
    let goal = active_goal();
    let ext = set_goal(&goal);
    let out = ext
        .handle_tool_call(
            GOAL_BLOCKED_TOOL,
            json!({ "goal_id": goal.id, "reason": "needs approval", "evidence": "evidence here", "repeated_turns": 3 }),
            &h.ctx,
        )
        .await
        .expect("handled");
    assert!(!out.is_error);
    assert_eq!(
        ext.goal_snapshot().unwrap().terminal_reason.as_deref(),
        Some("needs approval"),
        "terminal reason must be reason only"
    );
}

/// B9：checkpoint 用浮点秒累加（毫秒/1000，不整除截断）。
#[test]
fn test_checkpoint_active_time_fractional() {
    let mut goal = active_goal();
    goal.active_started_at = Some(1_000_000);
    goal.time_used_seconds = 0.0;
    // 1500ms → 1.5s（浮点，不被整除截断成 1s）。
    goal.checkpoint_active_time(1_001_500, true);
    assert!((goal.time_used_seconds - 1.5).abs() < f64::EPSILON, "got {}", goal.time_used_seconds);
}

/// D3/B9：恢复时活跃时钟重设为 now；非法 fingerprint 丢弃；waiting 仅 active
/// 保留。
#[test]
fn test_normalize_loaded_goal_resets_active_clock() {
    let now = current_millis();
    let goal = GoalState {
        status: GoalStatus::Active,
        active_started_at: Some(123), // 旧时钟起点
        last_tool_free_output_fingerprint: Some("not-hex".into()),
        ..active_goal()
    };
    let g = GoalInner::normalize_loaded_goal(goal).expect("kept");
    assert_eq!(g.active_started_at, Some(now), "active clock restarts at now");
    assert!(g.last_tool_free_output_fingerprint.is_none(), "bad fingerprint dropped");

    // 非 active 的 waiting 被丢弃。
    let paused = GoalState {
        status: GoalStatus::Paused,
        waiting: Some(GoalWaiting {
            reason: "w".into(),
            resume_at: None,
            requested_ms: None,
        }),
        ..active_goal()
    };
    let g2 = GoalInner::normalize_loaded_goal(paused).unwrap();
    assert!(g2.waiting.is_none(), "waiting only kept when active");
    assert!(g2.active_started_at.is_none());

    // active + waiting：waiting 保留、时钟暂停（对齐原版）。
    let waiting = GoalState {
        status: GoalStatus::Active,
        active_started_at: Some(123),
        waiting: Some(GoalWaiting {
            reason: "w".into(),
            resume_at: None,
            requested_ms: None,
        }),
        ..active_goal()
    };
    let g3 = GoalInner::normalize_loaded_goal(waiting).unwrap();
    assert!(g3.waiting.is_some(), "waiting kept when active");
    assert!(g3.active_started_at.is_none(), "clock paused while waiting");
}

/// E 类：goal prompt 结构（无预算行；规则数 = 原版 14 条 + 标题）。
#[test]
fn test_goal_prompt_structure() {
    let goal = active_goal();
    let p = build_goal_prompt(&goal);
    assert!(p.contains("</goal_id>\nThis goal_id is only the goal_complete tool stale-turn guard"), "{p}");
    assert!(!p.contains("Token budget"), "no budget line: {p}");
    // 规则数 = 原版 14 条 + 标题。
    let rules = goal_mode_rules("this goal");
    assert_eq!(rules.lines().count(), 15, "rule count: {rules}");
}


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
