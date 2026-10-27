//! subagent 扩展测试：假 pi 脚本模拟子进程 JSONL 输出。
//!
//! 假脚本把每次调用的 argv 追加到 `<dir>/argv.log`（`CALL` / 参数逐行 /
//! `END`），测试据此断言子进程参数构造（模型继承、thinking、工具白名单、
//! persona、`{previous}` 代入）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use pi_extension_api::{ExtensionContext, ExtensionUIContext, RuntimeHandle, ToolInvocation};

const SUCCESS_PAYLOAD: &str = r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"model":"child-model","usage":{"input":10,"output":5,"cacheRead":2,"cacheWrite":1,"totalTokens":17,"cost":{"total":0.5}},"stop_reason":"stop"}}'
printf '%s\n' '{"type":"end"}'
"#;

const ERROR_PAYLOAD: &str = r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[],"usage":{},"stop_reason":"error","error_message":"provider exploded"}}'
printf '%s\n' '{"type":"end"}'
"#;

const EXIT_1_PAYLOAD: &str = r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"partial"}],"usage":{},"stop_reason":"stop"}}'
exit 1
"#;

/// 测试环境：临时 agent 目录 + 临时 cwd + 假 pi 脚本。
struct TestEnv {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    confirm_answer: bool,
    /// 项目信任状态（注入 `ctx.is_project_trusted`）。
    project_trusted: bool,
    /// `ui.confirm` 被调用的次数（断言"受信任项目不弹确认"）。
    confirm_calls: Arc<std::sync::Mutex<usize>>,
    pi_binary: String,
    argv_log: PathBuf,
}

impl TestEnv {
    fn new(payload: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let argv_log = home.path().join("argv.log");
        let script = home.path().join("fake-pi.sh");
        let body = format!(
            "#!/bin/sh\n{{\n  printf 'CALL\\n'\n  for a in \"$@\"; do printf '%s\\n' \"$a\"; done\n  printf 'END\\n'\n}} >> '{log}'\n{payload}",
            log = argv_log.display(),
            payload = payload
        );
        std::fs::write(&script, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self {
            home,
            cwd,
            confirm_answer: true,
            project_trusted: false,
            confirm_calls: Arc::new(std::sync::Mutex::new(0)),
            pi_binary: script.to_string_lossy().into_owned(),
            argv_log,
        }
    }

    fn agents_dir(&self) -> PathBuf {
        self.home.path().join("agent").join("agents")
    }

    fn write_agent(&self, dir: &Path, file: &str, content: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(file), content).unwrap();
    }

    fn write_user_agent(&self, name: &str, content: &str) {
        self.write_agent(&self.agents_dir(), &format!("{name}.md"), content);
    }

    fn write_project_agent(&self, name: &str, content: &str) {
        let dir = self.cwd.path().join(PROJECT_CONFIG_DIR).join("agents");
        self.write_agent(&dir, &format!("{name}.md"), content);
    }

    fn extension(&self) -> SubagentExtension {
        SubagentExtension::new().with_pi_binary(&self.pi_binary)
    }

    fn ctx(&self, mode: &str) -> ExtensionContext {
        let mut handle = RuntimeHandle::noop();
        let agent_dir = self
            .home
            .path()
            .join("agent")
            .to_string_lossy()
            .into_owned();
        handle.get_agent_dir = Arc::new(move || agent_dir.clone());
        let cwd = self.cwd.path().to_string_lossy().into_owned();
        let cwd_owned = cwd.clone();
        handle.get_cwd = Arc::new(move || cwd_owned.clone());
        // 派发默认值：父会话模型 + thinking level。
        handle.get_model = Arc::new(|| "anthropic/claude-parent".to_string());
        handle.get_thinking_level = Arc::new(|| "high".to_string());

        let mut ui = ExtensionUIContext::noop();
        let answer = self.confirm_answer;
        let calls = Arc::clone(&self.confirm_calls);
        ui.confirm = Arc::new(move |_title: &str, _message: &Value| {
            *calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            answer
        });

        let mut ctx = ExtensionContext::new("test-session".into(), false, ui, handle);
        ctx.set_mode(mode);
        // 对齐 TS：`hasUI` = 绑定了真实 UI；interactive 模式启动时会换绑。
        ctx.set_has_ui(mode == "tui");
        ctx.set_project_trusted(self.project_trusted);
        ctx
    }

