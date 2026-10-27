//! pi-subagent — subagent 工具：把任务委派给专用子 agent（隔离上下文）。
//!
//! 对齐 pi-subagents / pi 上游 `examples/extensions/subagent`：
//! - **agent 定义**：`{agent_dir}/agents/*.md` + 最近的 `/.pi-rs/agents/*.md`，
//!   frontmatter 给 `name`/`description`/`tools`/`model`，正文作 system prompt
//! - **三种模式**（每次调用恰好一种）：
//!   - single：`{ agent, task }`
//!   - parallel：`{ tasks: [{agent, task}] }`（上限 8 个任务，4 路并发）
//!   - chain：`{ chain: [{agent, task}] }`（顺序执行，`{previous}` 代入上一步输出）
//! - 每个子 agent 都是独立 `pi --mode json -p --no-session` 进程，解析其
//!   JSONL 事件流（`message_end`）拿消息、usage、stopReason
//! - **失败判定**：`exitCode != 0` 或 `stopReason ∈ {error, aborted}`
//! - usage 聚合（turns / ↑in ↓out / cache / $cost / ctx）+ 每任务输出 50KB 上限
//! - `onUpdate` 进度上报（每条子进程消息推一次 partial + details）
//! - abort 传播：`invocation.abort` 触发时立刻杀掉子进程，结果标 `aborted`
//! - 自定义渲染（`renderCall`/`renderResult`）：折叠态条目、展开态 Markdown
//!   终稿、`─── Step N ───` 分节、usage 脚注，见 [`render_tool`]
//!
//! 剩余差异（渲染槽位形态、SIGKILL 直杀、事件源只有 `message_end`）见
//! `crates/pi-extensions/DEVIATIONS.md`。

mod agents;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use async_trait::async_trait;
use pi_extension_api::{
    ExtensionContext, HookHandler, ToolCallOutput, ToolDefinition, ToolInvocation, ToolRegistry,
    ToolRenderBlock, ToolRenderContext, ToolRenderOutput, ToolRenderSpan, ToolTextStyle,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

pub use agents::{
    discover_agents, find_agent, format_agent_list, AgentConfig, AgentDiscovery, AgentScope,
    AgentSource, PROJECT_CONFIG_DIR,
};

/// 并行模式下单次调用允许的最大任务数（TS `MAX_PARALLEL_TASKS`）。
const MAX_PARALLEL_TASKS: usize = 8;
/// 并行模式的最大并发度（TS `MAX_CONCURRENCY`）。
const MAX_CONCURRENCY: usize = 4;
/// 并行模式下单个任务回传给父模型的输出上限（TS `PER_TASK_OUTPUT_CAP`）。
const PER_TASK_OUTPUT_CAP: usize = 50 * 1024;
/// 子进程 stderr 的保留上限（继续读完但只留这么多，防止内存被日志吃满）。
const MAX_STDERR_BYTES: usize = 256 * 1024;

/// subagent 扩展。
pub struct SubagentExtension {
    /// 子 agent 用的 pi 二进制（默认当前可执行文件）。
    pi_binary: String,
}

impl SubagentExtension {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pi_binary: pi_invocation(),
        }
    }

    /// 指定子 agent 用的 pi 二进制（测试用假脚本）。
    #[must_use]
    pub fn with_pi_binary(mut self, binary: &str) -> Self {
        self.pi_binary = binary.to_string();
        self
    }
}

impl Default for SubagentExtension {
    fn default() -> Self {
        Self::new()
    }
}

/// 派发会话的默认配置：agent 未定义 `model` 时继承父会话的模型与
/// thinking level（TS `dispatchDefaults`）。
#[derive(Clone, Debug, Default)]
struct DispatchDefaults {
    model: Option<String>,
    thinking_level: Option<String>,
}

/// 累计 usage（TS `UsageStats`）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct UsageStats {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
    context_tokens: u64,
    turns: u32,
}

impl UsageStats {
    fn to_json(self) -> Value {
        json!({
            "input": self.input,
            "output": self.output,
            "cacheRead": self.cache_read,
            "cacheWrite": self.cache_write,
            "cost": self.cost,
            "contextTokens": self.context_tokens,
            "turns": self.turns,
        })
    }

    /// 对齐 TS `formatUsageStats`：`N turns ↑in ↓out Rcache Wcache $cost ctx:N model`。
    ///
    /// 原版只在 TUI 渲染里展示 usage（模型可见文本里没有），这里给自定义
    /// 渲染（[`render_tool`]）的脚注用。
    fn format(self, model: Option<&str>) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.turns > 0 {
            parts.push(format!(
                "{} turn{}",
                self.turns,
                if self.turns > 1 { "s" } else { "" }
            ));
        }
        if self.input > 0 {
            parts.push(format!("↑{}", format_tokens(self.input)));
        }
        if self.output > 0 {
            parts.push(format!("↓{}", format_tokens(self.output)));
        }
        if self.cache_read > 0 {
            parts.push(format!("R{}", format_tokens(self.cache_read)));
        }
        if self.cache_write > 0 {
            parts.push(format!("W{}", format_tokens(self.cache_write)));
        }
        if self.cost > 0.0 {
            parts.push(format!("${:.4}", self.cost));
        }
        if self.context_tokens > 0 {
            parts.push(format!("ctx:{}", format_tokens(self.context_tokens)));
        }
        if let Some(model) = model.filter(|m| !m.is_empty()) {
            parts.push(model.to_string());
        }
        parts.join(" ")
    }
}

fn format_tokens(count: u64) -> String {
    if count < 1000 {
        count.to_string()
    } else if count < 10_000 {
        format!("{:.1}k", count as f64 / 1000.0)
    } else if count < 1_000_000 {
        format!("{}k", count / 1000)
    } else {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    }
}

/// 一个子 agent 的运行结果（TS `SingleResult`）。
#[derive(Clone, Debug)]
struct SingleResult {
    agent: String,
    /// `user` / `project` / `unknown`。
    agent_source: String,
    task: String,
    exit_code: i32,
    /// 子进程 `message_end` 消息（原样保留，供渲染/诊断）。
    messages: Vec<Value>,
    stderr: String,
    usage: UsageStats,
    model: Option<String>,
    stop_reason: Option<String>,
    error_message: Option<String>,
    step: Option<usize>,
}

impl SingleResult {
    fn new(
        agent: &str,
        agent_source: &str,
        task: &str,
        model: Option<String>,
        step: Option<usize>,
    ) -> Self {
        Self {
            agent: agent.to_string(),
            agent_source: agent_source.to_string(),
            task: task.to_string(),
            exit_code: 0,
            messages: Vec::new(),
            stderr: String::new(),
            usage: UsageStats::default(),
            model,
            stop_reason: None,
            error_message: None,
            step,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "agentSource": self.agent_source,
            "task": self.task,
            "exitCode": self.exit_code,
            "messages": self.messages,
            "stderr": self.stderr,
            "usage": self.usage.to_json(),
            "model": self.model,
            "stopReason": self.stop_reason,
            "errorMessage": self.error_message,
            "step": self.step,
        })
    }
}

