//! pi-subagent — subagent 工具，委派独立任务给子 agent。
//!
//! 核心版（对应 pi-subagents 的 subagent 工具，去掉后台运行/工作流/控制通道）：
//! - 注册 `subagent` 工具，LLM 自主决定是否调用（工具描述引导，同 Claude Code）
//! - 执行时 spawn 子 pi 进程（`pi --mode json -p --model <m> --no-session <task>`）
//! - 解析子进程 stdout JSONL 事件流，提取最终 assistant 消息
//! - 超时 kill、深度限制（防无限递归）

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
            .unwrap_or("");
        let timeout_secs = params
            .get("timeoutSeconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.timeout.as_secs());
        let timeout_dur = Duration::from_secs(timeout_secs.max(1));

        // ── spawn 子 pi 进程 ──
        let cwd = (ctx.runtime.get_cwd)();
        let mut cmd = Command::new(&self.pi_binary);
        cmd.arg("--mode").arg("json").arg("-p");
        if !model.is_empty() {
            cmd.arg("--model").arg(model);
        }
        cmd.arg("--no-session");
        // 子 agent 不加载扩展（防递归：subagent 扩展不会在子进程里再注册）。
        cmd.arg("--no-extensions");
        cmd.arg(&task);
        cmd.current_dir(&cwd);
        cmd.env(SUBAGENT_DEPTH_ENV, (depth + 1).to_string());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

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
        let stdout = child.stdout.take().expect("stdout piped");
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
                            if let Some(text) = extract_message_text(msg) {
                                final_text = text;
                            }
                        }
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
        out.push_str("…");
        out
    }
}