    fn confirm_calls(&self) -> usize {
        *self
            .confirm_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 每次调用的参数列表（按调用顺序）。
    fn calls(&self) -> Vec<Vec<String>> {
        let Ok(log) = std::fs::read_to_string(&self.argv_log) else {
            return Vec::new();
        };
        let mut calls: Vec<Vec<String>> = Vec::new();
        let mut current: Option<Vec<String>> = None;
        for line in log.lines() {
            match line {
                "CALL" => current = Some(Vec::new()),
                "END" => {
                    if let Some(call) = current.take() {
                        calls.push(call);
                    }
                }
                other => {
                    if let Some(call) = current.as_mut() {
                        call.push(other.to_string());
                    }
                }
            }
        }
        calls
    }
}

fn text_of(output: &ToolCallOutput) -> String {
    output.content[0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn worker_agent() -> &'static str {
    "---\nname: worker\ndescription: General purpose\n---\nYou are a worker."
}

/// single 模式：agent 存在 → 跑子进程，返回终稿 + 结构化 details。
#[tokio::test]
async fn single_mode_returns_final_output_and_details() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "do it" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .expect("handled");

    assert!(!out.is_error, "unexpected error: {out:?}");
    assert_eq!(text_of(&out), "done");

    let details = out.details.clone().expect("details");
    assert_eq!(details["mode"], "single");
    assert_eq!(details["agentScope"], "user");
    let result = &details["results"][0];
    assert_eq!(result["agent"], "worker");
    assert_eq!(result["agentSource"], "user");
    assert_eq!(result["exitCode"], 0);
    assert_eq!(result["stopReason"], "stop");
    // usage 累计（turns / tokens / cache / cost / ctx）
    assert_eq!(result["usage"]["turns"], 1);
    assert_eq!(result["usage"]["input"], 10);
    assert_eq!(result["usage"]["output"], 5);
    assert_eq!(result["usage"]["cacheRead"], 2);
    assert_eq!(result["usage"]["cacheWrite"], 1);
    assert_eq!(result["usage"]["contextTokens"], 17);
    assert_eq!(result["usage"]["cost"], 0.5);
    // TS：构造时就写入派发模型，子进程回报的 model 不会覆盖它
    assert_eq!(result["model"], "anthropic/claude-parent");

    // 子进程参数：--mode json -p --no-session + persona + Task 前缀
    let calls = env.calls();
    assert_eq!(calls.len(), 1);
    let args = &calls[0];
    assert!(args.contains(&"--mode".to_string()));
    assert!(args.contains(&"json".to_string()));
    assert!(args.contains(&"-p".to_string()));
    assert!(args.contains(&"--no-session".to_string()));
    assert!(args.contains(&"--append-prompt".to_string()));
    assert!(args.contains(&"You are a worker.".to_string()));
    assert_eq!(args.last().unwrap(), "Task: do it");
}

/// agent 未定义 model：继承父会话模型 + thinking level。
#[tokio::test]
async fn inherits_dispatch_model_and_thinking_level() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    env.extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    let args = env.calls().remove(0);
    let model_at = args.iter().position(|a| a == "--model").expect("--model");
    assert_eq!(args[model_at + 1], "anthropic/claude-parent");
    let thinking_at = args
        .iter()
        .position(|a| a == "--thinking")
        .expect("--thinking");
    assert_eq!(args[thinking_at + 1], "high");
}

/// agent 定义了 model：不继承 thinking level（对齐原版 inheritsDispatchConfig）。
#[tokio::test]
async fn agent_model_disables_thinking_inheritance() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent(
        "worker",
        "---\nname: worker\ndescription: d\nmodel: openai/gpt-5\n---\nbody",
    );

    env.extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    let args = env.calls().remove(0);
    let model_at = args.iter().position(|a| a == "--model").expect("--model");
    assert_eq!(args[model_at + 1], "openai/gpt-5");
    assert!(!args.contains(&"--thinking".to_string()));
}

/// 空 model 是 TS falsy：继承派发模型/thinking，且不能生成 `--model ""`。
#[tokio::test]
async fn empty_agent_model_inherits_dispatch_config() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent(
        "worker",
        "---\nname: worker\ndescription: d\nmodel: \"\"\n---\nbody",
    );

    env.extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    let args = env.calls().remove(0);
    let model_at = args.iter().position(|a| a == "--model").expect("--model");
    assert_eq!(args[model_at + 1], "anthropic/claude-parent");
    let thinking_at = args
        .iter()
        .position(|a| a == "--thinking")
        .expect("--thinking");
    assert_eq!(args[thinking_at + 1], "high");
    assert!(!args.iter().any(String::is_empty));
}

/// agent 的 tools 白名单透传为 `--tools a,b`。
#[tokio::test]
async fn agent_tools_become_tools_flag() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent(
        "scout",
        "---\nname: scout\ndescription: d\ntools:\n  - read\n  - grep\n---\nbody",
    );

    env.extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "scout", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    let args = env.calls().remove(0);
    let tools_at = args.iter().position(|a| a == "--tools").expect("--tools");
    assert_eq!(args[tools_at + 1], "read,grep");
}

