//! pi-subagent — subagent 工具，委派独立任务给子 agent。
//!
//! 核心版（对应 pi-subagents 的 subagent 工具，去掉工作流/控制通道）：
//! - 注册 `subagent` 工具，LLM 自主决定是否调用（工具描述引导，同 Claude Code）
//! - 前台：spawn 子 pi 进程（`pi --mode json -p --model <m> --no-session <task>`），
//!   解析 stdout JSONL 事件流，提取最终 assistant 消息，超时 kill
//! - 后台（async: true）：spawn 子进程不等待，返回 run_id，状态写
//!   `{agent_dir}/subagent-runs/{run_id}/status.json`，可用 action=status 查询
//! - 深度限制（防无限递归）

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pi_extension_api::{ExtensionContext, HookHandler, ToolCallOutput, ToolDefinition, ToolRegistry};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

#[cfg(test)]
mod tests;

/// 子 agent 深度环境变量（父进程注入，子进程继承并 +1）。
const SUBAGENT_DEPTH_ENV: &str = "PI_SUBAGENT_DEPTH";
/// 最大递归深度，超过直接拒绝（防无限递归）。
const MAX_DEPTH: u32 = 3;
/// 默认超时。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// 后台 run 目录名（agent_dir 下）。
const RUNS_DIR: &str = "subagent-runs";
/// 后台 run 序号（run_id = 时间戳-序号）。
static RUN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// subagent 扩展：委派独立任务给子 pi 进程。
pub struct SubagentExtension {
    /// 子 agent 用的 pi 二进制（默认当前可执行文件）。
    pi_binary: String,
    /// 默认模型（None = 不传 --model，用 pi 默认）。
    default_model: Option<String>,
    /// 默认超时。
    timeout: Duration,
}

impl SubagentExtension {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pi_binary: std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "pi".to_string()),
            default_model: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_model(mut self, model: &str) -> Self {
        self.default_model = Some(model.to_string());
        self
    }

    /// 指定子 agent 用的 pi 二进制（测试用假脚本）。
    #[must_use]
    pub fn with_pi_binary(mut self, binary: &str) -> Self {
        self.pi_binary = binary.to_string();
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }
}

impl Default for SubagentExtension {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HookHandler for SubagentExtension {
    fn name(&self) -> &'static str {
        "subagent"
    }

