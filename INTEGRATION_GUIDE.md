# pi-ai 与 pi-agent-core 打通分析

## 原版 TypeScript 是怎么做的

原版 TypeScript 通过 npm 包共享类型，**没有类型重复**。

### 包依赖关系

```
@earendil-works/pi-ai (底层)
  └─ 导出: Model, Context, Message, ContentBlock, streamSimple, stream, ...

@earendil-works/pi-agent-core (中层)
  ├─ 依赖: @earendil-works/pi-ai
  └─ 导入: { type Model, type Context, streamSimple } from "@earendil-works/pi-ai"

@earendil-works/pi-coding-agent (上层)
  ├─ 依赖: @earendil-works/pi-ai
  ├─ 依赖: @earendil-works/pi-agent-core
  └─ 导入: { streamSimple, Model } from "@earendil-works/pi-ai"
           { Agent, AgentMessage } from "@earendil-works/pi-agent-core"
```

### StreamFn 的定义（types.ts:agent）

```typescript
import type { streamSimple } from "@earendil-works/pi-ai";

// StreamFn 就是 streamSimple 的函数签名，直接用 TypeScript 的 Parameters/ReturnType 推导
export type StreamFn = (
  ...args: Parameters<typeof streamSimple>
) => ReturnType<typeof streamSimple>;
```

**关键点：`StreamFn` 就是 `streamSimple` 本身的类型，不是一个新的类型。**

### agent-loop.ts 中的调用（L298）

```typescript
const streamFunction = streamFn || streamSimple; // 默认直接用 streamSimple

const response = await streamFunction(config.model, llmContext, {
    ...config,
    apiKey: resolvedApiKey,
    signal,
});
```

`streamSimple` 可以作为 `StreamFn` 直接传入，因为它俩是同一个类型。

### sdk.ts 中的 wiring（coding-agent L331-370）

```typescript
agent = new Agent({
    // ...
    streamFn: async (model, context, options) => {
        // 只做了一件事：注入 API key 和 headers
        const auth = await modelRegistry.getApiKeyAndHeaders(model);
        return streamSimple(model, context, {
            ...options,
            apiKey: auth.apiKey,
            headers: { ...attributionHeaders, ...auth.headers, ...options?.headers },
        });
    },
});
```

`sdk.ts` 的 `streamFn` 只是一个薄包装：从 ModelRegistry 获取 API key，然后透传给 `streamSimple`。

### convertToLlm（messages.ts:coding-agent）

```typescript
// 将 coding-agent 扩展的 AgentMessage 转换为 LLM 能理解的 Message
export function convertToLlm(messages: AgentMessage[]): Message[] {
    return messages
        .filter(m => m.role !== "bashExecution" || !m.excludeFromContext)
        .map(m => {
            if (m.role === "user") return m;           // 原样传递
            if (m.role === "assistant") return m;       // 原样传递
            if (m.role === "toolResult") return m;      // 原样传递
            if (m.role === "bashExecution") {           // 自定义类型 → 转换
                return { role: "user", content: [...] };
            }
            // branchSummary, compactionSummary → 特殊处理
        });
}
```

---

## Rust 版本的问题

### 当前问题：类型重复

```
pi_ai::types::Model          ≠  pi_agent_core::pi_ai_types::Model
pi_ai::types::Context        ≠  pi_agent_core::pi_ai_types::Context
pi_ai::types::Message        ≠  pi_agent_core::pi_ai_types::Message
pi_ai::stream::stream_simple ≠  pi_agent_core::types::StreamFn
```

pi-agent-core 在 `pi_ai_types.rs` 中维护了完整的类型副本。这导致：
1. 两份类型需要手动同步
2. 无法直接传递 `pi_ai::stream::stream_simple` 给 `StreamFn`
3. 需要编写大量适配代码做类型转换

### 正确做法：删除 pi_ai_types.rs，直接依赖 pi-ai

```rust
// pi-agent-core/Cargo.toml
[dependencies]
pi-ai = { path = "../pi-ai" }

// pi-agent-core/src/types.rs
use pi_ai::types::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, Context,
    Message, Model, StopReason, ThinkingLevel, Tool, Usage,
};
use pi_ai::stream::stream_simple;

// StreamFn 就是 stream_simple 的签名
pub type StreamFn = Arc<
    dyn Fn(
        Model,                          // pi_ai::types::Model
        Context,                        // pi_ai::types::Context
        Option<SimpleStreamOptions>,    // pi_ai::types::SimpleStreamOptions
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AssistantMessageEventStream> + Send>>
    + Send + Sync,
>;
```

**这样 `stream_simple` 本身就是一个合法的 `StreamFn`，无需任何适配器。**