/// 未知 agent：报错并列出可用 agent。
#[tokio::test]
async fn unknown_agent_lists_available() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "nope", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(out.is_error);
    let text = text_of(&out);
    assert!(text.contains("Unknown agent: \"nope\""), "got: {text}");
    assert!(text.contains("\"worker\""), "got: {text}");
    assert!(env.calls().is_empty(), "must not spawn");
}

/// 参数模式校验：0 种或 2 种模式都拒绝，并列出可用 agent。
#[tokio::test]
async fn invalid_mode_count_is_rejected() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());
    let ext = env.extension();
    let ctx = env.ctx("cli");

    let none = ext
        .handle_tool_call("subagent", json!({}), &ctx, &ToolInvocation::noop())
        .await
        .unwrap();
    assert!(!none.is_error);
    assert!(text_of(&none).contains("Provide exactly one mode."));

    let both = ext
        .handle_tool_call(
            "subagent",
            json!({
                "agent": "worker",
                "task": "t",
                "tasks": [{ "agent": "worker", "task": "t2" }],
            }),
            &ctx,
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();
    assert!(text_of(&both).contains("Provide exactly one mode."));
    assert!(text_of(&both).contains("worker (user)"));
    assert!(env.calls().is_empty());
}

/// parallel/chain 的每个 TaskStep 都必须先整体校验；任一非法项都拒绝整次
/// 调用、列出可用 agent，且不能先启动前面的合法步骤。
#[tokio::test]
async fn invalid_task_steps_are_rejected_without_spawning() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());
    let ext = env.extension();
    let ctx = env.ctx("cli");
    let cases = [
        (
            json!({ "chain": [
                { "agent": "worker", "task": "first" },
                { "task": "missing agent" },
            ] }),
            "Invalid chain step 2",
            "agent",
        ),
        (
            json!({ "chain": [
                { "agent": "worker", "task": "first" },
                { "agent": "worker", "task": "" },
            ] }),
            "Invalid chain step 2",
            "task",
        ),
        (
            json!({ "tasks": [
                { "agent": "worker", "task": "first" },
                { "agent": "worker" },
            ] }),
            "Invalid parallel step 2",
            "task",
        ),
        (
            json!({ "tasks": [
                { "agent": "worker", "task": "first" },
                { "agent": "", "task": "empty agent" },
            ] }),
            "Invalid parallel step 2",
            "agent",
        ),
    ];

    for (params, expected_error, expected_field) in cases {
        let out = ext
            .handle_tool_call("subagent", params, &ctx, &ToolInvocation::noop())
            .await
            .unwrap();
        assert!(out.is_error, "invalid step must fail: {out:?}");
        let text = text_of(&out);
        assert!(text.contains(expected_error), "got: {text}");
        assert!(
            text.contains(&format!("missing or empty field(s): {expected_field}")),
            "got: {text}"
        );
        assert!(
            text.contains("Available agents: worker (user)"),
            "got: {text}"
        );
    }
    assert!(env.calls().is_empty(), "no subprocess may start");
}

/// chain：顺序执行，`{previous}` 代入上一步输出。
#[tokio::test]
async fn chain_substitutes_previous_output() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({
                "chain": [
                    { "agent": "worker", "task": "first" },
                    { "agent": "worker", "task": "use {previous}" },
                ],
            }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(!out.is_error, "unexpected error: {out:?}");
    assert_eq!(text_of(&out), "done");
    assert_eq!(out.details.as_ref().unwrap()["mode"], "chain");

    let calls = env.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].last().unwrap(), "Task: first");
    // 上一步输出是 "done"
    assert_eq!(calls[1].last().unwrap(), "Task: use done");
}

/// chain：任一步失败即停，并报告步号与 agent。
#[tokio::test]
async fn chain_stops_at_first_failure() {
    let env = TestEnv::new(ERROR_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({
                "chain": [
                    { "agent": "worker", "task": "first" },
                    { "agent": "worker", "task": "second" },
                ],
            }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(out.is_error);
    let text = text_of(&out);
    assert!(
        text.contains("Chain stopped at step 1 (worker)"),
        "got: {text}"
    );
    assert!(text.contains("provider exploded"), "got: {text}");
    assert_eq!(env.calls().len(), 1);
}

/// parallel：成功/失败汇总 + `### [agent] status` 分节。
#[tokio::test]
async fn parallel_summarizes_each_task() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({
                "tasks": [
                    { "agent": "worker", "task": "a" },
                    { "agent": "missing", "task": "b" },
                ],
            }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    let text = text_of(&out);
    assert!(text.contains("Parallel: 1/2 succeeded"), "got: {text}");
    assert!(text.contains("### [worker] completed"), "got: {text}");
    assert!(text.contains("### [missing] failed"), "got: {text}");

    let details = out.details.clone().unwrap();
    assert_eq!(details["mode"], "parallel");
    assert_eq!(details["results"][0]["agent"], "worker");
    assert_eq!(details["results"][1]["agentSource"], "unknown");
}

/// parallel：超过 8 个任务直接拒绝（不发子进程）。
#[tokio::test]
async fn parallel_rejects_too_many_tasks() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());
    let tasks: Vec<Value> = (0..9)
        .map(|i| json!({ "agent": "worker", "task": format!("t{i}") }))
        .collect();

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "tasks": tasks }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(text_of(&out).contains("Too many parallel tasks (9). Max is 8."));
    assert!(env.calls().is_empty());
}