/// 失败判定（TS `isFailedResult`）。
fn is_failed_result(result: &SingleResult) -> bool {
    result.exit_code != 0 || matches!(result.stop_reason.as_deref(), Some("error" | "aborted"))
}

/// 最后一条 assistant 消息的第一段文本（TS `getFinalOutput`）。
fn get_final_output(messages: &[Value]) -> String {
    for message in messages.iter().rev() {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    return text.to_string();
                }
            }
        }
    }
    String::new()
}

/// 失败时优先返回错误信息/诊断输出（TS `getResultOutput`）。
fn get_result_output(result: &SingleResult) -> String {
    if is_failed_result(result) {
        let candidates = [
            result.error_message.clone().unwrap_or_default(),
            result.stderr.clone(),
            get_final_output(&result.messages),
        ];
        for candidate in candidates {
            if !candidate.trim().is_empty() {
                return candidate;
            }
        }
        return "(no output)".to_string();
    }
    let output = get_final_output(&result.messages);
    if output.is_empty() {
        "(no output)".to_string()
    } else {
        output
    }
}

/// 按字节上限截断（TS `truncateParallelOutput`），并附截断说明。
fn truncate_parallel_output(output: &str) -> String {
    if output.len() <= PER_TASK_OUTPUT_CAP {
        return output.to_string();
    }
    let mut end = PER_TASK_OUTPUT_CAP;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = &output[..end];
    let omitted = output.len() - truncated.len();
    format!(
        "{truncated}\n\n[Output truncated: {omitted} bytes omitted. Full output preserved in tool details.]"
    )
}

/// 一个模式步骤（`tasks` / `chain` 元素）。
#[derive(Clone, Debug)]
struct TaskStep {
    agent: String,
    task: String,
    cwd: Option<String>,
}

impl TaskStep {
    fn parse(value: &Value) -> Option<Self> {
        let agent = value.get("agent").and_then(Value::as_str)?.to_string();
        let task = value.get("task").and_then(Value::as_str)?.to_string();
        Some(Self {
            agent,
            task,
            cwd: value
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }
}

fn parse_steps(value: Option<&Value>) -> Vec<TaskStep> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(TaskStep::parse).collect())
        .unwrap_or_default()
}

/// 子 agent 可执行文件：默认复用当前能执行 pi 的入口
/// （TS `getPiInvocation` 的 Rust 版本——pi-rs 是单二进制，直接用自身路径）。
fn pi_invocation() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "pi".to_string())
}

/// 工具描述里展示的用户 agent 目录（对齐 TS
/// `path.join(getAgentDir(), "agents")`；注册期拿不到 runtime，用同款环境变量
/// 优先级推算）。
fn agent_dir_hint() -> String {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        if !dir.is_empty() {
            return dir;
        }
    }
    if let Ok(home) = std::env::var("PI_RS_HOME") {
        if !home.is_empty() {
            return format!("{home}/agent");
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if home.is_empty() {
        format!("{PROJECT_CONFIG_DIR}/agent")
    } else {
        format!("{home}/{PROJECT_CONFIG_DIR}/agent")
    }
}

#[async_trait]
impl HookHandler for SubagentExtension {
    fn name(&self) -> &'static str {
        "subagent"
    }

    fn register_tools(&self, tools: &mut ToolRegistry) {
        let agents_dir = format!("{}/agents", agent_dir_hint());
        let description = format!(
            "Delegate tasks to specialized subagents with isolated context. \
             Modes: single (agent + task), parallel (tasks array), chain (sequential with \
             {{previous}} placeholder). Default agent scope is \"user\" (from {agents_dir}). \
             To enable project-local agents in {PROJECT_CONFIG_DIR}/agents, set \
             agentScope: \"both\" (or \"project\")."
        );

        tools.register(
            "subagent",
            ToolDefinition {
                name: "subagent".into(),
                label: Some("Subagent".into()),
                description,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "agent": {
                            "type": "string",
                            "description": "Name of the agent to invoke (for single mode)",
                        },
                        "task": {
                            "type": "string",
                            "description": "Task to delegate (for single mode)",
                        },
                        "tasks": {
                            "type": "array",
                            "description": "Array of {agent, task} for parallel execution",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "agent": {
                                        "type": "string",
                                        "description": "Name of the agent to invoke",
                                    },
                                    "task": {
                                        "type": "string",
                                        "description": "Task to delegate to the agent",
                                    },
                                    "cwd": {
                                        "type": "string",
                                        "description": "Working directory for the agent process",
                                    },
                                },
                                "required": ["agent", "task"],
                            },
                        },
                        "chain": {
                            "type": "array",
                            "description": "Array of {agent, task} for sequential execution",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "agent": {
                                        "type": "string",
                                        "description": "Name of the agent to invoke",
                                    },
                                    "task": {
                                        "type": "string",
                                        "description": "Task with optional {previous} placeholder for prior output",
                                    },
                                    "cwd": {
                                        "type": "string",
                                        "description": "Working directory for the agent process",
                                    },
                                },
                                "required": ["agent", "task"],
                            },
                        },
                        "agentScope": {
                            "type": "string",
                            "enum": ["user", "project", "both"],
                            "default": "user",
                            "description": "Which agent directories to use. Default: \"user\". Use \"both\" to include project-local agents.",
                        },
                        "confirmProjectAgents": {
                            "type": "boolean",
                            "default": true,
                            "description": "Prompt before running project-local agents. Default: true.",
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Working directory for the agent process (single mode)",
                        },
                    },
                })),
                // 自定义渲染（对齐 TS renderCall / renderResult）。
                render: Some(Arc::new(render_tool)),
                ..Default::default()
            },
        );
    }

    async fn handle_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        ctx: &ExtensionContext,
        invocation: &ToolInvocation,
    ) -> Option<ToolCallOutput> {
        if tool_name != "subagent" {
            return None;
        }
        Some(self.execute(&params, ctx, invocation).await)
    }
}

impl SubagentExtension {
    async fn execute(
        &self,
        params: &Value,
        ctx: &ExtensionContext,
        invocation: &ToolInvocation,
    ) -> ToolCallOutput {
        let scope = params
            .get("agentScope")
            .and_then(Value::as_str)
            .and_then(AgentScope::parse)
            .unwrap_or_default();

        // 派发默认值：父会话当前模型 + thinking level（agent 未定义 model 时继承）。
        let defaults = DispatchDefaults {
            model: non_empty((ctx.runtime.get_model)()),
            thinking_level: non_empty((ctx.runtime.get_thinking_level)()),
        };

        let cwd = {
            let cwd = (ctx.runtime.get_cwd)();
            if cwd.is_empty() {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| ".".to_string())
            } else {
                cwd
            }
        };

