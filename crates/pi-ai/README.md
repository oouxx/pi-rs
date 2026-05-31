# pi-ai

统一 LLM API 层，提供流式调用、模型发现、Token 成本跟踪。Rust 复刻自 [`@earendil-works/pi-ai`](https://github.com/earendil-works/pi/tree/main/packages/ai)。

支持 Anthropic Messages、OpenAI Chat Completions、DeepSeek、xAI Grok、Mistral 等多个 provider。

## 快速开始

```rust
use pi_ai::models::get_model;
use pi_ai::stream::stream;
use pi_ai::providers::register_builtins::register_built_in_api_providers;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. 注册内置 provider（只需调用一次）
    register_built_in_api_providers();

    // 2. 查找模型
    let model = get_model("anthropic", "claude-sonnet-4-6")
        .expect("Model not found");

    // 3. 构建上下文
    let context = pi_ai::types::Context {
        system_prompt: Some("You are a helpful assistant.".into()),
        messages: vec![pi_ai::types::Message::User {
            content: vec![pi_ai::types::ContentBlock::Text {
                text: "Hello!".into(),
                text_signature: None,
            }],
            timestamp: 0,
        }],
        tools: None,
    };

    // 4. 流式调用
    let event_stream = stream(model, &context, None);
    let result = event_stream.result().await?;
    println!("Response: {:?}", result);

    Ok(())
}
```

## 支持的 Provider

| Provider | API | 说明 |
|----------|-----|------|
| Anthropic | `anthropic-messages` | Claude 系列模型 |
| OpenAI | `openai-completions` | GPT 系列模型 |
| DeepSeek | `openai-completions` | 复用 OpenAI 兼容格式 |
| xAI Grok | `openai-completions` | 复用 OpenAI 兼容格式 |
| Mistral | `mistral-conversations` | 复用 OpenAI 兼容格式 |
| Google | `openai-completions` | Gemini 系列 |
| Groq | `openai-completions` | 高性能推理 |
| Cerebras | `openai-completions` | 超快推理 |
| Together | `openai-completions` | 开源模型托管 |
| Fireworks | `openai-completions` | 多模型平台 |
| OpenRouter | `openai-completions` | 多 provider 路由 |
| 更多... | | |

## 核心模块

| 模块 | 说明 |
|------|------|
| `stream` | `stream()` / `complete()` / `stream_simple()` 入口 |
| `models` | `get_model()` / `get_providers()` / `calculate_cost()` |
| `models_generated` | 15+ provider、30+ 模型的元数据 |
| `types` | `Model`, `Message`, `ContentBlock`, `Usage`, `StreamOptions` 等 |
| `providers::anthropic` | Anthropic Messages API SSE streaming |
| `providers::openai` | OpenAI Chat Completions SSE streaming |
| `providers::deepseek` | DeepSeek（委托给 openai） |
| `providers::xai` | xAI Grok（委托给 openai） |
| `providers::register_builtins` | 注册内置 provider |
| `utils::sse` | SSE 协议解析器（Anthropic + OpenAI 两种格式） |
| `utils::event_stream` | 事件流包装器 |
| `api_registry` | Provider 注册/查找机制 |
| `env_api_keys` | 环境变量 → API key 映射（25 provider） |

## 类型系统

```rust
// 内容块
pub enum ContentBlock {
    Text { text: String, text_signature: Option<String> },
    Thinking { thinking: String, thinking_signature: Option<String>, redacted: Option<bool> },
    ToolCall { id: String, name: String, arguments: Value, thought_signature: Option<String> },
    Image { data: String, mime_type: String },
}

// 消息
pub enum Message {
    User { content: Vec<ContentBlock>, timestamp: i64 },
    Assistant { content: Vec<ContentBlock>, api, provider, model, usage, stop_reason, ... },
    ToolResult { tool_call_id, tool_name, content, details, is_error, timestamp },
}

// 流式事件
pub enum AssistantMessageEvent {
    Start { partial: AssistantMessage },
    TextStart { content_index, partial },
    TextDelta { content_index, delta, partial },
    TextEnd { content_index, content, partial },
    ThinkingStart { content_index, partial },
    ThinkingDelta { content_index, delta, partial },
    ThinkingEnd { content_index, content, partial },
    ToolCallStart { content_index, partial },
    ToolCallDelta { content_index, delta, partial },
    ToolCallEnd { content_index, tool_call: ToolCall, partial },
    Done { reason: StopReason, message: AssistantMessage },
    Error { reason: StopReason, error: AssistantMessage },
}
```

## 环境变量

| Provider | 环境变量 |
|----------|---------|
| Anthropic | `ANTHROPIC_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| DeepSeek | `DEEPSEEK_API_KEY` |
| xAI | `XAI_API_KEY` |
| Google | `GOOGLE_API_KEY` |
| Groq | `GROQ_API_KEY` |
| OpenRouter | `OPENROUTER_API_KEY` |
| Mistral | `MISTRAL_API_KEY` |
| Cerebras | `CEREBRAS_API_KEY` |
| Together | `TOGETHER_API_KEY` |
| Fireworks | `FIREWORKS_API_KEY` |
| ... | 更多见 `env_api_keys.rs` |

## 集成测试

使用 OpenRouter 免费模型验证端到端联通性：

```bash
export OPENROUTER_API_KEY="sk-or-..."
cargo test --test openrouter_integration_test -- --ignored --nocapture
```

**测试输出：**

```
running 4 tests

test test_pi_ai_stream_simple_text ... ok
  response: Hello! How can I assist you today?
  stop_reason: Stop

test test_pi_ai_complete_simple ... ok
  Complete OK — 82 chars, 52 input tokens

test test_pi_ai_stream_with_temperature ... ok
  response: 1 + 1 = 2!
  stop_reason: Length (max_tokens=100)

test test_pi_ai_stream_with_system_prompt ... ok
  response: The capital of France is Paris...

test result: ok. 4 passed; 0 failed; finished in 3.26s
```

默认使用 `poolside/laguna-xs.2:free`，可通过 `PI_TEST_MODEL` 环境变量覆盖。

## License

MIT