/// 退出码非 0 → 失败（即使有输出）。
#[tokio::test]
async fn non_zero_exit_code_is_failure() {
    let env = TestEnv::new(EXIT_1_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(out.is_error);
    assert_eq!(out.details.as_ref().unwrap()["results"][0]["exitCode"], 1);
    assert!(text_of(&out).contains("partial"));
}

/// 非法 cwd 在 spawn 前直接失败，不能伪装成 binary 不存在。
#[tokio::test]
async fn invalid_cwd_is_rejected_before_spawn() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());
    let invalid_cwd = env.cwd.path().join("missing-directory");
    let invalid_cwd = invalid_cwd.to_string_lossy().into_owned();

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t", "cwd": invalid_cwd }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(out.is_error);
    let text = text_of(&out);
    assert!(
        text.contains("Invalid cwd (not a directory)"),
        "got: {text}"
    );
    assert!(text.contains(&invalid_cwd), "got: {text}");
    assert!(text.contains(&env.pi_binary), "got: {text}");
    assert!(env.calls().is_empty(), "cwd validation must precede spawn");
}

/// spawn 失败诊断同时带上 binary 和 cwd，便于区分路径问题。
#[tokio::test]
async fn spawn_error_reports_binary_and_cwd() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_user_agent("worker", worker_agent());
    let missing_binary = env
        .home
        .path()
        .join("missing-pi")
        .to_string_lossy()
        .into_owned();

    let out = SubagentExtension::new()
        .with_pi_binary(&missing_binary)
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(out.is_error);
    let text = text_of(&out);
    assert!(text.contains("Failed to spawn pi"), "got: {text}");
    assert!(text.contains(&missing_binary), "got: {text}");
    assert!(text.contains("cwd:"), "got: {text}");
    assert!(
        text.contains(&env.cwd.path().to_string_lossy().into_owned()),
        "got: {text}"
    );
}

/// stopReason = error → 失败，错误信息来自 errorMessage。
#[tokio::test]
async fn stop_reason_error_is_failure() {
    let env = TestEnv::new(ERROR_PAYLOAD);
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(out.is_error);
    assert!(text_of(&out).contains("Agent error: provider exploded"));
}

/// 默认 scope = user：项目级 agent 不加载。
#[tokio::test]
async fn project_agents_are_opt_in() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_project_agent(
        "reviewer",
        "---\nname: reviewer\ndescription: Review\n---\nReview only.",
    );

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "reviewer", "task": "t" }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(out.is_error);
    assert!(text_of(&out).contains("Unknown agent"));
    assert!(env.calls().is_empty());
}

/// agentScope = both + TUI：项目 agent 需确认；拒绝则取消。
#[tokio::test]
async fn project_agents_require_confirmation() {
    let mut env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_project_agent(
        "reviewer",
        "---\nname: reviewer\ndescription: Review\n---\nReview only.",
    );
    env.confirm_answer = false;

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "reviewer", "task": "t", "agentScope": "both" }),
            &env.ctx("tui"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(!out.is_error);
    assert_eq!(
        text_of(&out),
        "Canceled: project-local agents not approved."
    );
    assert!(env.calls().is_empty());

    // 同意后正常执行，且参数里带项目 agent 的 persona
    env.confirm_answer = true;
    let ok = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "reviewer", "task": "t", "agentScope": "both" }),
            &env.ctx("tui"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();
    assert!(!ok.is_error, "unexpected error: {ok:?}");
    assert_eq!(env.calls().len(), 1);
    assert!(env.calls()[0].contains(&"Review only.".to_string()));
    assert_eq!(
        ok.details.as_ref().unwrap()["results"][0]["agentSource"],
        "project"
    );
}

/// 项目已受信任：即使 scope=both + TUI 也不弹确认（对齐原版
/// `!ctx.isProjectTrusted()` 分支）。
#[tokio::test]
async fn trusted_project_skips_confirmation() {
    let mut env = TestEnv::new(SUCCESS_PAYLOAD);
    env.write_project_agent(
        "reviewer",
        "---\nname: reviewer\ndescription: Review\n---\nReview only.",
    );
    env.project_trusted = true;
    env.confirm_answer = false; // 一旦弹窗就会被拒

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "reviewer", "task": "t", "agentScope": "both" }),
            &env.ctx("tui"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(!out.is_error, "unexpected error: {out:?}");
    assert_eq!(text_of(&out), "done");
    assert_eq!(
        env.confirm_calls(),
        0,
        "no confirmation for trusted projects"
    );
}