        let agent_dir = (ctx.runtime.get_agent_dir)();
        let user_agents_dir = std::path::Path::new(&agent_dir).join("agents");
        let discovery = discover_agents(&user_agents_dir, &cwd, scope);
        let agents = discovery.agents.clone();

        // ── 模式判定：三种模式必须恰好出现一种 ──
        let chain = parse_steps(params.get("chain"));
        let tasks = parse_steps(params.get("tasks"));
        let single_agent = params
            .get("agent")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let single_task = params
            .get("task")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let has_chain = !chain.is_empty();
        let has_tasks = !tasks.is_empty();
        let has_single = single_agent.is_some() && single_task.is_some();
        let mode_count = u8::from(has_chain) + u8::from(has_tasks) + u8::from(has_single);

        let available = || {
            let list = agents
                .iter()
                .map(|a| format!("{} ({})", a.name, a.source.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            if list.is_empty() {
                "none".to_string()
            } else {
                list
            }
        };

        if mode_count != 1 {
            return text_output(
                format!(
                    "Invalid parameters. Provide exactly one mode.\nAvailable agents: {}",
                    available()
                ),
                details("single", scope, &discovery.project_agents_dir, &[]),
                false,
            );
        }

        // ── 项目级 agent 需要确认（原版：UI 且项目未受信任时）──
        let confirm_project_agents = params
            .get("confirmProjectAgents")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if scope != AgentScope::User
            && confirm_project_agents
            && ctx.has_ui
            && !ctx.is_project_trusted
        {
            let mut requested: Vec<&str> = Vec::new();
            for step in chain.iter().chain(tasks.iter()) {
                requested.push(step.agent.as_str());
            }
            if let Some(agent) = single_agent {
                requested.push(agent);
            }
            let project_agents: Vec<&AgentConfig> = requested
                .iter()
                .filter_map(|name| find_agent(&agents, name))
                .filter(|a| a.source == AgentSource::Project)
                .collect();
            if !project_agents.is_empty() {
                let names = project_agents
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                let dir = discovery
                    .project_agents_dir
                    .clone()
                    .unwrap_or_else(|| "(unknown)".to_string());
                // 条件与原版一致：有 UI 且项目未受信任才确认（受信任项目
                // 直接跑）。
                let ok = (ctx.ui.confirm)(
                    "Run project-local agents?",
                    &Value::String(format!(
                        "Agents: {names}\nSource: {dir}\n\nProject agents are \
                         repo-controlled. Only continue for trusted repositories."
                    )),
                );
                if !ok {
                    // 原版这里用当前模式（chain / parallel / single）。
                    let mode = if has_chain {
                        "chain"
                    } else if has_tasks {
                        "parallel"
                    } else {
                        "single"
                    };
                    return text_output(
                        "Canceled: project-local agents not approved.".to_string(),
                        details(mode, scope, &discovery.project_agents_dir, &[]),
                        false,
                    );
                }
            }
        }

        // ── chain：顺序执行，`{previous}` 代入上一步输出，失败即停 ──
        if has_chain {
            let mut results: Vec<SingleResult> = Vec::new();
            let mut previous_output = String::new();
            // 进度上报：已完成步骤 + 当前步骤（对齐 TS `chainUpdate`）。
            let completed = Arc::new(std::sync::Mutex::new(Vec::<SingleResult>::new()));
            for (index, step) in chain.iter().enumerate() {
                let task = step.task.replace("{previous}", &previous_output);
                let progress = {
                    let completed = Arc::clone(&completed);
                    let invocation = invocation.clone();
                    let project_dir = discovery.project_agents_dir.clone();
                    Some(Arc::new(move |current: &SingleResult| {
                        let mut all = completed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone();
                        all.push(current.clone());
                        invocation.update(&running_output("chain", scope, &project_dir, &all));
                    }) as Arc<ProgressFn>)
                };
                let result = self
                    .run_single_agent(
                        &cwd,
                        &defaults,
                        &agents,
                        &step.agent,
                        &task,
                        step.cwd.as_deref(),
                        Some(index + 1),
                        invocation,
                        progress,
                    )
                    .await;
                let failed = is_failed_result(&result);
                completed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(result.clone());
                results.push(result);
                if failed {
                    let error_output = get_result_output(&results[index]);
                    return text_output(
                        format!(
                            "Chain stopped at step {} ({}): {error_output}",
                            index + 1,
                            step.agent
                        ),
                        details("chain", scope, &discovery.project_agents_dir, &results),
                        true,
                    );
                }
                previous_output = get_final_output(&results[index].messages);
            }
            let final_output = get_final_output(&results[results.len() - 1].messages);
            return text_output(
                if final_output.is_empty() {
                    "(no output)".to_string()
                } else {
                    final_output
                },
                details("chain", scope, &discovery.project_agents_dir, &results),
                false,
            );
        }

        // ── parallel：≤8 任务，4 路并发 ──
        if has_tasks {
            if tasks.len() > MAX_PARALLEL_TASKS {
                return text_output(
                    format!(
                        "Too many parallel tasks ({}). Max is {MAX_PARALLEL_TASKS}.",
                        tasks.len()
                    ),
                    details("parallel", scope, &discovery.project_agents_dir, &[]),
                    false,
                );
            }

            let results = self
                .run_parallel(
                    &cwd, &defaults, &agents, &tasks, invocation, scope, &discovery,
                )
                .await;

            // 中止不是"某个任务失败"而是整次调用被取消：原版 `runSingleAgent`
            // 抛错让 `Promise.all` 拒绝，工具调用整体报 `Subagent was aborted`。
            // 保持同样语义（否则父模型会把"被取消"当成"任务失败"继续干）。
            if results
                .iter()
                .any(|r| r.stop_reason.as_deref() == Some("aborted"))
            {
                return text_output(
                    "Subagent was aborted".to_string(),
                    details("parallel", scope, &discovery.project_agents_dir, &results),
                    true,
                );
            }

            let success_count = results.iter().filter(|r| !is_failed_result(r)).count();
            let summaries: Vec<String> = results
                .iter()
                .map(|r| {
                    let output = truncate_parallel_output(&get_result_output(r));
                    // TS 用 `stopReason !== "end"` 才显示原因；pi-rs 的
                    // 正常结束值是 "stop"。
                    let reason = r
                        .stop_reason
                        .as_deref()
                        .filter(|s| *s != "stop" && !s.is_empty())
                        .map(|s| format!(" ({s})"))
                        .unwrap_or_default();
                    let status = if is_failed_result(r) {
                        format!("failed{reason}")
                    } else {
                        "completed".to_string()
                    };
                    format!("### [{}] {status}\n\n{output}", r.agent)
                })
                .collect();
            return text_output(
                format!(
                    "Parallel: {success_count}/{} succeeded\n\n{}",
                    results.len(),
                    summaries.join("\n\n---\n\n")
                ),
                details("parallel", scope, &discovery.project_agents_dir, &results),
                false,
            );
        }

        // ── single ──
        let agent_name = single_agent.unwrap_or_default();
        let task = single_task.unwrap_or_default();
        let step_cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let progress = {
            let invocation = invocation.clone();
            let project_dir = discovery.project_agents_dir.clone();
            Some(Arc::new(move |current: &SingleResult| {
                invocation.update(&running_output(
                    "single",
                    scope,
                    &project_dir,
                    std::slice::from_ref(current),
                ));
            }) as Arc<ProgressFn>)
        };
        let result = self
            .run_single_agent(
                &cwd, &defaults, &agents, agent_name, task, step_cwd, None, invocation, progress,
            )
            .await;
        let failed = is_failed_result(&result);
        let output = if failed {
            let reason = result
                .stop_reason
                .clone()
                .unwrap_or_else(|| "failed".to_string());
            format!("Agent {reason}: {}", get_result_output(&result))
        } else {
            let output = get_final_output(&result.messages);
            if output.is_empty() {
                "(no output)".to_string()
            } else {
                output
            }
        };
        text_output(
            output,
            details("single", scope, &discovery.project_agents_dir, &[result]),
            failed,
        )
    }

    /// 并行执行任务（对齐 TS `mapWithConcurrencyLimit` + `MAX_CONCURRENCY`）。
    async fn run_parallel(
        &self,
        cwd: &str,
        defaults: &DispatchDefaults,
        agents: &[AgentConfig],
        tasks: &[TaskStep],
        invocation: &ToolInvocation,
        scope: AgentScope,
        discovery: &AgentDiscovery,
    ) -> Vec<SingleResult> {
        let agents = Arc::new(agents.to_vec());
        let total = tasks.len();
        if total == 0 {
            return Vec::new();
        }
        // 占位结果（exitCode = -1 = 未完成，对齐 TS）
        let mut results: Vec<Option<SingleResult>> = (0..total)
            .map(|i| {
                let mut placeholder =
                    SingleResult::new(&tasks[i].agent, "unknown", &tasks[i].task, None, None);
                placeholder.exit_code = -1;
                Some(placeholder)
            })
            .collect();
        let slots = Arc::new(std::sync::Mutex::new(results.clone()));
        let mut next = 0usize;
        let mut set: tokio::task::JoinSet<(usize, SingleResult)> = tokio::task::JoinSet::new();
        let limit = MAX_CONCURRENCY.min(total);

        let spawn_task = |set: &mut tokio::task::JoinSet<(usize, SingleResult)>, index: usize| {
            let pi_binary = self.pi_binary.clone();
            let cwd = cwd.to_string();
            let defaults = defaults.clone();
            let agents = Arc::clone(&agents);
            let step = tasks[index].clone();
            let invocation = invocation.clone();
            let slots = Arc::clone(&slots);
            let project_dir = discovery.project_agents_dir.clone();
            set.spawn(async move {
                let runner = SubagentExtension { pi_binary };
                // 进度上报：把当前任务写回槽位并推送整体快照（对齐 TS
                // `emitParallelUpdate`）。
                let update_invocation = invocation.clone();
                let progress: Arc<ProgressFn> = Arc::new(move |current: &SingleResult| {
                    let snapshot = {
                        let mut guard = slots
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        guard[index] = Some(current.clone());
                        guard.clone()
                    };
                    let all: Vec<SingleResult> = snapshot.into_iter().flatten().collect();
                    let running = all.iter().filter(|r| r.exit_code == -1).count();
                    let done = all.len() - running;
                    let text = if running > 0 {
                        format!("Parallel: {done}/{} done, {running} running...", all.len())
                    } else {
                        format!("Parallel: {done}/{} done", all.len())
                    };
                    update_invocation.update(&ToolCallOutput {
                        content: vec![json!({ "type": "text", "text": text })],
                        details: Some(details("parallel", scope, &project_dir, &all)),
                        is_error: false,
                        terminate: None,
                    });
                });
                let result = runner
                    .run_single_agent(
                        &cwd,
                        &defaults,
                        &agents,
                        &step.agent,
                        &step.task,
                        step.cwd.as_deref(),
                        None,
                        &invocation,
                        Some(progress),
                    )
                    .await;
                (index, result)
            });
        };

        while next < limit {
            spawn_task(&mut set, next);
            next += 1;
        }
        while let Some(joined) = set.join_next().await {
            // join 失败（任务 panic）时保留占位，下面补成失败结果。
            if let Ok((index, result)) = joined {
                results[index] = Some(result);
            }
            if next < total {
                spawn_task(&mut set, next);
                next += 1;
            }
        }

        results
            .into_iter()
            .enumerate()
            .map(|(index, result)| {
                result.unwrap_or_else(|| {
                    let mut failed = SingleResult::new(
                        &tasks[index].agent,
                        "unknown",
                        &tasks[index].task,
                        None,
                        None,
                    );
                    failed.exit_code = 1;
                    failed.stderr = "subagent task failed to run".to_string();
                    failed
                })
            })
            .collect()
    }

    /// 跑一个子 agent（对齐 TS `runSingleAgent`）。
    ///
    /// `invocation` 提供中止信号：Ctrl+C 时立刻杀掉子进程并把结果标记为
    /// aborted（原版 `AbortSignal` → SIGTERM/SIGKILL 的等价物）。
    /// `progress` 在每收到一条子进程消息后调用，用于 `onUpdate` 上报。
    #[allow(clippy::too_many_arguments)]
    async fn run_single_agent(
        &self,
        default_cwd: &str,
        defaults: &DispatchDefaults,
        agents: &[AgentConfig],
        agent_name: &str,
        task: &str,
        cwd: Option<&str>,
        step: Option<usize>,
        invocation: &ToolInvocation,
        progress: Option<Arc<ProgressFn>>,
    ) -> SingleResult {
        let Some(agent) = find_agent(agents, agent_name) else {
            let available = agents
                .iter()
                .map(|a| format!("\"{}\"", a.name))
                .collect::<Vec<_>>()
                .join(", ");
            let mut result = SingleResult::new(agent_name, "unknown", task, None, step);
            result.exit_code = 1;
            result.stderr = format!(
                "Unknown agent: \"{agent_name}\". Available agents: {}.",
                if available.is_empty() {
                    "none"
                } else {
                    &available
                }
            );
            return result;
        };

        // agent 未定义 model 时继承派发会话的模型与 thinking level。
        let inherits_dispatch_config = agent.model.is_none();
        let model = agent.model.clone().or_else(|| defaults.model.clone());
        let mut result =
            SingleResult::new(agent_name, agent.source.as_str(), task, model.clone(), step);

        let mut args: Vec<String> = vec![
            "--mode".to_string(),
            "json".to_string(),
            "-p".to_string(),
            "--no-session".to_string(),
        ];
        if let Some(model) = &model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if inherits_dispatch_config {
            if let Some(level) = &defaults.thinking_level {
                args.push("--thinking".to_string());
                args.push(level.clone());
            }
        }
        if let Some(tools) = agent.tools.as_ref().filter(|t| !t.is_empty()) {
            args.push("--tools".to_string());
            args.push(tools.join(","));
        }
        // 原版把 agent 正文写临时文件后走 `--append-system-prompt <path>`；
        // pi-rs 的 `--append-prompt` 收字面文本，直接传正文（同样的语义）。
        if !agent.system_prompt.trim().is_empty() {
            args.push("--append-prompt".to_string());
            args.push(agent.system_prompt.clone());
        }
        args.push(format!("Task: {task}"));

        let mut command = Command::new(&self.pi_binary);
        command.args(&args);
        command.current_dir(cwd.unwrap_or(default_cwd));
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        // 没有 abort signal 可用（扩展 API 未暴露），task 被 drop 时杀掉子进程
        // 是唯一的兜底，防孤儿进程。
        command.kill_on_drop(true);

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                result.exit_code = 1;
                result.stderr = format!("Failed to spawn pi: {e} (binary: {})", self.pi_binary);
                return result;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        // stderr 必须持续读走，否则子进程写满管道会阻塞；保留量设上限，避免
        // 一个刷日志的子进程把内存吃满（原版是无上限字符串拼接）。
        //
        // 结果写进共享缓冲而不是让任务返回：子进程退出后孙进程可能仍持有
        // 管道写端（例如子 agent 起过的 bash 留下的后台进程），等 EOF 会把
        // 工具调用挂住——原版只在子进程 `close` 事件上 resolve，不等流结束。
        let stderr_buf: Arc<std::sync::Mutex<String>> =
            Arc::new(std::sync::Mutex::new(String::new()));
        let stderr_sink = Arc::clone(&stderr_buf);
        // 读到 EOF 时通知主任务（`Ok(0)`），这样正常结束就立刻返回 stderr，
        // 只有孙进程还攥着写端的病态情况才靠超时兜底。
        let (stderr_eof_tx, stderr_eof_rx) = tokio::sync::oneshot::channel::<()>();
        let stderr_task = tokio::spawn(async move {
            let mut collected: Vec<u8> = Vec::new();
            if let Some(mut stderr) = stderr {
                let mut chunk = [0u8; 8192];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) | Err(_) => {
                            let _ = stderr_eof_tx.send(());
                            break;
                        }
                        Ok(n) => {
                            let room = MAX_STDERR_BYTES.saturating_sub(collected.len());
                            collected.extend_from_slice(&chunk[..room.min(n)]);
                            *stderr_sink
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                String::from_utf8_lossy(&collected).into_owned();
                        }
                    }
                }
            }
            if collected.len() >= MAX_STDERR_BYTES {
                stderr_sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_str("\n[stderr truncated]");
            }
        });

        let mut abort_rx = invocation.abort.clone();
        let mut aborted = false;
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                let line = tokio::select! {
                    biased;
                    () = pi_extension_api::wait_aborted(&mut abort_rx) => {
                        aborted = true;
                        break;
                    }
                    next = lines.next_line() => match next {
                        Ok(Some(line)) => line,
                        // EOF / 读错误都结束循环（failed 判定看 exit code）
                        Ok(None) | Err(_) => break,
                    },
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                if event.get("type").and_then(Value::as_str) == Some("message_end") {
                    if let Some(message) = event.get("message") {
                        absorb_message(&mut result, message);
                        if let Some(progress) = progress.as_ref() {
                            progress(&result);
                        }
                    }
                }
            }
        }

        if aborted || invocation.is_aborted() {
            // 原版：`proc.kill("SIGTERM")`，5s 后 `SIGKILL`，然后抛
            // "Subagent was aborted"。这里直接 SIGKILL（子进程没有需要优雅
            // 收尾的状态），结果标记 aborted 让上层按失败处理。
            let _ = child.start_kill();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
            aborted = true;
        }

        let status = child.wait().await;
        // TS: `proc.on("close", code => resolve(code ?? 0))` —— 被信号杀死
        // （code 为 null）时按 0 处理，失败判定交给 stopReason。
        result.exit_code = status.ok().and_then(|status| status.code()).unwrap_or(0);
        // 子进程已退出：等 stderr 读到 EOF（正常情况立刻返回），最多 2s，
        // 之后放弃——孙进程可能仍持有写端（原版只在 `close` 事件上 resolve，
        // 也不等流结束）。
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stderr_eof_rx).await;
        stderr_task.abort();
        result.stderr = stderr_buf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if aborted {
            result.stop_reason = Some("aborted".to_string());
            result.error_message = Some("Subagent was aborted".to_string());
        }
        result
    }
}

