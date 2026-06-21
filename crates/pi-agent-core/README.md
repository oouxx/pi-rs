# pi-agent-core

通用状态化 Agent 框架，支持工具执行、事件流、会话管理。建立在 `pi-ai` 之上。Rust 复刻自 [`@earendil-works/pi-agent-core`](https://github.com/earendil-works/pi/tree/main/packages/agent)。

## 架构概览

```
pi-agent-core
├── Agent          # 高级 Agent 生命周期管理、事件分发、消息队列
├── agent_loop     # 底层 Agent 循环：流式 LLM 调用 + 多轮工具执行
├── types          # 核心类型：AgentMessage, AgentTool, 回调函数签名
├── pi_ai_types    # pi-ai 类型 re-export + ToolExecutionMode + 辅助函数
├── proxy          # SSE 代理流式传输（用于外部 LLM API）
└── harness        # 高层封装：会话/分支/压缩/技能加载/SKILL.md
    ├── agent_harness   # AgentHarness：完整生命周期管理器
    ├── types           # Skill, Session, ExecutionEnv, CompactionSettings
    ├── messages        # AgentMessage ↔ LLM Message 转换
    ├── compaction      # 上下文压缩、分支摘要
    ├── env/nodejs      # NodeExecutionEnv (基于 tokio::fs + tokio::process)
    ├── session         # 会话存储（JSONL / InMemory）
    ├── skill_loader    # SKILL.md 递归加载
    ├── skills          # Skill XML 格式化
    ├── prompt_templates # 提示词模板（Frontmatter 解析 + 参数替换）
    └── utils           # 文本截断、Shell 输出捕获
```

## 快速入门

### 依赖

```toml
[dependencies]
pi-agent-core = "1.78"
pi-ai = { path = "../pi-ai" }
tokio = { version = "1", features = ["full"] }
```

### 最小 Agent

```rust
use std::sync::Arc;
use pi_agent_core::agent::{Agent, AgentOptions, create_agent};
use pi_agent_core::types::{AgentMessage, DynTool, StreamFn, ConvertToLlmFn, StreamFnOptions};
use pi_agent_core::pi_ai_types::{Model, ContentBlock, text_block};

// 1. 定义 StreamFn（LLM 驱动）
let stream_fn: StreamFn = Arc::new(|model, context, thinking_level, options| {
    Box::pin(async move {
        // 调用 LLM API，返回 Stream<AssistantMessageEvent>
        // 参见 pi-ai 的 stream::stream_request()
        todo!("实现流式 LLM 调用")
    })
});

// 2. 定义消息转换函数
let convert_to_llm: ConvertToLlmFn = Arc::new(|messages| {
    pi_agent_core::harness::messages::convert_to_llm(messages)
});

// 3. 创建 Agent（使用 create_agent 便捷函数）
let agent = create_agent(
    model,                                    // pi_ai::types::Model
    "You are a helpful assistant.",           // system prompt
    vec![],                                   // 工具列表
    stream_fn,
    convert_to_llm,
);

// 4. 发送消息
use chrono::Utc;
let msg = AgentMessage::User {
    content: vec![text_block("Hello!")],
    timestamp: Utc::now().timestamp_millis(),
};
let results = agent.process(vec![msg]).await?;

// 5. 读取回复
for msg in &results {
    if let AgentMessage::Assistant { content, model, usage, .. } = msg {
        for block in content {
            match block {
                ContentBlock::Text { text, .. } => println!("{}", text),
                ContentBlock::Thinking { thinking, .. } => println!("[thinking] {}", thinking),
                _ => {}
            }
        }
    }
}
```

### 使用 AgentOptions 完整配置

```rust
let agent = Agent::new(AgentOptions {
    convert_to_llm: Some(convert_fn),
    stream_fn: Some(stream_fn),
    get_api_key: Some(Arc::new(|provider| {
        Box::pin(async move {
            Some(std::env::var("API_KEY").unwrap_or_default())
        })
    })),
    before_tool_call: Some(before_hook),
    after_tool_call: Some(after_hook),
    steering_mode: Some(QueueMode::OneAtATime),
    follow_up_mode: Some(QueueMode::All),
    session_id: Some("session-123".into()),
    tool_execution: Some(ToolExecutionMode::Parallel),
    transport: Some("auto".into()),
    ..Default::default()
});
```

## AgentMessage

Agent 使用 `AgentMessage` 作为统一消息类型，包含 7 种变体：

| 变体 | 角色 | 说明 |
|------|------|------|
| `User` | `"user"` | 用户输入 |
| `Assistant` | `"assistant"` | 模型回复（含 usage、stop_reason） |
| `ToolResult` | `"toolResult"` | 工具执行结果 |
| `BashExecution` | `"bashExecution"` | Shell 命令执行记录 |
| `Custom` | `"custom"` | 自定义扩展消息 |
| `BranchSummary` | `"branchSummary"` | 分支摘要 |
| `CompactionSummary` | `"compactionSummary"` | 上下文压缩摘要 |

Agent 在每次 LLM 调用前将 `AgentMessage` 转换为 `pi_ai::Message`：

```
AgentMessage[] → convert_to_llm() → Message[] → LLM
```

默认转换函数（`harness::messages::convert_to_llm`）：
- `User` → `Message::User`
- `Assistant` → `Message::Assistant`
- `ToolResult` → `Message::ToolResult`
- `BashExecution` → `Message::User`（格式化为文本），`exclude_from_context: true` 时跳过
- `Custom` → `Message::User`
- `BranchSummary` → `Message::User`（包裹 XML 标签）
- `CompactionSummary` → `Message::User`（包裹 XML 标签）

自定义转换：

```rust
let custom_convert: ConvertToLlmFn = Arc::new(|messages| {
    messages.iter().filter_map(|m| match m {
        AgentMessage::Custom { custom_type, content, .. } if custom_type == "notification" => None,
        _ => Some(convert_default(m)),
    }).collect()
});
```

### AgentMessage 辅助方法

```rust
// 获取消息角色标识
msg.role();  // "user" | "assistant" | "toolResult" | "bashExecution" | ...

// 获取时间戳
msg.timestamp();  // i64
```

## 事件流

Agent 在运行时发出事件，可通过 `subscribe` 订阅：

### 基本流程

```
AgentStart
├─ TurnStart
├─ MessageStart   { user message }
├─ MessageEnd     { user message }
├─ MessageStart   { assistant message }   // LLM 开始响应
├─ MessageUpdate  { partial... }          // 流式增量
├─ MessageEnd     { assistant message }   // 完整回复
├─ TurnEnd        { message, toolResults }
└─ AgentEnd       { messages }
```

### 带工具调用的流程

```
├─ ToolExecutionStart   { tool_call_id, tool_name, args }
├─ ToolExecutionUpdate  { partial_result }
├─ ToolExecutionEnd     { tool_call_id, result, is_error }
├─ MessageStart/End     { tool_result_message }
├─ TurnEnd
│
├─ TurnStart            // LLM 处理工具结果
├─ MessageStart         { assistant message }
├─ ...
└─ AgentEnd
```

### 订阅事件

```rust
let handle = agent.subscribe(Arc::new(|event, _signal| {
    Box::pin(async move {
        match event {
            AgentEvent::AgentStart => println!("Agent started"),
            AgentEvent::MessageStart { message } => {
                println!("Message from: {}", message.role());
            }
            AgentEvent::MessageUpdate { message, assistant_message_event } => {
                // 流式增量
            }
            AgentEvent::MessageEnd { message } => {
                println!("Message complete");
            }
            AgentEvent::TurnEnd { message, tool_results } => {
                println!("Turn ended, {} tool result(s)", tool_results.len());
            }
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                println!("Tool {} executing", tool_name);
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, is_error, .. } => {
                if is_error { println!("Tool failed"); }
            }
            AgentEvent::AgentEnd { messages } => {
                println!("Agent finished, {} messages total", messages.len());
            }
            _ => {}
        }
    })
})).await;

// 取消订阅
handle.unsubscribe().await;
```

## Agent API

### 核心方法

| 方法 | 说明 |
|------|------|
| `Agent::new(options)` | 构造 Agent |
| `create_agent(...)` | 便捷函数 |
| `process(messages)` | 发送消息并运行 agent loop |
| `continue_run()` | 从当前上下文继续（最后一条不能是 assistant） |
| `steer(message)` | 注入 steering 消息，当前 turn 完成后处理 |
| `follow_up(message)` | 注入 follow-up 消息，agent 完成所有工作后处理 |
| `abort()` | 取消当前运行 |
| `wait_for_idle()` | 等待 agent 空闲（基于 Notify，无轮询） |

### 状态查询

```rust
agent.state().await;              // AgentState（含 model, messages, is_streaming 等）
agent.messages().await;           // Vec<AgentMessage>
agent.is_streaming().await;       // bool
agent.error_message().await;      // Option<String>
agent.has_queued_messages().await; // bool
agent.cancellation_token().await;  // Option<CancellationToken>
```

### 运行时修改

```rust
agent.set_model(new_model).await;
agent.set_thinking_level("high".into()).await;
agent.set_system_prompt("New prompt".into()).await;
agent.set_steering_mode(QueueMode::All).await;
agent.set_follow_up_mode(QueueMode::OneAtATime).await;
```

### 队列管理

```rust
agent.steer(msg).await;        // 注入（当前 turn 后执行）
agent.follow_up(msg).await;    // 注入（所有工作完成后执行）
agent.clear_steering_queue().await;
agent.clear_follow_up_queue().await;
agent.clear_all_queues().await;
```

### 复位

```rust
agent.reset().await;
// 清除消息、中止运行、清空队列
```

## 工具注册与执行

### 定义工具

`AgentTool<P, D>` 是泛型工具定义，`DynTool` 是 `AgentTool<Value, Value>` 的类型别名：

```rust
use pi_agent_core::types::{AgentTool, DynTool, AgentToolResult};
use pi_agent_core::pi_ai_types::{ContentBlock, ToolExecutionMode, text_block};

let read_tool: Arc<DynTool> = Arc::new(AgentTool {
    name: "read".into(),
    description: "Read a file from the filesystem".into(),
    label: "Read File".into(),
    parameters_schema: serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "File path" }
        },
        "required": ["path"]
    }),
    execution_mode: None,  // 继承全局模式
    prepare_arguments: None, // 参数预处理函数
    execute: Arc::new(|_id, args, _signal, _on_update| {
        Box::pin(async move {
            let path = args["path"].as_str().unwrap_or("");
            let content = tokio::fs::read_to_string(path).await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            Ok(AgentToolResult {
                content: vec![text_block(&content)],
                details: serde_json::json!({"path": path}),
                terminate: None,
            })
        })
    }),
});
```

### 注册到 Agent

```rust
let agent = Agent::new(AgentOptions {
    model: model.clone(),
    tools: vec![read_tool],
    tool_execution: Some(ToolExecutionMode::Parallel),
    ..Default::default()
});
```

### 工具执行生命周期

```
ToolExecutionStart { tool_call_id, tool_name, args }
  ├─ prepare: validate_tool_arguments + prepare_arguments 回调
  ├─ before_tool_call hook → 可阻断执行（返回 block: true）
  ├─ execute: 异步执行（Parallel 模式下并发执行）
  ├─ after_tool_call hook → 可修改结果
ToolExecutionEnd   { tool_call_id, result, is_error }
MessageStart/End   { ToolResult }
```

### Before/After Hooks

```rust
use pi_agent_core::types::{BeforeToolCallFn, BeforeToolCallResult,
    AfterToolCallFn, AfterToolCallResult};

let before_hook: BeforeToolCallFn = Arc::new(|ctx, _signal| {
    Box::pin(async move {
        // ctx.args — 验证或修改参数
        if ctx.args.get("path").and_then(|v| v.as_str())
            .map_or(true, |p| p.contains(".."))
        {
            return Some(BeforeToolCallResult {
                block: true,
                reason: Some("Path traversal detected".into()),
            });
        }
        None  // 允许执行
    })
});

let after_hook: AfterToolCallFn = Arc::new(|ctx, _signal| {
    Box::pin(async move {
        // ctx.result — 修改工具输出
        Some(AfterToolCallResult {
            content: Some(ctx.result.content.clone()),
            details: Some(serde_json::json!({"modified": true})),
            is_error: None,
            terminate: None,
        })
    })
});
```

### 工具执行模式

| 模式 | 行为 |
|------|------|
| `Parallel`（默认） | prepare 阶段顺序执行，execute 阶段并发运行 |
| `Sequential` | 每个工具完整执行后才开始下一个 |

可在全局 `AgentOptions.tool_execution` 或单个工具的 `execution_mode` 中设置。单个工具标记为 `Sequential` 会强制整个批次转为顺序执行。

### 工具终止

当所有工具返回 `terminate: Some(true)` 时，agent loop 停止，不再向 LLM 发送工具结果。

## 使用 pi-ai 构建 StreamFn

`pi-agent-core` 需要用户提供一个 `StreamFn` 实现，典型使用 `pi-ai` 的 `stream_request`：

```rust
use pi_ai::stream::stream_request;
use pi_ai::providers::register_builtins::register_built_in_api_providers;

register_built_in_api_providers();

let stream_fn: StreamFn = Arc::new(|model, context, thinking_level, options| {
    Box::pin(async move {
        let api_key = options.api_key.clone();
        let result = stream_request(
            model,
            context,
            SimpleStreamOptions {
                api_key: api_key.clone(),
                signal: options.signal.clone(),
                ..Default::default()
            },
        ).await?;
        Ok(result.stream)
    })
});
```

## 底层 API: agent_loop

直接使用 `run_agent_loop` 获得更多控制权：

```rust
use pi_agent_core::agent_loop::{run_agent_loop, AgentLoopConfig};
use pi_agent_core::types::{AgentContext, AgentEventSink};

let config = AgentLoopConfig {
    model: model.clone(),
    reasoning: Some("medium".into()),
    tool_execution: ToolExecutionMode::Parallel,
    convert_to_llm: convert_fn.clone(),
    get_steering_messages: Some(Arc::new(|| {
        Box::pin(async { vec![] })
    })),
    get_follow_up_messages: Some(Arc::new(|| {
        Box::pin(async { vec![] })
    })),
    ..Default::default()  // 注：AgentLoopConfig 没有 Default，需手动填 None
};

let context = AgentContext {
    system_prompt: "You are helpful.".into(),
    messages: vec![],
    tools: Some(vec![my_tool]),
};

let emit: AgentEventSink = Arc::new(|event| Box::pin(async {
    // 处理事件
}));

let new_messages = run_agent_loop(
    vec![user_message],  // prompts
    context,
    &config,
    &emit,
    &signal,              // Option<watch::Receiver<bool>>
    &stream_fn,
).await?;
```

`run_agent_loop_continue` 用于从已有上下文继续运行（不能是 assistant 结尾）：

```rust
let context = AgentContext {
    system_prompt: "Continue".into(),
    messages: existing_messages,
    tools: None,
};

let more_messages = run_agent_loop_continue(
    context, &config, &emit, &signal, &stream_fn
).await?;
```

`agent_loop` 和 `agent_loop_continue` 是流式包装函数，将 loop 放入后台执行并返回事件流：

```rust
use pi_agent_core::agent_loop::agent_loop;

let mut stream = agent_loop(
    prompts,
    context,
    config,
    signal,              // Option<watch::Receiver<bool>>
    stream_fn,
);

use futures::StreamExt;
while let Some(event) = stream.next().await {
    match event {
        AgentEvent::MessageEnd { message } => { /* ... */ }
        AgentEvent::TurnEnd { .. } => { /* ... */ }
        AgentEvent::AgentEnd { messages } => {
            // 消息收集完毕
            break;
        }
        _ => {}
    }
}
```

## pi_ai_types 模块

Re-export `pi-ai` 的所有核心类型，外加 pi-agent-core 特有的类型和辅助函数。

### Re-export 的类型

```rust
pub use pi_ai::types::{
    Model, Message, ContentBlock, Context, Tool, ToolCall,
    AssistantMessage, AssistantMessageEvent,
    StopReason, Usage, ThinkingLevel, ThinkingBudgets,
    // ... 其他
};
pub use pi_ai::models::{calculate_cost, get_model, get_models, get_providers};
```

### 特有的类型

```rust
pub enum ToolExecutionMode { Sequential, Parallel }
pub type StreamResponse = Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>;
```

### 辅助函数

```rust
use pi_agent_core::pi_ai_types::*;

// ContentBlock 构造
text_block("Hello");
thinking_block("思考中...");
tool_call_block("id-1", "read", json!({"path": "/tmp"}));
image_block("base64data", "image/png");

// ModelCost
model_cost(3.0, 15.0, 1.5, 6.0);  // input, output, cache_read, cache_write

// AssistantMessage
assistant_message(content, api, provider, model, usage, stop_reason, timestamp);
assistant_message_error(content, api, provider, model, usage, stop_reason, error, timestamp);

// pi_ai Message 构造
user_msg(content, timestamp);
assistant_msg(content, api, provider, model, usage, stop_reason, timestamp);
tool_result_msg(tool_call_id, tool_name, content, is_error, timestamp);

// Context 构造
make_context(system_prompt, messages, tools);

// 其他
empty_usage();
```

### ThinkingLevel 常量

```rust
THINKING_OFF      // "off"
THINKING_MINIMAL  // "minimal"
THINKING_LOW      // "low"
THINKING_MEDIUM   // "medium"
THINKING_HIGH     // "high"
THINKING_XHIGH    // "xhigh"
```

### 典型用法

```rust
use pi_agent_core::pi_ai_types::{
    Model, ContentBlock, text_block, make_context, ToolExecutionMode,
    THINKING_MEDIUM,
};
```

## Proxy 模块

SSE 代理流式传输，用于将 LLM 请求代理到外部端点。

```rust
use pi_agent_core::proxy::{
    stream_proxy,
    ProxyAssistantMessageEvent,
    ProxyStreamOptions,
    process_proxy_event,
};

// 代理流式请求
let response = stream_proxy(
    model,                  // pi_ai::types::Model
    context,                // pi_ai::types::Context
    ProxyStreamOptions {
        proxy_url: "https://api.example.com".into(),
        auth_token: "sk-xxx".into(),
        signal: Some(cancel_rx),
        headers: Some(HashMap::from([("X-Custom".into(), "value".into())])),
        temperature: Some(0.7),
        max_tokens: Some(4096),
        ..Default::default()
    },
);

// 处理代理事件
let mut partial = AssistantMessage::default();
if let Some(event) = process_proxy_event(proxy_event, &mut partial) {
    // 转发为标准 AssistantMessageEvent
}
```

## Harness 模块

`harness` 提供高层封装，用于构建完整的 Agent 应用。它整合了 Agent 生命周期、会话持久化、上下文压缩、技能加载等功能。

### AgentHarness

```rust
use pi_agent_core::harness::{
    AgentHarness, AgentHarnessOptions, AgentHarnessEvent,
    AgentHarnessResources, AgentHarnessOwnEvent,
};
use pi_agent_core::harness::env::nodejs::NodeExecutionEnv;
use pi_agent_core::harness::session::InMemorySessionRepo;
use pi_agent_core::harness::types::*;

let env = Arc::new(NodeExecutionEnv::new("/path/to/cwd"));
let mut repo = InMemorySessionRepo::new();
let session = repo.create(SessionCreateOptions {
    id: None,
    cwd: "/path/to/cwd".into(),
    parent_session_path: None,
}).await.unwrap();

let harness = AgentHarness::new(
    env,
    session,
    model,
    Some(AgentHarnessOptions {
        thinking_level: Some("off".into()),
        active_tool_names: Some(vec!["read".into(), "write".into()]),
        resources: Some(AgentHarnessResources {
            skills: Some(vec![/* Skill */]),
            prompt_templates: Some(vec![]),
        }),
        ..Default::default()
    }),
);

// 发起提示
harness.prompt("Hello, what can you do?", None).await.unwrap();
harness.wait_for_idle().await;

// 运行时修改
harness.set_model(new_model).await.unwrap();
harness.set_thinking_level("high".into()).await.unwrap();
harness.set_active_tools(vec!["read".into()]).await.unwrap();
harness.set_resources(new_resources).await;

// 中止
harness.abort().await.unwrap();

// 手动压缩
harness.compact(None).await.unwrap();

// 订阅事件
let handle = harness.subscribe(Arc::new(|event, _signal| {
    Box::pin(async move {
        match event {
            AgentHarnessEvent::Agent(agent_event) => { /* Agent 事件 */ }
            AgentHarnessEvent::Own(own_event) => {
                match own_event {
                    AgentHarnessOwnEvent::BeforeAgentStart { .. } => {}
                    AgentHarnessOwnEvent::ModelUpdate { model, .. } => {}
                    AgentHarnessOwnEvent::SessionCompact { result } => {}
                    _ => {}
                }
            }
        }
    })
})).await;

// 注册类型化钩子（JSON 类型擦除方式）
harness.on("before_provider_request", Arc::new(|payload| {
    Box::pin(async move {
        // payload: serde_json::Value
        // 返回修改后的 stream_options
        Ok(serde_json::json!({
            "stream_options": { "temperature": 0.5 }
        }))
    })
})).await;

// 树导航（分支切换）
harness.navigate_tree("target-entry-id", Some(MoveToSummary {
    summary: "Branch summary".into(),
    details: None,
    from_hook: None,
})).await.unwrap();
```

### 会话管理

会话（Session）使用树形结构存储消息，支持分支和回溯。

#### SessionStorage trait

```rust
#[async_trait]
pub trait SessionStorage<M = SessionMetadata>: Send + Sync {
    async fn get_metadata(&self) -> M;
    async fn get_leaf_id(&self) -> Option<String>;
    async fn set_leaf_id(&mut self, leaf_id: Option<String>) -> Result<(), SessionError>;
    async fn create_entry_id(&self) -> String;
    async fn append_entry(&mut self, entry: SessionTreeEntry) -> Result<(), SessionError>;
    async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry>;
    async fn find_entries(&self, entry_type: &str) -> Vec<SessionTreeEntry>;
    async fn get_label(&self, id: &str) -> Option<String>;
    async fn get_path_to_root(&self, leaf_id: Option<&str>) -> Result<Vec<SessionTreeEntry>, SessionError>;
    async fn get_entries(&self) -> Vec<SessionTreeEntry>;
}
```

#### SessionRepo trait

```rust
#[async_trait]
pub trait SessionRepo<M = SessionMetadata>: Send + Sync {
    async fn create(&mut self, options: SessionCreateOptions) -> Result<Session<M>, SessionError>;
    async fn open(&self, metadata: &M) -> Result<Session<M>, SessionError>;
    async fn list(&self) -> Result<Vec<M>, SessionError>;
    async fn delete(&mut self, metadata: &M) -> Result<(), SessionError>;
    async fn fork(&mut self, source_metadata: &M, options: ForkOptions) -> Result<Session<M>, SessionError>;
}
```

#### 存储实现

| 实现 | 说明 |
|------|------|
| `InMemorySessionStorage` | 内存存储，适合测试 |
| `InMemorySessionRepo` | 内存仓库 |
| `JsonlSessionStorage` | JSONL 文件持久化 |
| `JsonlSessionRepo` | JSONL 文件仓库（按目录组织） |

#### Session 操作

```rust
// 创建存储
let mut storage = JsonlSessionStorage::create(
    "/tmp/sessions/session-1.jsonl",
    "/workspace",
    "session-1",
    None,  // parent_session_path
).await.unwrap();

// 或创建仓库
let mut repo = JsonlSessionRepo::new("/tmp/sessions");
let session = repo.create(SessionCreateOptions {
    id: Some("my-session".into()),
    cwd: "/workspace".into(),
    parent_session_path: None,
}).await.unwrap();

// 追加条目
session.append_message(user_msg).await.unwrap();
session.append_thinking_level_change("high".into()).await.unwrap();
session.append_model_change("anthropic".into(), "claude-opus-4".into()).await.unwrap();
session.append_compaction("summary".into(), "entry-1".into(), 50000, None, None).await.unwrap();

// 构建当前分支上下文
let ctx = session.build_context().await.unwrap();
for msg in &ctx.messages { /* ... */ }

// 分支树导航
session.move_to(Some("entry-id"), Some(MoveToSummary {
    summary: "回溯到此节点".into(),
    details: None,
    from_hook: None,
})).await.unwrap();

// 列表/删除
repo.list().await.unwrap();
repo.delete(&metadata).await.unwrap();
```

### ExecutionEnv trait

`ExecutionEnv` 提供了统一的文件系统和进程执行接口：

```rust
#[async_trait]
pub trait ExecutionEnv: Send + Sync {
    fn cwd(&self) -> &str;
    async fn read_text_file(&self, path: &str, options: Option<ReadTextFileOptions>) -> Result<String, FileError>;
    async fn read_binary_file(&self, path: &str) -> Result<Vec<u8>, FileError>;
    async fn read_text_lines(&self, path: &str, options: Option<ReadTextFileOptions>) -> Result<Vec<String>, FileError>;
    async fn write_file(&self, path: &str, content: &str, abort_signal: Option<watch::Receiver<bool>>) -> Result<(), FileError>;
    async fn append_file(&self, path: &str, content: &str) -> Result<(), FileError>;
    async fn file_info(&self, path: &str) -> Result<FileInfoType, FileError>;
    async fn list_dir(&self, path: &str) -> Result<Vec<FileInfoType>, FileError>;
    async fn canonical_path(&self, path: &str) -> Result<String, FileError>;
    async fn exists(&self, path: &str) -> Result<bool, FileError>;
    async fn join_path(&self, parts: &[&str]) -> Result<String, FileError>;
    async fn absolute_path(&self, path: &str) -> Result<String, FileError>;
    async fn create_dir(&self, path: &str, options: Option<CreateDirOptions>) -> Result<(), FileError>;
    async fn remove(&self, path: &str, options: Option<RemoveOptions>) -> Result<(), FileError>;
    async fn create_temp_dir(&self, prefix: &str) -> Result<String, FileError>;
    async fn create_temp_file(&self, options: Option<TempFileOptions>) -> Result<String, FileError>;
    async fn exec(&self, command: &str, options: ExecutionEnvExecOptions) -> Result<ExecResult, ExecutionError>;
    async fn cleanup(&self);
}
```

内置实现 `NodeExecutionEnv` 基于 `tokio::fs` + `tokio::process::Command`：

```rust
let env = Arc::new(NodeExecutionEnv::new("/workspace"));

// 读取文件
let content = env.read_text_file("/workspace/src/main.rs", None).await?;

// 执行命令
let result = env.exec("cargo test", ExecutionEnvExecOptions {
    cwd: Some("/workspace".into()),
    env: None,
    abort_signal: None,
    on_stdout: Some(Box::new(|line| println!("[stdout] {}", line))),
    on_stderr: None,
}).await?;
println!("exit: {}, stdout: {}", result.exit_code, result.stdout);
```

### Shell 输出捕获

```rust
use pi_agent_core::harness::utils::shell_output::{
    execute_shell_with_capture,
    ShellCaptureOptions,
    ShellCaptureResult,
};

let result = execute_shell_with_capture(
    env.as_ref(),
    "cargo build 2>&1",
    Some(ShellCaptureOptions {
        max_bytes: Some(100_000),    // 最多捕获 100KB
        abort_signal: Some(cancel_rx),
        on_chunk: Some(Box::new(|chunk| {
            print!("{}", chunk);
        })),
    }),
).await?;

// result: ShellCaptureResult
println!("output: {}", result.output);
println!("exit_code: {:?}", result.exit_code);
println!("cancelled: {}", result.cancelled);
println!("truncated: {}", result.truncated);
```

### 文本截断

```rust
use pi_agent_core::harness::utils::truncate::{
    truncate_head, truncate_tail, TruncationOptions, TruncationResult
};

let result = truncate_head(long_text, TruncationOptions {
    max_lines: Some(100),
    max_bytes: Some(50_000),
});
// result.content, result.truncated, result.total_lines, result.output_lines
```

### 上下文压缩

```rust
use pi_agent_core::harness::compaction::compaction::{
    prepare_compaction, compact, should_compact,
    estimate_context_tokens, calculate_context_tokens,
    generate_summary, find_cut_point,
};
use pi_agent_core::harness::types::{CompactionSettings, DEFAULT_COMPACTION_SETTINGS};

// 估算 token
let total = estimate_context_tokens(system_prompt, &messages);

// 检查是否需要压缩
if should_compact(total, model.context_window, &DEFAULT_COMPACTION_SETTINGS) {
    let prep = prepare_compaction(
        &entries,
        model.context_window,
        &CompactionSettings {
            reserve_tokens: 16384,
            keep_recent_tokens: 8192,
            ..Default::default()
        },
    )?;

    let result = compact(
        prep,
        &model,
        &api_key,
        None,                   // headers
        None,                   // custom_instructions
        None,                   // signal
        None,                   // thinking_level
    ).await?;

    println!("summary: {}", result.summary);
    println!("tokens_before: {}", result.tokens_before);
    println!("first_kept_entry_id: {}", result.first_kept_entry_id);
}
```

### 分支摘要

```rust
use pi_agent_core::harness::compaction::branch_summarization::{
    generate_branch_summary, prepare_branch_entries,
};
use pi_agent_core::harness::types::GenerateBranchSummaryOptions;

let result = generate_branch_summary(
    &entries,
    &GenerateBranchSummaryOptions {
        model: model.clone(),
        reserve_tokens: Some(4096),
        custom_instructions: None,
        replace_instructions: None,
    },
).await?;

println!("summary: {}", result.summary);
println!("read_files: {:?}", result.read_files);
println!("modified_files: {:?}", result.modified_files);
```

### 技能（Skills）加载

```rust
use pi_agent_core::harness::skill_loader::load_skills_from_directories;
use pi_agent_core::harness::skills::format_skills_for_system_prompt;

let (skills, diagnostics) = load_skills_from_directories(
    env.as_ref(),
    &[".claude/skills", "skills"],
).await;

let formatted = format_skills_for_system_prompt(&skills);
// 返回 XML 格式的技能描述，可用于 system prompt
```

### 提示词模板

```rust
use pi_agent_core::harness::prompt_templates::{
    load_prompt_templates,
    format_prompt_template_invocation,
    substitute_args,
    parse_command_args,
    PromptTemplateDiagnostic,
};
use pi_agent_core::harness::env::nodejs::NodeExecutionEnv;

let env = NodeExecutionEnv::new(".");
let (templates, diagnostics) = load_prompt_templates(&env, "templates").await;
for d in &diagnostics {
    eprintln!("[{}] {}: {}", d.diagnostic_type, d.code, d.message);
}
let result = format_prompt_template_invocation(&templates[0], &["arg1".into(), "arg2".into()]);
```

## 消息转换辅助函数

```rust
use pi_agent_core::harness::messages::{
    convert_to_llm,
    bash_execution_to_text,
    create_branch_summary_message,
    create_compaction_summary_message,
    create_custom_message,
};

// BashExecution 转文本
let text = bash_execution_to_text(
    "ls -la", "file1\nfile2", Some(0), false, false, None
);

// 创建特殊消息
let branch_msg = create_branch_summary_message("Summary".into(), "from-id".into(), timestamp);
let compact_msg = create_compaction_summary_message("Summary".into(), 50000, timestamp);
let custom_msg = create_custom_message(
    "note".into(),
    CustomContent::Text("Hello".into()),
    true,
    None,
    timestamp,
);
```

## 完整示例

以下是一个完整的 agent 应用示例（需要集成 `pi-ai` 的 LLM provider）：

```rust
use std::sync::Arc;
use chrono::Utc;
use pi_agent_core::agent::{Agent, AgentOptions};
use pi_agent_core::types::{
    AgentMessage, AgentTool, AgentToolResult, DynTool, AgentEvent,
    ConvertToLlmFn, StreamFn,
};
use pi_agent_core::pi_ai_types::{Model, ContentBlock, ToolExecutionMode, text_block};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 注册 built-in providers
    pi_ai::providers::register_builtins::register_built_in_api_providers();

    // 获取模型
    let model = pi_ai::models::get_model("openrouter", "openrouter/auto").unwrap();

    // 定义 StreamFn
    let stream_fn: StreamFn = Arc::new(|model, context, thinking_level, opts| {
        Box::pin(async move {
            let result = pi_ai::stream::stream_request(
                model,
                context,
                pi_ai::types::SimpleStreamOptions {
                    api_key: opts.api_key.clone(),
                    signal: opts.signal.clone(),
                    ..Default::default()
                },
            ).await?;
            Ok(result.stream)
        })
    });

    // 定义消息转换
    let convert_to_llm: ConvertToLlmFn = Arc::new(|msgs| {
        pi_agent_core::harness::messages::convert_to_llm(msgs)
    });

    // 定义工具
    let echo_tool: Arc<DynTool> = Arc::new(AgentTool {
        name: "echo".into(),
        description: "Echo back the input".into(),
        label: "Echo".into(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(|_id, args, _signal, _on_update| {
            Box::pin(async move {
                let msg = args["message"].as_str().unwrap_or("");
                Ok(AgentToolResult {
                    content: vec![text_block(&format!("Echo: {}", msg))],
                    details: serde_json::json!({}),
                    terminate: None,
                })
            })
        }),
    });

    // 创建 Agent
    let agent = Agent::new(AgentOptions {
        stream_fn: Some(stream_fn),
        convert_to_llm: Some(convert_to_llm),
        session_id: Some("demo".into()),
        tool_execution: Some(ToolExecutionMode::Parallel),
        ..Default::default()
    });

    agent.set_system_prompt("You are a helpful assistant with tools.".into()).await;
    agent.set_model(model).await;

    // 订阅事件
    let _handle = agent.subscribe(Arc::new(|event, _| {
        Box::pin(async move {
            if let AgentEvent::MessageUpdate { message, .. } = &event {
                if let AgentMessage::Assistant { content, .. } = message {
                    for block in content {
                        if let ContentBlock::Text { text, .. } = block {
                            print!("{}", text);
                        }
                    }
                }
            }
        })
    })).await;

    // 发送消息
    let msg = AgentMessage::User {
        content: vec![text_block("Hello! What can you do?")],
        timestamp: Utc::now().timestamp_millis(),
    };
    let results = agent.process(vec![msg]).await?;

    // 使用 continue_run
    let continue_msg = AgentMessage::User {
        content: vec![text_block("Tell me more.")],
        timestamp: Utc::now().timestamp_millis(),
    };
    agent.steer(continue_msg).await;
    let more_results = agent.continue_run().await?;

    Ok(())
}
```

## 模块一览

| 模块 | 路径 | 说明 |
|------|------|------|
| `agent` | `src/agent.rs` | Agent 生命周期、事件分发、消息队列 |
| `agent_loop` | `src/agent_loop.rs` | 底层多轮对话循环、流式 LLM 调用、工具执行 |
| `types` | `src/types.rs` | AgentMessage, AgentTool, AgentEvent, 回调签名 |
| `pi_ai_types` | `src/pi_ai_types.rs` | pi-ai 类型 re-export + 辅助函数 + ToolExecutionMode |
| `proxy` | `src/proxy.rs` | SSE 代理流式传输 |
| `harness` | `src/harness/` | 高层封装 |
| `harness::agent_harness` | `src/harness/agent_harness.rs` | AgentHarness |
| `harness::types` | `src/harness/types.rs` | Skill, Session, ExecutionEnv, 压缩, 错误类型 |
| `harness::messages` | `src/harness/messages.rs` | AgentMessage ↔ LLM Message 转换 |
| `harness::compaction` | `src/harness/compaction/` | 上下文压缩、分支摘要 |
| `harness::session` | `src/harness/session/` | JSONL/InMemory 会话存储 |
| `harness::skill_loader` | `src/harness/skill_loader.rs` | SKILL.md 递归加载 |
| `harness::skills` | `src/harness/skills.rs` | Skill XML 格式化 |
| `harness::prompt_templates` | `src/harness/prompt_templates.rs` | 提示词模板 |
| `harness::env/nodejs` | `src/harness/env/nodejs.rs` | NodeExecutionEnv 实现 |
| `harness::utils` | `src/harness/utils/` | 文本截断、Shell 输出捕获 |

## 集成测试

```bash
# 使用 OpenRouter
export OPENROUTER_API_KEY="sk-or-..."
cargo test --test agent_loop_integration_test -- --ignored --nocapture
```

## License

MIT