/// 非 subagent 工具不处理。
#[tokio::test]
async fn ignores_other_tools() {
    let env = TestEnv::new(SUCCESS_PAYLOAD);
    let out = env
        .extension()
        .handle_tool_call(
            "web_search",
            json!({}),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await;
    assert!(out.is_none());
}

#[test]
fn truncate_parallel_output_caps_and_notes() {
    let small = "x".repeat(100);
    assert_eq!(truncate_parallel_output(&small), small);

    let big = "y".repeat(PER_TASK_OUTPUT_CAP + 100);
    let truncated = truncate_parallel_output(&big);
    assert!(truncated.len() < big.len());
    assert!(truncated.contains("[Output truncated: 100 bytes omitted."));
}

#[test]
fn usage_format_matches_ts_shape() {
    let usage = UsageStats {
        input: 1500,
        output: 255_000,
        cache_read: 1200,
        cache_write: 0,
        cost: 0.0123,
        context_tokens: 900,
        turns: 2,
    };
    assert_eq!(
        usage.format(Some("claude-sonnet")),
        "2 turns ↑1.5k ↓255k R1.2k $0.0123 ctx:900 claude-sonnet"
    );
    assert_eq!(format_tokens(999), "999");
    assert_eq!(format_tokens(1_500_000), "1.5M");
    assert_eq!(UsageStats::default().format(None), "");
}

/// 中止信号：Ctrl+C 等价物 → 杀掉子进程，结果标记 aborted（不等进程自然结束）。
#[tokio::test]
async fn abort_kills_child_and_marks_aborted() {
    let env = TestEnv::new(
        r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"started"}],"usage":{},"stop_reason":"toolUse"}}'
sleep 30
"#,
    );
    env.write_user_agent("worker", worker_agent());

    let (tx, rx) = tokio::sync::watch::channel(false);
    let invocation = ToolInvocation::new("call-1", Some(rx), pi_extension_api::noop_tool_update());

    let handle = tokio::spawn({
        let pi_binary = env.pi_binary.clone();
        let ctx = env.ctx("cli");
        async move {
            SubagentExtension::new()
                .with_pi_binary(&pi_binary)
                .handle_tool_call(
                    "subagent",
                    json!({ "agent": "worker", "task": "t" }),
                    &ctx,
                    &invocation,
                )
                .await
                .unwrap()
        }
    });

    // 等子进程起来再中止。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    tx.send(true).unwrap();

    let out = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("abort should finish the call, not wait for the child")
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(out.is_error, "aborted call must be an error: {out:?}");
    let text = text_of(&out);
    assert!(text.contains("aborted"), "got: {text}");
    assert_eq!(
        out.details.as_ref().unwrap()["results"][0]["stopReason"],
        "aborted"
    );
}

/// stdout 已 EOF 但进程仍存活时，abort 仍必须立刻杀掉并结束等待。
#[tokio::test]
async fn abort_after_stdout_eof_kills_child() {
    let env = TestEnv::new(
        r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"started"}],"usage":{},"stop_reason":"toolUse"}}'
exec 1>&-
sleep 30
"#,
    );
    env.write_user_agent("worker", worker_agent());

    let (tx, rx) = tokio::sync::watch::channel(false);
    let invocation = ToolInvocation::new("call-1", Some(rx), pi_extension_api::noop_tool_update());

    let handle = tokio::spawn({
        let pi_binary = env.pi_binary.clone();
        let ctx = env.ctx("cli");
        async move {
            SubagentExtension::new()
                .with_pi_binary(&pi_binary)
                .handle_tool_call(
                    "subagent",
                    json!({ "agent": "worker", "task": "t" }),
                    &ctx,
                    &invocation,
                )
                .await
                .unwrap()
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    tx.send(true).unwrap();

    let out = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("abort must interrupt the post-EOF child wait")
        .unwrap();
    assert!(out.is_error, "aborted call must be an error: {out:?}");
    assert_eq!(
        out.details.as_ref().unwrap()["results"][0]["stopReason"],
        "aborted"
    );
}

/// 运行中上报中间结果（对齐 TS `onUpdate`）。
#[tokio::test]
async fn emits_progress_updates_while_running() {
    let env = TestEnv::new(
        r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"half way"}],"usage":{"input":1,"output":1,"totalTokens":2,"cost":{"total":0.0}},"stop_reason":"toolUse"}}'