/// 吸收一条 `message_end` 消息：assistant 消息累计 usage/模型/停止原因，
/// 所有消息都进 `messages`（对齐 TS `runSingleAgent` 的 processLine）。
fn absorb_message(result: &mut SingleResult, message: &Value) {
    result.messages.push(message.clone());

    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    result.usage.turns += 1;
    if let Some(usage) = message.get("usage") {
        result.usage.input += usage.get("input").and_then(Value::as_u64).unwrap_or(0);
        result.usage.output += usage.get("output").and_then(Value::as_u64).unwrap_or(0);
        result.usage.cache_read += usage.get("cacheRead").and_then(Value::as_u64).unwrap_or(0);
        result.usage.cache_write += usage.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0);
        result.usage.cost += usage
            .get("cost")
            .and_then(|cost| cost.get("total"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        result.usage.context_tokens = usage
            .get("totalTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    }
    if result.model.is_none() {
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            if !model.is_empty() {
                result.model = Some(model.to_string());
            }
        }
    }
    // TS `AgentMessage` wire shape is camelCase (`stopReason`, `errorMessage`);
    // accept the legacy snake_case spellings too so children that predate the
    // rename (or a mixed-version setup) still report their stop reason.
    if let Some(stop_reason) = message
        .get("stopReason")
        .or_else(|| message.get("stop_reason"))
        .and_then(Value::as_str)
    {
        result.stop_reason = Some(stop_reason.to_string());
    }
    if let Some(error_message) = message
        .get("errorMessage")
        .or_else(|| message.get("error_message"))
        .and_then(Value::as_str)
    {
        result.error_message = Some(error_message.to_string());
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// 子 agent 进度回调（收到一条子进程消息后调用，对齐 TS `emitUpdate`）。
type ProgressFn = dyn Fn(&SingleResult) + Send + Sync;

/// 运行中的中间结果（对齐 TS `emitUpdate`：终稿或 `(running...)` + 当前 details）。
fn running_output(
    mode: &str,
    scope: AgentScope,
    project_agents_dir: &Option<String>,
    results: &[SingleResult],
) -> ToolCallOutput {
    let text = results
        .iter()
        .rev()
        .map(|r| get_final_output(&r.messages))
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| "(running...)".to_string());
    ToolCallOutput {
        content: vec![json!({ "type": "text", "text": text })],
        details: Some(details(mode, scope, project_agents_dir, results)),
        is_error: false,
        terminate: None,
    }
}

// ============================================================================
// 自定义渲染（对齐 TS renderCall / renderResult）
// ============================================================================

/// 折叠态每条结果显示的条目数（TS `COLLAPSED_ITEM_COUNT`）。
const COLLAPSED_ITEM_COUNT: usize = 10;
/// 多任务模式下 折叠态每个结果显示的条目数（TS chain/parallel 用 5）。
const COLLAPSED_ITEMS_PER_RESULT: usize = 5;

/// 渲染用的轻量结果视图（从 `details.results[i]` 解析）。
struct RenderResult {
    agent: String,
    source: String,
    task: String,
    exit_code: i64,
    stop_reason: Option<String>,
    error_message: Option<String>,
    final_output: String,
    items: Vec<RenderItem>,
    usage: UsageStats,
    model: Option<String>,
    step: Option<usize>,
}

/// 一条可显示的记录（TS `DisplayItem`）。
enum RenderItem {
    Text(String),
    ToolCall(String),
}

impl RenderResult {
    fn from_json(value: &Value) -> Option<Self> {
        let agent = value.get("agent").and_then(Value::as_str)?.to_string();
        let messages = value
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let usage = value.get("usage").map(parse_usage).unwrap_or_default();
        Some(Self {
            agent,
            source: value
                .get("agentSource")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            task: value
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            exit_code: value.get("exitCode").and_then(Value::as_i64).unwrap_or(0),
            stop_reason: value
                .get("stopReason")
                .and_then(Value::as_str)
                .map(str::to_string),
            error_message: value
                .get("errorMessage")
                .and_then(Value::as_str)
                .map(str::to_string),
            final_output: get_final_output(messages),
            items: display_items(messages),
            usage,
            model: value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            step: value
                .get("step")
                .and_then(Value::as_u64)
                .map(|s| s as usize),
        })
    }

    fn failed(&self) -> bool {
        self.exit_code != 0 || matches!(self.stop_reason.as_deref(), Some("error" | "aborted"))
    }
}

fn parse_usage(value: &Value) -> UsageStats {
    UsageStats {
        input: value.get("input").and_then(Value::as_u64).unwrap_or(0),
        output: value.get("output").and_then(Value::as_u64).unwrap_or(0),
        cache_read: value.get("cacheRead").and_then(Value::as_u64).unwrap_or(0),
        cache_write: value.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0),
        cost: value.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        context_tokens: value
            .get("contextTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        turns: value.get("turns").and_then(Value::as_u64).unwrap_or(0) as u32,
    }
}

/// 从消息列表提取显示条目（TS `getDisplayItems`）：assistant 的文本块与
/// 工具调用，按出现顺序。
fn display_items(messages: &[Value]) -> Vec<RenderItem> {
    let mut items = Vec::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            items.push(RenderItem::Text(text.to_string()));
                        }
                    }
                }
                Some("toolCall") => {
                    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let args = block.get("arguments").cloned().unwrap_or(Value::Null);
                    items.push(RenderItem::ToolCall(format_tool_call(name, &args)));
                }
                _ => {}
            }
        }
    }
    items
}