### AgentMessage 扩展

原版中，AgentMessage 在 pi-agent-core/types.ts 中定义，包含基础的 user/assistant/toolResult 角色，加上 coding-agent 特有的 bashExecution/custom/branchSummary/compactionSummary。

Rust 中保持同样结构：
- `pi_agent_core::types::AgentMessage` — 基础角色 + 扩展角色（与 pi_ai::types::Message 不同）
- `ConvertToLlmFn` — 负责 `AgentMessage → Vec<Message>` 的转换

---

## 修正方案

### 步骤 1：删除 pi-agent-core/src/pi_ai_types.rs

将其中定义的类型替换为 `pi_ai::types` 的直接引用。

### 步骤 2：修改 pi-agent-core 的 types.rs

```rust
// types.rs — 只定义 Agent 层特有的类型
use pi_ai::types::{
    AssistantMessage, AssistantMessageEvent, ContentBlock, Context,
    Message, Model, SimpleStreamOptions, StopReason, ThinkingLevel,
    ThinkingBudgets, Tool, Usage,
};

// AgentMessage 扩展了 pi_ai::Message，增加了 Agent 层特有的消息类型
pub enum AgentMessage {
    User { content: Vec<ContentBlock>, timestamp: i64 },
    Assistant { content: Vec<ContentBlock>, ... },
    ToolResult { ... },
    // Agent 层特有的：
    BashExecution { command, output, ... },
    Custom { custom_type, content, ... },
    BranchSummary { summary, ... },
    CompactionSummary { summary, ... },
}

// ConvertToLlmFn — 将 AgentMessage 转为 pi_ai::Message
pub type ConvertToLlmFn = Arc<dyn Fn(&[AgentMessage]) -> Vec<Message> + Send + Sync>;

// StreamFn — 和 pi_ai::stream::stream_simple 相同的签名
pub type StreamFn = Arc<
    dyn Fn(Model, Context, Option<SimpleStreamOptions>)
        -> Pin<Box<dyn Future<Output = AssistantMessageEventStream> + Send>>
    + Send + Sync,
>;
```

### 步骤 3：在 agent_loop.rs 中直接使用 pi-ai 类型

```rust
use pi_ai::types::{Context, Message, Model, AssistantMessageEvent};
use pi_ai::stream::stream_simple;

async fn stream_assistant_response(
    context: &AgentContext,
    config: &AgentLoopConfig,
    // ...
) -> Result<pi_ai::types::AssistantMessage, ...> {
    // ConvertToLlm: AgentMessage → pi_ai::Message
    let llm_messages = (config.convert_to_llm)(&context.messages);

    let pi_context = pi_ai::types::Context {
        system_prompt: Some(context.system_prompt.clone()),
        messages: llm_messages,
        tools: context.tools.as_ref().map(|t| convert_tools(t)),
    };

    // 直接用 streamSimple 或用户提供的 stream_fn
    let stream = match &config.stream_fn {
        Some(f) => f(config.model.clone(), pi_context, thinking_level, options).await?,
        None => stream_simple(&config.model, &pi_context, Some(&simple_options)),
    };

    // 消费事件流...
}
```

### 步骤 4：coding-agent 中 wiring 变得简单

```rust
use pi_agent_core::agent::Agent;
use pi_ai::stream::stream_simple;  // 这就是 StreamFn

let agent = Agent::new(AgentOptions {
    model: pi_ai::models::get_model("anthropic", "claude-sonnet-4-6").unwrap(),
    stream_fn: Some(Arc::new(move |model, context, options| {
        // 和原版一样：注入 API key
        let api_key = get_api_key_from_env_or_config();
        Box::pin(stream_simple(&model, &context, Some(SimpleStreamOptions {
            api_key: Some(api_key),
            ..options.unwrap_or_default()
        })))
    })),
    convert_to_llm: Some(make_convert_to_llm()),
    // ...
});
```

---

## 对比总结

| | 原版 TypeScript | 当前 Rust | 修正后 Rust |
|------|------|------|------|
| 类型来源 | pi-ai 导出，所有包共享 | pi_ai_types.rs 副本 | pi_ai::types 直接引用 |
| StreamFn | `typeof streamSimple` | 自定义函数签名 | 等于 stream_simple 签名 |
| wiring | `streamFn \|\| streamSimple` | 需要适配器 | `Some(wrapper) \|\| stream_simple` |
| convertToLlm | 1:1 字段映射 | 需要手动转换 | 1:1 映射（ContentBlock 一致） |
| API key 注入 | sdk.ts 包装 streamSimple | 需要在适配器中处理 | AgentOptions 中包装 |
| 适配代码量 | 0 行 | ~200 行 | ~10 行（API key 注入） |