sleep 1
printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input":1,"output":1,"totalTokens":2,"cost":{"total":0.0}},"stop_reason":"stop"}}'
"#,
    );
    env.write_user_agent("worker", worker_agent());

    let updates: Arc<std::sync::Mutex<Vec<ToolCallOutput>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&updates);
    let on_update: pi_extension_api::ToolUpdateFn = Arc::new(move |output: ToolCallOutput| {
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(output);
    });
    let invocation = ToolInvocation::new("call-1", None, on_update);

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "agent": "worker", "task": "t" }),
            &env.ctx("cli"),
            &invocation,
        )
        .await
        .unwrap();
    assert_eq!(text_of(&out), "done");

    let updates = updates
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(updates.len(), 2, "one update per child message");
    assert_eq!(updates[0].content[0]["text"], "half way");
    assert_eq!(updates[0].details.as_ref().unwrap()["mode"], "single");
    assert_eq!(updates[1].content[0]["text"], "done");
}

// ============================================================================
// 自定义渲染（对齐 TS renderCall / renderResult）
// ============================================================================

fn render_ctx(args: Value, details: Option<Value>, expanded: bool) -> ToolRenderContext {
    ToolRenderContext {
        tool_name: "subagent".to_string(),
        cwd: "/repo".to_string(),
        expanded,
        args,
        output: String::new(),
        details,
        is_error: false,
    }
}

fn assistant_message(text: &str, tool_calls: &[(&str, Value)]) -> Value {
    let mut content = vec![json!({ "type": "text", "text": text })];
    for (name, args) in tool_calls {
        content.push(json!({ "type": "toolCall", "id": "c1", "name": name, "arguments": args }));
    }
    json!({
        "role": "assistant",
        "content": content,
        "model": "claude-sonnet",
        "usage": { "input": 100, "output": 50, "cacheRead": 10, "cacheWrite": 0,
                   "totalTokens": 160, "cost": { "total": 0.25 } },
        "stop_reason": "stop",
    })
}

fn single_details(expanded_extra: bool) -> Value {
    let mut messages = vec![assistant_message(
        "planning",
        &[(
            "read",
            json!({"path": "src/a.rs", "offset": 1, "limit": 10}),
        )],
    )];
    if expanded_extra {
        messages.push(assistant_message("all done", &[]));
    }
    json!({
        "mode": "single",
        "agentScope": "user",
        "projectAgentsDir": null,
        "results": [{
            "agent": "worker",
            "agentSource": "user",
            "task": "review src/a.rs",
            "exitCode": 0,
            "messages": messages,
            "stderr": "",
            "usage": { "input": 100, "output": 50, "cacheRead": 10, "cacheWrite": 0,
                       "cost": 0.25, "contextTokens": 160, "turns": 1 },
            "model": "claude-sonnet",
            "stopReason": "stop",
            "errorMessage": null,
            "step": null,
        }],
    })
}

/// 渲染辅助：标题拼成纯文本（便于断言）。
fn title_text(out: &ToolRenderOutput) -> String {
    out.title.iter().map(|s| s.text.as_str()).collect()
}

/// 渲染辅助：内容块拼成纯文本行（Markdown 块原样出现）。
fn blocks_text(out: &ToolRenderOutput) -> Vec<String> {
    out.blocks
        .iter()
        .map(|block| match block {
            ToolRenderBlock::Line(spans) => {
                spans.iter().map(|s| s.text.as_str()).collect::<String>()
            }
            ToolRenderBlock::Spacer => String::new(),
            ToolRenderBlock::Markdown(text) => text.clone(),
        })
        .collect()
}

/// 折叠态：标题 + 工具调用行 + `(Ctrl+O to expand)` + usage。
#[test]
fn render_single_collapsed() {
    let out = render_tool(&render_ctx(
        json!({ "agent": "worker", "task": "review src/a.rs" }),
        Some(single_details(false)),
        false,
    ))
    .expect("rendered");

    assert_eq!(title_text(&out), "✓subagent worker (user) [user]");
    let lines = blocks_text(&out);
    assert!(
        lines.iter().any(|l| l == "→ read src/a.rs:1-10"),
        "tool call line: {lines:?}"
    );
    let text = lines.join("\n");
    assert!(text.contains("(Ctrl+O to expand)"), "expand hint: {text}");
    assert!(text.contains("1 turn"), "usage: {text}");
    assert!(text.contains("$0.2500"), "cost: {text}");
    // 折叠态没有 Markdown 终稿块
    assert!(
        !out.blocks
            .iter()
            .any(|b| matches!(b, ToolRenderBlock::Markdown(_))),
        "collapsed has no markdown block"
    );
}