/// 工具调用的单行摘要（对齐 TS `formatToolCall`，去掉主题着色后的纯文本）。
fn format_tool_call(name: &str, args: &Value) -> String {
    let path = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(Value::as_str);
    match name {
        "bash" => {
            let command = args.get("command").and_then(Value::as_str).unwrap_or("...");
            let preview: String = command.chars().take(60).collect();
            let ellipsis = if command.chars().count() > 60 {
                "..."
            } else {
                ""
            };
            format!("$ {preview}{ellipsis}")
        }
        "read" => {
            let mut text = format!("read {}", shorten_path(path.unwrap_or("...")));
            let offset = args.get("offset").and_then(Value::as_u64);
            let limit = args.get("limit").and_then(Value::as_u64);
            if offset.is_some() || limit.is_some() {
                let start = offset.unwrap_or(1);
                match limit {
                    Some(limit) => text.push_str(&format!(":{start}-{}", start + limit - 1)),
                    None => text.push_str(&format!(":{start}")),
                }
            }
            text
        }
        "write" => {
            let content = args.get("content").and_then(Value::as_str).unwrap_or("");
            let lines = content.lines().count();
            let mut text = format!("write {}", shorten_path(path.unwrap_or("...")));
            if lines > 1 {
                text.push_str(&format!(" ({lines} lines)"));
            }
            text
        }
        "edit" => format!("edit {}", shorten_path(path.unwrap_or("..."))),
        "ls" => format!(
            "ls {}",
            shorten_path(args.get("path").and_then(Value::as_str).unwrap_or("."))
        ),
        "find" => format!(
            "find {} in {}",
            args.get("pattern").and_then(Value::as_str).unwrap_or("*"),
            shorten_path(args.get("path").and_then(Value::as_str).unwrap_or("."))
        ),
        "grep" => format!(
            "grep /{}/ in {}",
            args.get("pattern").and_then(Value::as_str).unwrap_or(""),
            shorten_path(args.get("path").and_then(Value::as_str).unwrap_or("."))
        ),
        other => {
            let args_text = args.to_string();
            let preview: String = args_text.chars().take(50).collect();
            let ellipsis = if args_text.chars().count() > 50 {
                "..."
            } else {
                ""
            };
            format!("{other} {preview}{ellipsis}")
        }
    }
}

