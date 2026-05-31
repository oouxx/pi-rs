# pi-agent-core

带工具执行和事件流的状态化 Agent 框架。建立在 `pi-ai` 之上。Rust 复刻自 [`@earendil-works/pi-agent-core`](https://github.com/earendil-works/pi/tree/main/packages/agent)。

## 快速开始

```rust
use pi_agent_core::agent::{Agent, AgentOptions};
use pi_agent_core::types::{AgentMessage, AgentEvent};
use pi_agent_core::pi_ai_types::{ContentBlock, ThinkingLevel, ToolExecutionMode};
use pi_ai::models::get_model;
use pi_ai::providers::register_builtins::register_built_in_api_providers;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    register_built_in_api_providers();

    let model = get_model("anthropic", "claude-sonnet-4-6").unwrap();

    let agent = Agent::new(AgentOptions {
        model: pi_agent_core::pi_ai_types::Model {
            provider: model.provider.clone(),
            api: model.api.clone(),
            id: model.id.clone(),
            name: model.name.clone(),
            base_url: model.base_url.clone(),
            context_window: model.context_window,
            max_tokens: model.max_tokens,
            cost: model.cost.clone(),
            reasoning: model.reasoning,
            thinking_level_map: None,
            input: vec!["text".into()],
            headers: None,
            compat: None,
        },
        system_prompt: Some("You are a helpful assistant.".into()),
        thinking_level: "medium".into(),
        tool_execution: Some(ToolExecutionMode::Sequential),
        ..Default::default()
    });

    // 订阅事件
    agent.subscribe(Arc::new(|event, _signal| {
        Box::pin(async move {
            match event {
                AgentEvent::TextDelta { delta, .. } => print!("{}", delta),
                _ => {}
            }
        })
    })).await;

    // 发送消息
    let messages = agent.process(vec![AgentMessage::User {
        content: vec![ContentBlock::Text {
            text: "Hello!".into(),
            text_signature: None,
        }],
        timestamp: chrono::Utc::now().timestamp_millis(),
    }]).await?;

    Ok(())
}
```

## 核心概念

### AgentMessage vs LLM Message

Agent 使用 `AgentMessage`，可以包含：
- 标准 LLM 消息：`User`, `Assistant`, `ToolResult`
- 扩展消息：`BashExecution`, `Custom`, `BranchSummary`, `CompactionSummary`

`ConvertToLlmFn` 在每次 LLM 调用前将 `AgentMessage` → `pi_ai::Message`：

```
AgentMessage[] → convertToLlm() → Message[] → LLM
```

### 事件流

```text
prompt("Hello")
├─ AgentStart
├─ TurnStart
├─ MessageStart  { userMessage }
├─ MessageEnd    { userMessage }
├─ MessageStart  { assistantMessage }    // LLM 开始响应
├─ MessageUpdate { partial... }          // 流式增量
├─ MessageEnd    { assistantMessage }    // 完整回复
├─ TurnEnd       { message, toolResults }
└─ AgentEnd      { messages }
```

带工具调用时，循环继续：

```text
├─ ToolExecutionStart  { toolCallId, toolName, args }
├─ ToolExecutionUpdate { partialResult }
├─ ToolExecutionEnd    { toolCallId, result }
├─ MessageStart/End    { toolResultMessage }
├─ TurnEnd
│
├─ TurnStart           // LLM 处理工具结果
├─ MessageStart        { assistantMessage }
├─ ...
└─ AgentEnd
```

## AgentOptions

```rust
let agent = Agent::new(AgentOptions {
    // 初始状态
    model: Model { ... },
    system_prompt: Some("You are helpful.".into()),
    thinking_level: "medium".into(),

    // 工具
    tools: vec![my_tool],

    // 转换函数
    convert_to_llm: Some(Arc::new(|messages| { ... })),

    // 生命周期 hooks
    before_tool_call: Some(Arc::new(|ctx, signal| { ... })),
    after_tool_call: Some(Arc::new(|ctx, signal| { ... })),
    should_stop_after_turn: Some(Arc::new(|ctx| { ... })),
    prepare_next_turn: Some(Arc::new(|ctx| { ... })),

    // 高级配置
    stream_fn: custom_stream_function,
    get_api_key: Some(Arc::new(|provider| { ... })),
    session_id: Some("session-123".into()),
    tool_execution: Some(ToolExecutionMode::Parallel),
    thinking_budgets: Some(ThinkingBudgets { ... }),

    ..Default::default()
});
```

## Agent 方法

| 方法 | 说明 |
|------|------|
| `process(messages)` | 发送消息并运行 agent loop |
| `continue_run()` | 从当前上下文继续（最后一条消息必须是 user 或 toolResult） |
| `steer(message)` | 注入 steering 消息 |
| `follow_up(message)` | 注入 follow-up 消息 |
| `abort()` | 取消当前运行 |
| `wait_for_idle()` | 等待 agent 空闲 |
| `reset()` | 重置状态 |
| `subscribe(listener)` | 订阅事件 |
| `state()` | 获取当前状态 |
| `set_model(model)` | 切换模型 |
| `set_thinking_level(level)` | 切换 thinking level |
| `set_system_prompt(prompt)` | 设置系统提示词 |

## Steering 和 Follow-up

```rust
// Steering: 当前 turn 完成后注入（适用于工具运行时打断）
agent.steer(AgentMessage::User { ... }).await;

// Follow-up: agent 完成所有工作后注入
agent.follow_up(AgentMessage::User { ... }).await;

// 队列管理
agent.clear_steering_queue().await;
agent.clear_follow_up_queue().await;
agent.clear_all_queues().await;
```

## 自定义消息类型

```rust
// AgentMessage 支持扩展类型
AgentMessage::Custom {
    custom_type: "notification".into(),
    content: CustomContent::Text("Info".into()),
    display: true,
    details: None,
    timestamp,
}

// 在 convert_to_llm 中处理自定义类型
let convert_fn = Arc::new(|messages: &[AgentMessage]| -> Vec<Message> {
    messages.iter().filter_map(|m| match m {
        AgentMessage::User { .. } => Some(convert_user(m)),
        AgentMessage::Assistant { .. } => Some(convert_assistant(m)),
        AgentMessage::ToolResult { .. } => Some(convert_tool_result(m)),
        AgentMessage::Custom { .. } => None, // 过滤掉
        _ => None,
    }).collect()
});
```

## 定义工具

```rust
use pi_agent_core::types::{AgentTool, AgentToolResult};

let read_file_tool = AgentTool {
    name: "read_file".into(),
    description: "Read a file's contents".into(),
    parameters_schema: serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "File path" }
        },
        "required": ["path"]
    }),
    execute: Arc::new(|tool_call_id, args, signal, on_update| {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or("");
            let content = tokio::fs::read_to_string(path).await?;
            Ok(AgentToolResult {
                content: vec![ContentBlock::text(content)],
                details: serde_json::json!({"path": path}),
                terminate: None,
            })
        })
    }),
    ..Default::default()
};
```

## 低层 API

```rust
use pi_agent_core::agent_loop::{run_agent_loop, AgentLoopConfig};

let config = AgentLoopConfig {
    model,
    reasoning: Some("medium".into()),
    convert_to_llm: Arc::new(|msgs| convert(msgs)),
    tool_execution: ToolExecutionMode::Parallel,
    ..Default::default() // AgentLoopConfig::default() is not implemented, see below
};

let context = AgentContext {
    system_prompt: "You are helpful.".into(),
    messages: vec![],
    tools: Some(vec![my_tool]),
};

let new_messages = run_agent_loop(
    vec![user_message],
    context,
    &config,
    &event_sink,
    &signal,
    &stream_fn,
).await?;
```

## 模块结构

| 模块 | 说明 |
|------|------|
| `agent` | Agent 生命周期管理、事件分发 |
| `agent_loop` | 多轮对话循环、顺序/并行工具执行 |
| `types` | AgentMessage, AgentContext, AgentTool, StreamFn 等 |
| `pi_ai_types` | re-export pi-ai 的类型（Model, Message, ContentBlock 等） |
| `harness::compaction` | 上下文压缩、LLM 摘要生成 |
| `harness::messages` | AgentMessage ↔ LLM Message 转换 |
| `harness::prompt_templates` | 模板加载（frontmatter 解析）和参数替换 |
| `harness::skills` | Skill 格式化和 XML 输出 |
| `harness::skill_loader` | 递归加载 SKILL.md 文件 |
| `harness::session` | JSONL/内存会话存储 |
| `harness::system_prompt` | 系统提示词组装 |
| `harness::types` | ExecutionEnv, Session, 错误类型 |
| `harness::utils` | 文本截断、Shell 输出捕获 |
| `proxy` | HTTP 代理流式传输 |

## 集成测试

使用 OpenRouter 免费模型验证 Agent → pi-ai → OpenRouter 完整链路：

```bash
export OPENROUTER_API_KEY="sk-or-..."
cargo test --test openrouter_integration_test -- --ignored --nocapture
```

**测试输出：**

```
running 3 tests

test test_agent_basic_prompt ... ok
  Got 2 messages
  Response: Hello! I'm here and ready to help...
  85 events received

test test_agent_multi_turn ... ok
  Turn 1: What is 1+1? → 1 + 1 = 2
  Turn 2: Multiply by 3 → 2 × 3 = 6

test test_agent_idle_state ... ok
  State: model=openrouter/poolside/laguna-xs.2:free, messages=1, thinking=off

test result: ok. 3 passed; 0 failed; finished in 2.42s
```

测试覆盖：
- `test_agent_basic_prompt` — Agent.process() 基本流程，验证事件流和响应文本
- `test_agent_multi_turn` — 多轮对话上下文保持（1+1 → ×3）
- `test_agent_idle_state` — Agent 状态跟踪（is_streaming、error_message）

默认使用 `poolside/laguna-xs.2:free`，可通过 `PI_TEST_MODEL` 环境变量覆盖。

## License

MIT