/// 展开态：`─── Task ───` / `─── Output ───` 分节 + Markdown 终稿 + usage。
#[test]
fn render_single_expanded() {
    let out = render_tool(&render_ctx(
        json!({ "agent": "worker", "task": "review src/a.rs" }),
        Some(single_details(true)),
        true,
    ))
    .expect("rendered");

    let lines = blocks_text(&out);
    assert!(
        lines.iter().any(|l| l == "─── Task ───"),
        "lines: {lines:?}"
    );
    assert!(lines.iter().any(|l| l == "review src/a.rs"));
    assert!(lines.iter().any(|l| l == "─── Output ───"));
    assert!(
        out.blocks
            .iter()
            .any(|b| matches!(b, ToolRenderBlock::Markdown(text) if text == "all done")),
        "final output as markdown: {lines:?}"
    );
    assert!(
        !lines.join("\n").contains("Ctrl+O"),
        "no hint when expanded"
    );
}

/// 失败结果：图标 ✗ 用 Error 语义色，标题带 stopReason，块里带 errorMessage。
#[test]
fn render_failure_shows_reason_and_error() {
    let mut details = single_details(false);
    details["results"][0]["exitCode"] = json!(1);
    details["results"][0]["stopReason"] = json!("error");
    details["results"][0]["errorMessage"] = json!("provider exploded");

    let out = render_tool(&render_ctx(
        json!({ "agent": "worker", "task": "t" }),
        Some(details),
        false,
    ))
    .expect("rendered");

    assert_eq!(
        out.title[0].style,
        ToolTextStyle::Error,
        "icon styled as error"
    );
    assert_eq!(out.title[0].text, "✗");
    let title = title_text(&out);
    assert!(title.contains("[error]"), "title: {title}");
    assert!(blocks_text(&out)
        .iter()
        .any(|l| l == "Error: provider exploded"));
}

/// parallel：标题带进度，展开态每步 `Task:` + 终稿 + 该步 usage + `Total:`。
#[test]
fn render_parallel_titles_and_sections() {
    let details = json!({
        "mode": "parallel",
        "agentScope": "both",
        "projectAgentsDir": "/repo/.pi-rs/agents",
        "results": [
            {
                "agent": "worker", "agentSource": "user", "task": "t1", "exitCode": 0,
                "messages": [assistant_message("out one", &[])],
                "stderr": "", "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                                        "cost": 0.0, "contextTokens": 2, "turns": 1},
                "model": "m", "stopReason": "stop", "errorMessage": null, "step": null,
            },
            {
                "agent": "broken", "agentSource": "unknown", "task": "t2", "exitCode": 1,
                "messages": [], "stderr": "boom", "usage": {"input": 0, "output": 0,
                "cacheRead": 0, "cacheWrite": 0, "cost": 0.0, "contextTokens": 0, "turns": 0},
                "model": null, "stopReason": null, "errorMessage": "Unknown agent", "step": null,
            },
        ],
    });

    let collapsed = render_tool(&render_ctx(
        json!({ "tasks": [{ "agent": "worker", "task": "t1" }], "agentScope": "both" }),
        Some(details.clone()),
        false,
    ))
    .expect("rendered");
    let title = title_text(&collapsed);
    assert!(title.contains("parallel 1/2 tasks"), "title: {title}");
    assert!(title.contains("[both]"), "title: {title}");
    let lines = blocks_text(&collapsed);
    assert!(lines.iter().any(|l| l.contains("─── worker")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("─── broken")), "{lines:?}");
    assert!(lines.iter().any(|l| l == "Error: Unknown agent"));
    assert!(lines.iter().any(|l| l == "(Ctrl+O to expand)"));

    let expanded = render_tool(&render_ctx(
        json!({ "tasks": [], "agentScope": "both" }),
        Some(details),
        true,
    ))
    .expect("rendered");
    let lines = blocks_text(&expanded);
    let text = lines.join("\n");
    assert!(text.contains("─── worker ✓"), "body: {text}");
    assert!(text.contains("─── broken ✗"), "body: {text}");
    // 每步 Task 行 + 该步 usage
    assert!(text.contains("Task: t1"), "body: {text}");
    assert!(text.contains("1 turn"), "per-step usage: {text}");
    assert!(text.contains("Total: 1 turn"), "total usage: {text}");
    assert!(text.contains("out one"), "body: {text}");
    assert!(text.contains("Unknown agent"), "body: {text}");
}

/// chain：标题带步数，展开态按 `─── Step N ───` 分节。
#[test]
fn render_chain_sections() {
    let step = |index: usize, output: &str| {
        json!({
            "agent": "worker", "agentSource": "user", "task": "t", "exitCode": 0,
            "messages": [assistant_message(output, &[])],
            "stderr": "", "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                                    "cost": 0.0, "contextTokens": 2, "turns": 1},
            "model": "m", "stopReason": "stop", "errorMessage": null, "step": index,
        })
    };
    let details = json!({
        "mode": "chain", "agentScope": "user", "projectAgentsDir": null,
        "results": [step(1, "one"), step(2, "two")],
    });

    let out =
        render_tool(&render_ctx(json!({ "chain": [] }), Some(details), true)).expect("rendered");
    let title = title_text(&out);
    assert!(title.contains("chain 2/2 steps"), "title: {title}");
    let text = blocks_text(&out).join("\n");
    assert!(text.contains("─── Step 1: worker ✓"), "body: {text}");
    assert!(text.contains("─── Step 2: worker ✓"), "body: {text}");
}