fn shorten_path(path: &str) -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if !home.is_empty() {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// 一条显示条目 → 若干渲染块（TS：文本折叠态取前 3 行，工具调用一行）。
fn item_blocks(item: &RenderItem, expanded: bool) -> Vec<ToolRenderBlock> {
    match item {
        RenderItem::ToolCall(text) => vec![ToolRenderBlock::spans(vec![
            ToolRenderSpan::new(ToolTextStyle::Muted, "→ "),
            ToolRenderSpan::new(ToolTextStyle::Muted, text.clone()),
        ])],
        RenderItem::Text(text) => {
            let lines: Vec<&str> = text.lines().collect();
            let take = if expanded || lines.len() <= 3 {
                lines.len()
            } else {
                3
            };
            lines[..take]
                .iter()
                .map(|line| ToolRenderBlock::styled(ToolTextStyle::Default, (*line).to_string()))
                .collect()
        }
    }
}

/// `─── 分节标题 ───` 行（TS muted 色）。
fn section(title: &str) -> ToolRenderBlock {
    ToolRenderBlock::styled(ToolTextStyle::Muted, title.to_string())
}

/// subagent 工具的自定义渲染（TS `renderCall` + `renderResult`）。
fn render_tool(ctx: &ToolRenderContext) -> Option<ToolRenderOutput> {
    let scope = ctx
        .args
        .get("agentScope")
        .and_then(Value::as_str)
        .unwrap_or("user");

    let results: Vec<RenderResult> = ctx
        .details
        .as_ref()
        .and_then(|d| d.get("results"))
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(RenderResult::from_json).collect())
        .unwrap_or_default();

    let mode = ctx
        .details
        .as_ref()
        .and_then(|d| d.get("mode"))
        .and_then(Value::as_str);

    // ── 调用阶段（还没有结果）：标题 + 步骤/任务预览（TS renderCall）──
    if results.is_empty() {
        return Some(render_call(&ctx.args, scope));
    }

    if results.len() == 1 && mode != Some("chain") && mode != Some("parallel") {
        return Some(render_single(&results[0], scope, ctx.expanded));
    }
    Some(render_multi(
        mode.unwrap_or("parallel"),
        &results,
        scope,
        ctx.expanded,
    ))
}