    fn register_tools(&self, tools: &mut ToolRegistry) {
        tools.register(
            "subagent",
            ToolDefinition {
                name: "subagent".into(),
                description: "Delegate a focused, independent task to a child agent. \
                    Use when the task is self-contained and benefits from a fresh context: \
                    code review of a specific change, researching a question, implementing a \
                    well-scoped change, or a second opinion on a plan. The child agent runs \
                    with its own context and tools (read/bash/edit/write) and returns its \
                    final answer. Do NOT use for tasks that depend on the current conversation \
                    context or require coordination with the parent."
                    .into(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "The task for the child agent. Be specific: what to do, what files/areas to focus on, what to return.",
                        },
                        "model": {
                            "type": "string",
                            "description": "Optional model override for the child agent (e.g. a cheaper/faster model for simple tasks).",
                        },
                        "timeoutSeconds": {
                            "type": "number",
                            "description": "Optional timeout in seconds. Default 300.",
                        },
                        "tools": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional tool allowlist for the child agent (e.g. [\"read\",\"bash\"]). Default: read/bash/edit/write.",
                        },
                        "async": {
                            "type": "boolean",
                            "description": "Run in background. Returns a runId immediately; check later with action=status + runId.",
                        },
                        "action": {
                            "type": "string",
                            "enum": ["status"],
                            "description": "Query a background run: action=status + runId.",
                        },
                        "runId": {
                            "type": "string",
                            "description": "Run ID of a background subagent (returned by async:true).",
                        },
                    },
                    "required": ["task"],
                })),
                ..Default::default()
            },
        );
    }

    async fn handle_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        if tool_name != "subagent" {
            return None;
        }

        // ── action=status：查询后台 run ──
        if params.get("action").and_then(|v| v.as_str()) == Some("status") {
            return Some(self.query_run(&params, ctx));
        }

        // ── 深度限制：子 agent 不加载扩展（--no-extensions），但环境变量
        //    仍传递，防止未来子 agent 启用扩展时无限递归。 ──
        let depth: u32 = (ctx.runtime.get_env)(SUBAGENT_DEPTH_ENV.to_string())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if depth >= MAX_DEPTH {
            return Some(error_output(format!(
                "subagent depth limit reached ({MAX_DEPTH}). Cannot delegate further."
            )));
        }

        // ── 解析参数 ──
        let task = params
            .get("task")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if task.is_empty() {
            return Some(error_output("subagent: task is required."));
        }
        let model = params
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or(self.default_model.as_deref())
            .map(|s| s.to_string())
            // 模型继承：父会话当前模型（provider/model），子 agent 默认继承。
            .or_else(|| {
                let m = (ctx.runtime.get_model)();
                if m.is_empty() {
                    None
                } else {
                    Some(m)
                }
            })
            .unwrap_or_default();
        let timeout_secs = params
            .get("timeoutSeconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.timeout.as_secs());
        let timeout_dur = Duration::from_secs(timeout_secs.max(1));
        // 可选工具白名单（逗号分隔字符串或数组），限制子 agent 可用工具。
        let tools: Vec<String> = params
            .get("tools")
            .and_then(|v| match v {
                Value::String(s) => Some(
                    s.split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect(),
                ),
                Value::Array(a) => Some(
                    a.iter()
                        .filter_map(|t| t.as_str())
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();

        // ── 解析 cwd（async 和前台共用）──
        // get_cwd 可能为空（宿主 RuntimeHandle 未正确设置），fallback 到进程 cwd。
        let cwd = (ctx.runtime.get_cwd)();
        let cwd = if cwd.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        } else {
            cwd
        };

        // ── async=true：后台运行，立即返回 run_id ──
        if params.get("async").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Some(self.start_background(
                &task,
                &model,
                &tools,
                &cwd,
                depth,
                timeout_dur,
                ctx,
            ));
        }

        // ── spawn 子 pi 进程 ──
        let mut cmd = Command::new(&self.pi_binary);
        cmd.arg("--mode").arg("json").arg("-p");
        // 模型继承：get_model 返回 "provider/model"，拆开传 --provider + --model
        // （pi-cli 的 --model 不支持 provider 前缀，需分开传）。
        if !model.is_empty() {
            if let Some((provider, model_id)) = model.split_once('/') {
                cmd.arg("--provider").arg(provider);
                cmd.arg("--model").arg(model_id);
            } else {
                cmd.arg("--model").arg(model);
            }
        }
        cmd.arg("--no-session");
        // 子 agent 不加载扩展（防递归：subagent 扩展不会在子进程里再注册）。
        cmd.arg("--no-extensions");
        // 工具白名单（可选）：--tools read,bash,edit,write
        if !tools.is_empty() {
            cmd.arg("--tools").arg(tools.join(","));
        }
        cmd.arg(&task);
        cmd.current_dir(&cwd);
        cmd.env(SUBAGENT_DEPTH_ENV, (depth + 1).to_string());
        cmd.stdout(std::process::Stdio::piped());
        // stderr 重定向到 null：不读 stderr 管道，子进程写满 64KB 管道会
        // 阻塞死锁（错误信息会反映在 stdout 的 message_end error_message）。
        cmd.stderr(std::process::Stdio::null());
        // task 被 drop（父进程退出/取消）时自动 kill 子进程，防孤儿。
        cmd.kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return Some(error_output(format!(
                    "subagent: failed to spawn pi: {e} (binary: {})",
                    self.pi_binary
                )));
            }
        };

        // ── 解析 stdout JSONL，提取最终消息 ──
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => return Some(error_output("subagent: failed to capture child stdout")),
        };
        let mut lines = BufReader::new(stdout).lines();
        let mut final_text = String::new();
        let mut saw_end = false;

        let parse_result = timeout(timeout_dur, async {
            loop {
                let line = match lines.next_line().await {
                    Ok(Some(l)) => l,
                    Ok(None) => break, // EOF
                    Err(_) => break,
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                match v.get("type").and_then(|t| t.as_str()) {
                    Some("message_end") => {
                        if let Some(msg) = v.get("message") {
                            // 只取最终 assistant 消息：role=assistant 且
                            // stop_reason 不是 toolUse（工具调用中间态）。
                            // 子进程多轮工具调用会产生多个 message_end，
                            // 最后一个可能是工具结果而非最终结论。
                            let role = msg.get("role").and_then(|r| r.as_str());
                            let stop_reason = msg.get("stop_reason").and_then(|s| s.as_str());
                            if role == Some("assistant") && stop_reason != Some("toolUse") {
                                if let Some(text) = extract_message_text(msg) {
                                    final_text = text;
                                }
                            }
                        }
                    }
                    // agent_end 表示 agent 循环结束，最终消息已收到。
                    // 不依赖后续的 `end` 事件（print mode 在流式错误时可能
                    // 卡在 wait_for_idle 不输出 end，子进程也不退出）。
                    Some("agent_end") => {
                        saw_end = true;
                        break;
                    }
                    Some("end") => {
                        saw_end = true;
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await;

        // ── 收尾：kill 子进程（若还在跑） ──
        let _ = child.kill().await;
        let _ = child.wait().await;

        match parse_result {
            Err(_) => Some(error_output(format!(
                "subagent timed out after {timeout_secs}s. Partial output: {}",
                truncate(&final_text, 2000)
            ))),
            Ok(()) => {
                if final_text.is_empty() {
                    Some(error_output(
                        "subagent produced no output (child agent failed or returned empty).",
                    ))
                } else {
                    Some(ToolCallOutput {
                        content: vec![json!({ "type": "text", "text": final_text })],
                        details: Some(json!({ "sawEnd": saw_end })),
                        is_error: false,
                        terminate: None,
                    })
                }
            }
        }
    }
}

impl SubagentExtension {
    /// 查询后台 run 状态（action=status + runId）。
    fn query_run(&self, params: &Value, ctx: &ExtensionContext) -> ToolCallOutput {
        let run_id = params
            .get("runId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if run_id.is_empty() {
            return error_output("subagent: runId is required for action=status.");
        }
        let agent_dir = (ctx.runtime.get_agent_dir)();
        let status_path = std::path::Path::new(&agent_dir)
            .join(RUNS_DIR)
            .join(run_id)
            .join("status.json");
        match std::fs::read_to_string(&status_path) {
            Ok(s) => ToolCallOutput {
                content: vec![json!({ "type": "text", "text": s })],
                details: None,
                is_error: false,
                terminate: None,
            },
            Err(_) => error_output(format!("subagent: run {run_id} not found.")),
        }
    }

    /// 后台运行：spawn 子进程不等待，返回 run_id；监控 task 在子进程
    /// 退出后解析 output.jsonl 并更新 status.json。
    fn start_background(
        &self,
        task: &str,
        model: &str,
        tools: &[String],
        cwd: &str,
        depth: u32,
        timeout_dur: Duration,
        ctx: &ExtensionContext,
    ) -> ToolCallOutput {
        // ── run_id + 目录 ──
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = RUN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let run_id = format!("{ts}-{seq}");
        let agent_dir = (ctx.runtime.get_agent_dir)();
        let run_dir = std::path::Path::new(&agent_dir).join(RUNS_DIR).join(&run_id);
        if let Err(e) = std::fs::create_dir_all(&run_dir) {
            return error_output(format!("subagent: failed to create run dir: {e}"));
        }

        // ── 写 status.json: running ──
        let status = json!({
            "status": "running",
            "runId": run_id,
            "startedAt": ts,
        });
        write_status(&run_dir, &status, &ctx.ui.notify);

        // ── spawn 子进程，stdout 重定向到 output.jsonl ──
        let mut cmd = Command::new(&self.pi_binary);
        cmd.arg("--mode").arg("json").arg("-p");
        // 模型继承："provider/model" 拆开传 --provider + --model。
        if !model.is_empty() {
            if let Some((provider, model_id)) = model.split_once('/') {
                cmd.arg("--provider").arg(provider);
                cmd.arg("--model").arg(model_id);
            } else {
                cmd.arg("--model").arg(model);
            }
        }
        cmd.arg("--no-session");
        cmd.arg("--no-extensions");
        if !tools.is_empty() {
            cmd.arg("--tools").arg(tools.join(","));
        }
        cmd.arg(task);
        cmd.current_dir(cwd);
        cmd.env(SUBAGENT_DEPTH_ENV, (depth + 1).to_string());
        // output.jsonl 创建失败：报错而不是静默丢弃 stdout。
        let output_file = match std::fs::File::create(run_dir.join("output.jsonl")) {
            Ok(f) => f,
            Err(e) => {
                let status = json!({
                    "status": "error",
                    "runId": run_id,
                    "error": format!("failed to create output.jsonl: {e}"),
                });
                write_status(&run_dir, &status, &ctx.ui.notify);
                return error_output(format!(
                    "subagent: failed to create output.jsonl: {e}"
                ));
            }
        };
        cmd.stdout(std::process::Stdio::from(output_file));
        cmd.stderr(std::process::Stdio::null());
        // task 被 drop（父进程退出/取消）时自动 kill 子进程，防孤儿。
        cmd.kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let status = json!({
                    "status": "error",
                    "runId": run_id,
                    "error": format!("failed to spawn pi: {e}"),
                });
                write_status(&run_dir, &status, &ctx.ui.notify);
                return error_output(format!(
                    "subagent: failed to spawn pi: {e} (binary: {})",
                    self.pi_binary
                ));
            }
        };

        // ── 后台监控 task：等子进程退出 → 解析 output.jsonl → 更新 status ──
        // 超时后 kill 子进程（print mode 在流式错误时可能卡在 wait_for_idle
        // 不退出），status 标记为 timeout。
        let run_dir_owned = run_dir.clone();
        let run_id_owned = run_id.clone();
        let notify = ctx.ui.notify.clone();
        tokio::spawn(async move {
            let wait_result = tokio::time::timeout(timeout_dur, child.wait()).await;
            let timed_out = wait_result.is_err();
            if timed_out {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            let output = std::fs::read_to_string(run_dir_owned.join("output.jsonl"))
                .unwrap_or_default();
            let final_text = parse_output_jsonl(&output);
            let finished_ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let status = if timed_out {
                json!({
                    "status": "timeout",
                    "runId": run_id_owned,
                    "finishedAt": finished_ts,
                    "output": final_text,
                })
            } else if final_text.is_empty() {
                json!({
                    "status": "error",
                    "runId": run_id_owned,
                    "finishedAt": finished_ts,
                    "output": final_text,
                })
            } else {
                json!({
                    "status": "done",
                    "runId": run_id_owned,
                    "finishedAt": finished_ts,
                    "output": final_text,
                })
            };
            write_status(&run_dir_owned, &status, &notify);
        });

        ToolCallOutput {
            content: vec![json!({ "type": "text", "text": format!(
                "Subagent started in background. Run ID: {run_id}. Check with subagent action=status runId={run_id}."
            ) })],
            details: Some(json!({
                "runId": run_id,
                "statusFile": run_dir.to_string_lossy(),
            })),
            is_error: false,
            terminate: None,
        }
    }
}

/// 从序列化的 AgentMessage 提取文本内容（content 数组里 type=="text" 的 text 拼接）。
fn extract_message_text(msg: &Value) -> Option<String> {
    let content = msg.get("content")?.as_array()?;
    let mut parts = Vec::new();
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                parts.push(t.to_string());
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// 从 output.jsonl 提取最终 assistant 消息（最后一个 role=assistant 且
/// stop_reason != toolUse 的 message_end 的文本）。
fn parse_output_jsonl(output: &str) -> String {
    let mut final_text = String::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("message_end") {
            if let Some(msg) = v.get("message") {
                let role = msg.get("role").and_then(|r| r.as_str());
                let stop_reason = msg.get("stop_reason").and_then(|s| s.as_str());
                if role == Some("assistant") && stop_reason != Some("toolUse") {
                    if let Some(text) = extract_message_text(msg) {
                        final_text = text;
                    }
                }
            }
        }
    }
    final_text
}

fn error_output(text: impl Into<String>) -> ToolCallOutput {
    let text = text.into();
    ToolCallOutput {
        content: vec![json!({ "type": "text", "text": text })],
        details: None,
        is_error: true,
        terminate: None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// 写 status.json；失败时通过扩展 UI notify 上报（TUI 模式显示为系统消息，
/// CLI 模式落到 stderr），不静默吞错——否则 query_run 会报 "run not found"
/// 掩盖真实原因。
#[allow(clippy::type_complexity)]
fn write_status(
    run_dir: &std::path::Path,
    status: &Value,
    notify: &Arc<dyn Fn(&str, &Value) + Send + Sync>,
) {
    let path = run_dir.join("status.json");
    let body = serde_json::to_string_pretty(status).unwrap_or_default();
    if let Err(e) = std::fs::write(&path, body) {
        notify(
            &format!("[subagent] failed to write {}: {e}", path.display()),
            &json!({}),
        );
    }
}