/// 调用阶段（无 details）：标题分段着色 + 步骤预览（TS renderCall）。
#[test]
fn render_call_previews() {
    let chain = render_tool(&render_ctx(
        json!({ "chain": [
            { "agent": "scout", "task": "find {previous}" },
            { "agent": "planner", "task": "plan" },
        ], "agentScope": "both" }),
        None,
        false,
    ))
    .expect("rendered");
    assert_eq!(title_text(&chain), "subagent chain (2 steps) [both]");
    assert_eq!(chain.title[1].style, ToolTextStyle::Accent, "描述走 accent");
    assert_eq!(chain.title[2].text, " [both]");
    let lines = blocks_text(&chain);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("  1. scout"), "lines: {lines:?}");

    let single = render_tool(&render_ctx(
        json!({ "agent": "worker", "task": "do it" }),
        None,
        false,
    ))
    .expect("rendered");
    assert_eq!(title_text(&single), "subagent worker [user]");
    assert_eq!(blocks_text(&single), vec!["  do it".to_string()]);
}

/// 工具注册时挂上 render 回调（宿主据此接 TUI 渲染）。
#[test]
fn tool_definition_exposes_render_fn() {
    let ext = SubagentExtension::new();
    let mut registry = ToolRegistry::new(pi_extension_api::create_builtin_source_info("subagent"));
    ext.register_tools(&mut registry);
    let tools = registry.into_vec();
    let def = tools
        .iter()
        .find(|t| t.definition.name == "subagent")
        .expect("registered");
    assert!(
        def.definition.render.is_some(),
        "render callback registered"
    );
}

/// 子 agent 自己返回 aborted 不等同于父级取消：应保留其他任务结果并作为
/// 普通失败项汇总。原版只有 AbortSignal 回调设置 wasAborted 才整次抛错。
#[tokio::test]
async fn child_aborted_result_does_not_abort_parallel_batch() {
    let env = TestEnv::new(
        r#"case "$*" in
  *aborted-task*)
    printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"child aborted"}],"usage":{},"stop_reason":"aborted"}}'
    ;;
  *)
    printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"usage":{},"stop_reason":"stop"}}'
    ;;
esac
"#,
    );
    env.write_user_agent("worker", worker_agent());

    let out = env
        .extension()
        .handle_tool_call(
            "subagent",
            json!({ "tasks": [
                { "agent": "worker", "task": "aborted-task" },
                { "agent": "worker", "task": "normal-task" },
            ] }),
            &env.ctx("cli"),
            &ToolInvocation::noop(),
        )
        .await
        .unwrap();

    assert!(!out.is_error, "parallel summarizes child failures");
    let text = text_of(&out);
    assert!(text.contains("Parallel: 1/2 succeeded"), "got: {text}");
    assert!(text.contains("failed (aborted)"), "got: {text}");
    assert!(text.contains("child aborted"), "got: {text}");
    assert!(text.contains("done"), "got: {text}");
}

/// parallel 中止：整次调用报错（对齐原版 `Promise.all` 拒绝 + 抛
/// "Subagent was aborted"），而不是当成"某个任务失败"返回汇总。
#[tokio::test]
async fn parallel_abort_fails_whole_call() {
    let env = TestEnv::new(
        r#"printf '%s\n' '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"started"}],"usage":{},"stop_reason":"toolUse"}}'
sleep 30
"#,
    );
    env.write_user_agent("worker", worker_agent());

    let (tx, rx) = tokio::sync::watch::channel(false);
    let invocation = ToolInvocation::new("call-1", Some(rx), pi_extension_api::noop_tool_update());

    let handle = tokio::spawn({
        let pi_binary = env.pi_binary.clone();
        let ctx = env.ctx("cli");
        async move {
            SubagentExtension::new()
                .with_pi_binary(&pi_binary)
                .handle_tool_call(
                    "subagent",
                    json!({ "tasks": [
                        { "agent": "worker", "task": "a" },
                        { "agent": "worker", "task": "b" },
                    ] }),
                    &ctx,
                    &invocation,
                )
                .await
                .unwrap()
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    tx.send(true).unwrap();

    let out = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .expect("abort finishes the call")
        .unwrap();
    assert!(out.is_error, "abort must fail the whole call: {out:?}");
    assert_eq!(text_of(&out), "Subagent was aborted");
    let details = out.details.unwrap();
    assert_eq!(details["mode"], "parallel");
    assert_eq!(details["results"][0]["stopReason"], "aborted");
}