/// 标题三段：`bold("subagent ") + accent(<描述>) + muted(" [scope]")`
/// （对齐 TS `theme.bold("subagent ")` + accent + muted 拼行）。
fn title_spans(description: &str, scope: &str) -> Vec<ToolRenderSpan> {
    vec![
        ToolRenderSpan::new(ToolTextStyle::Bold, "subagent "),
        ToolRenderSpan::new(ToolTextStyle::Accent, description.to_string()),
        ToolRenderSpan::new(ToolTextStyle::Muted, format!(" [{scope}]")),
    ]
}

/// 带 ✓/✗/⏳ 图标的标题：图标单独着色（TS `theme.fg("success"/"error", icon)`），
/// 其余沿用 [`title_spans`]。
fn result_title(
    icon: &str,
    icon_style: ToolTextStyle,
    description: &str,
    scope: &str,
) -> Vec<ToolRenderSpan> {
    let mut spans = vec![ToolRenderSpan::new(icon_style, icon.to_string())];
    spans.extend(title_spans(description, scope));
    spans
}

/// 渲染调用中的工具行（TS `renderCall`）。
fn render_call(args: &Value, scope: &str) -> ToolRenderOutput {
    let preview = |text: &str, max: usize| -> String {
        let mut out: String = text.chars().take(max).collect();
        if text.chars().count() > max {
            out.push_str("...");
        }
        out
    };

    if let Some(chain) = args.get("chain").and_then(Value::as_array) {
        if !chain.is_empty() {
            let mut blocks = Vec::new();
            for (index, step) in chain.iter().take(3).enumerate() {
                let agent = step.get("agent").and_then(Value::as_str).unwrap_or("...");
                let task = step
                    .get("task")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .replace("{previous}", "");
                blocks.push(ToolRenderBlock::spans(vec![
                    ToolRenderSpan::new(ToolTextStyle::Dim, "  "),
                    ToolRenderSpan::new(ToolTextStyle::Muted, format!("{}. ", index + 1)),
                    ToolRenderSpan::new(ToolTextStyle::Accent, agent.to_string()),
                    ToolRenderSpan::new(
                        ToolTextStyle::Dim,
                        format!(" {}", preview(task.trim(), 40)),
                    ),
                ]));
            }
            if chain.len() > 3 {
                blocks.push(ToolRenderBlock::styled(
                    ToolTextStyle::Muted,
                    format!("  ... +{} more", chain.len() - 3),
                ));
            }
            return ToolRenderOutput {
                title: title_spans(&format!("chain ({} steps)", chain.len()), scope),
                blocks,
            };
        }
    }

    if let Some(tasks) = args.get("tasks").and_then(Value::as_array) {
        if !tasks.is_empty() {
            let mut blocks = Vec::new();
            for task in tasks.iter().take(3) {
                let agent = task.get("agent").and_then(Value::as_str).unwrap_or("...");
                let text = task.get("task").and_then(Value::as_str).unwrap_or_default();
                blocks.push(ToolRenderBlock::spans(vec![
                    ToolRenderSpan::new(ToolTextStyle::Dim, "  "),
                    ToolRenderSpan::new(ToolTextStyle::Accent, agent.to_string()),
                    ToolRenderSpan::new(ToolTextStyle::Dim, format!(" {}", preview(text, 40))),
                ]));
            }
            if tasks.len() > 3 {
                blocks.push(ToolRenderBlock::styled(
                    ToolTextStyle::Muted,
                    format!("  ... +{} more", tasks.len() - 3),
                ));
            }
            return ToolRenderOutput {
                title: title_spans(&format!("parallel ({} tasks)", tasks.len()), scope),
                blocks,
            };
        }
    }

    let agent = args
        .get("agent")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("...");
    let task = args
        .get("task")
        .and_then(Value::as_str)
        .map(|t| preview(t, 60))
        .unwrap_or_else(|| "...".to_string());
    ToolRenderOutput {
        title: title_spans(agent, scope),
        blocks: vec![ToolRenderBlock::styled(
            ToolTextStyle::Dim,
            format!("  {task}"),
        )],
    }
}

/// 单个子 agent 的结果（TS single 分支）。
fn render_single(result: &RenderResult, scope: &str, expanded: bool) -> ToolRenderOutput {
    let failed = result.failed();
    let mut description = format!("{} ({})", result.agent, result.source);
    if failed {
        if let Some(reason) = result
            .stop_reason
            .as_deref()
            .filter(|s| *s != "stop" && !s.is_empty())
        {
            description.push_str(&format!(" [{reason}]"));
        }
    }
    let mut blocks: Vec<ToolRenderBlock> = Vec::new();
    if let Some(error) = result.error_message.as_ref().filter(|e| !e.is_empty()) {
        blocks.push(ToolRenderBlock::styled(
            ToolTextStyle::Error,
            format!("Error: {error}"),
        ));
    }

    if expanded {
        // TS 展开态：Spacer + `─── Task ───` + task + Spacer +
        // `─── Output ───` + 工具调用 + Markdown 终稿 + Spacer + usage。
        blocks.push(ToolRenderBlock::Spacer);
        blocks.push(section("─── Task ───"));
        blocks.push(ToolRenderBlock::styled(
            ToolTextStyle::Dim,
            result.task.clone(),
        ));
        blocks.push(ToolRenderBlock::Spacer);
        blocks.push(section("─── Output ───"));
        for item in &result.items {
            if matches!(item, RenderItem::ToolCall(_)) {
                blocks.extend(item_blocks(item, true));
            }
        }
        if !result.final_output.is_empty() {
            blocks.push(ToolRenderBlock::Spacer);
            blocks.push(ToolRenderBlock::Markdown(result.final_output.clone()));
        }
        let usage = result.usage.format(result.model.as_deref());
        if !usage.is_empty() {
            blocks.push(ToolRenderBlock::Spacer);
            blocks.push(ToolRenderBlock::styled(ToolTextStyle::Dim, usage));
        }
    } else {
        // 折叠态：最近 N 条（文本 + 工具调用）+ 折叠提示 + usage。
        let skipped = result.items.len().saturating_sub(COLLAPSED_ITEM_COUNT);
        for item in &result.items[skipped..] {
            blocks.extend(item_blocks(item, false));
        }
        let mut footer: Vec<String> = Vec::new();
        if skipped > 0 {
            footer.push(format!("({skipped} earlier items)"));
        }
        footer.push("(Ctrl+O to expand)".to_string());
        blocks.push(ToolRenderBlock::styled(
            ToolTextStyle::Muted,
            footer.join(" · "),
        ));
        let usage = result.usage.format(result.model.as_deref());
        if !usage.is_empty() {
            blocks.push(ToolRenderBlock::styled(ToolTextStyle::Dim, usage));
        }
    }

    ToolRenderOutput {
        title: result_title(
            if failed { "✗" } else { "✓" },
            if failed {
                ToolTextStyle::Error
            } else {
                ToolTextStyle::Success
            },
            &description,
            scope,
        ),
        blocks,
    }
}

/// chain / parallel 的结果（TS 同名分支）：每步一组，
/// `─── Step N: agent` + Task + 工具调用 + 终稿 + 该步 usage，末尾 `Total:`。
fn render_multi(
    mode: &str,
    results: &[RenderResult],
    scope: &str,
    expanded: bool,
) -> ToolRenderOutput {
    let failed = results.iter().filter(|r| r.failed()).count();
    let running = results.iter().filter(|r| r.exit_code == -1).count();
    let (icon, icon_style) = if running > 0 {
        ("⏳", ToolTextStyle::Warning)
    } else if failed > 0 {
        ("◐", ToolTextStyle::Warning)
    } else {
        ("✓", ToolTextStyle::Success)
    };
    let description = if mode == "chain" {
        let done = results.iter().filter(|r| r.exit_code != -1).count();
        format!("chain {done}/{} steps", results.len())
    } else if running > 0 {
        format!(
            "parallel {}/{} done, {running} running",
            results.len() - running,
            results.len()
        )
    } else {
        format!(
            "parallel {}/{} tasks",
            results.len() - failed,
            results.len()
        )
    };

    let per_result = if expanded {
        usize::MAX
    } else {
        COLLAPSED_ITEMS_PER_RESULT
    };
    let mut total_usage = UsageStats::default();
    for result in results {
        total_usage.input += result.usage.input;
        total_usage.output += result.usage.output;
        total_usage.cache_read += result.usage.cache_read;
        total_usage.cache_write += result.usage.cache_write;
        total_usage.cost += result.usage.cost;
        total_usage.turns += result.usage.turns;
        total_usage.context_tokens = result.usage.context_tokens;
    }

    let mut blocks: Vec<ToolRenderBlock> = Vec::new();
    for result in results {
        let (mark, mark_style) = if result.exit_code == -1 {
            ("⏳", ToolTextStyle::Warning)
        } else if result.failed() {
            ("✗", ToolTextStyle::Error)
        } else {
            ("✓", ToolTextStyle::Success)
        };
        let label = match (mode, result.step) {
            ("chain", Some(step)) => format!("─── Step {step}: {} ", result.agent),
            _ => format!("─── {} ", result.agent),
        };
        blocks.push(ToolRenderBlock::Spacer);
        // 标题与图标同处一行（TS：muted 标题 + 着色图标）。
        blocks.push(ToolRenderBlock::spans(vec![
            ToolRenderSpan::new(ToolTextStyle::Muted, label),
            ToolRenderSpan::new(mark_style, mark),
        ]));

        if expanded {
            // TS：`Task: ...` 一行 + 工具调用 + Markdown 终稿 + 该步 usage。
            blocks.push(ToolRenderBlock::spans(vec![
                ToolRenderSpan::new(ToolTextStyle::Muted, "Task: "),
                ToolRenderSpan::new(ToolTextStyle::Dim, result.task.clone()),
            ]));
            for item in &result.items {
                if matches!(item, RenderItem::ToolCall(_)) {
                    blocks.extend(item_blocks(item, true));
                }
            }
            if let Some(error) = result.error_message.as_ref().filter(|e| !e.is_empty()) {
                blocks.push(ToolRenderBlock::styled(
                    ToolTextStyle::Error,
                    format!("Error: {error}"),
                ));
            }
            if !result.final_output.is_empty() {
                blocks.push(ToolRenderBlock::Spacer);
                blocks.push(ToolRenderBlock::Markdown(result.final_output.clone()));
            }
            let step_usage = result.usage.format(result.model.as_deref());
            if !step_usage.is_empty() {
                blocks.push(ToolRenderBlock::styled(ToolTextStyle::Dim, step_usage));
            }
        } else {
            let skipped = result.items.len().saturating_sub(per_result);
            for item in &result.items[skipped..] {
                blocks.extend(item_blocks(item, false));
            }
            if let Some(error) = result.error_message.as_ref().filter(|e| !e.is_empty()) {
                blocks.push(ToolRenderBlock::styled(
                    ToolTextStyle::Error,
                    format!("Error: {error}"),
                ));
            }
        }
    }

    blocks.push(ToolRenderBlock::Spacer);
    let total = total_usage.format(None);
    if !total.is_empty() {
        blocks.push(ToolRenderBlock::styled(
            ToolTextStyle::Dim,
            format!("Total: {total}"),
        ));
    }
    if !expanded {
        blocks.push(ToolRenderBlock::styled(
            ToolTextStyle::Muted,
            "(Ctrl+O to expand)",
        ));
    }

    ToolRenderOutput {
        title: result_title(icon, icon_style, &description, scope),
        blocks,
    }
}

fn details(
    mode: &str,
    scope: AgentScope,
    project_agents_dir: &Option<String>,
    results: &[SingleResult],
) -> Value {
    json!({
        "mode": mode,
        "agentScope": scope.as_str(),
        "projectAgentsDir": project_agents_dir,
        "results": results.iter().map(SingleResult::to_json).collect::<Vec<_>>(),
    })
}

fn text_output(text: String, details: Value, is_error: bool) -> ToolCallOutput {
    ToolCallOutput {
        content: vec![json!({ "type": "text", "text": text })],
        details: Some(details),
        is_error,
        terminate: None,
    }
}
