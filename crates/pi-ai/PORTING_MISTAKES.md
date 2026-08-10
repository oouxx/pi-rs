# PORTING_MISTAKES.md — pi-ai

对齐检查（v0.79 → v0.80）中修复的回归 bug 归档。根因模式尽量归到
`PORTING.md` 高危陷阱表已有分类；新出现的模式已同步回该表。

| 位置 | 现象 | 根因模式 | 修复方式 |
| ---- | ---- | -------- | -------- |
| `utils/overflow.rs` OVERFLOW_PATTERNS | 括号形式 `maximum context length (262144)` 不判为 overflow（TS v0.79.2 #5677 已支持） | 正则翻译遗漏分支（`of [\d,]+ tokens?` 与 `\s*\([\d,]+\)` 二选一） | 补上 `\s*\([\d,]+\)` 分支，与 TS 正则一致 |
| `types.rs` `Usage` | 缺 `reasoning`（TS v0.80.3 #6057）与 `cacheWrite1h`（TS v0.79.4 #5738 配套）字段 | 类型定义翻译遗漏新增字段 | 补 `reasoning: Option<u64>`、`cache_write_1h: Option<u64>`（serde default + skip none） |
| `providers/openai.rs` `parse_chunk_usage` | input 未扣除 cacheRead/cacheWrite；`prompt_cache_hit_tokens` fallback 缺失；`cache_write_tokens` 恒 0；无 `reasoning` | 数值语义翻译不完整（TS 的 `Math.max(0, prompt - cacheRead - cacheWrite)` 与字段 fallback 未逐项翻译） | 按 TS `parseChunkUsage` 逐项对齐，补测试 |
| `providers/anthropic.rs` `AnthropicUsage` | 未解析 `cache_creation.ephemeral_1h_input_tokens` 与 `output_tokens_details.thinking_tokens` | 流式事件字段翻译遗漏（嵌套对象） | 补嵌套结构体解析，message_start 填 `cache_write_1h`、message_delta 填 `reasoning` |
| `providers/anthropic.rs` `map_stop_reason` | `refusal`/`sensitive`/未知 stop reason 全部静默映射为 Stop（TS v0.79.2 #5666 要求 error + explanation / throw） | 静默 fallback（CLAUDE.md 高危陷阱：`?? defaultValue` 掩盖异常） | 按 TS `mapStopReason` 对齐：refusal→Error+explanation、sensitive→Error、未知→panic |
| `providers/anthropic.rs` `convert_messages` | 所有 thinking 块被丢弃，不回放（TS v0.80.6 #6457 要求 redacted 透传、带 signature 保留、空 signature 转纯文本） | 功能翻译遗漏（"Thinking blocks are not sent back" 是旧版行为，v0.80 已改） | 按 TS `convertMessages` 实现 thinking 回放，`allowEmptySignature` 走 compat |
| `models.rs` `get_supported_thinking_levels` | 无 `thinkingLevelMap` 时 xhigh/max 被错误地视为支持（TS 要求显式声明才支持） | 逻辑翻译错误（`if let Some(map)` 外层判断导致 None 时全放行） | 改为 match `map.get(level)`：None→非 xhigh/max 放行，Some(None)→不支持，Some(Some)→支持 |
| `models.rs` `calculate_cost` | 无 input-based pricing tiers（TS v0.80.6）与 1h cache write 2x input 定价（TS v0.79.4 #5738） | 功能翻译遗漏 | 按 TS `calculateCost` 实现 tiers 选择 + `cacheWrite1h` 定价 |
| `models.rs` `EXTENDED_THINKING_LEVELS` | 缺 `max` thinking level（TS v0.80.6） | 常量翻译遗漏 | 补 "max"，xhigh/max 同规则 |
| `pi-agent-core` `ExecutionEnvExecOptions` | 未按 TS v0.80.2 重命名为 `ShellExecOptions` | 公开 API 重命名未同步 | 全仓重命名 |
| `providers/openai.rs` `convert_messages` | 跨 provider 回放时 tool call id 直接透传，pipe 分隔 ID（`{call_id}|{item_id}`，来自 Responses API）未归一化，多个共享 call_id 的 tool call 在 Chat Completions 里 id 冲突（TS v0.81.0 #6854） | 功能翻译遗漏：TS `normalizeToolCallId`（pipe 拆分 + 40 字符截断 + shortHash 兜底）未移植；`transformMessages` 的 isSameModel 门控 + toolCallIdMap 也未移植 | 移植 `normalize_tool_call_id`（对齐 TS：pipe 拆分、非法字符替换、>40 用 shortHash、openai provider 截断 40）；assistant 分支算 isSameModel，仅跨模型归一化并记录 toolCallIdMap；toolResult 分支用映射后的 id |
| `types.rs` `Message::ToolResult` / `AgentMessage::ToolResult` / `AgentToolResult` | 缺 `usage` 字段（TS v0.81.0 #6671：工具可报告 LLM usage，透传到 ToolResultMessage） | 类型定义翻译遗漏新增字段 | 三处类型加 `usage: Option<Usage>`（serde skip none），`create_tool_result_message` 透传 `finalized.result.usage`，补透传测试 |
| `utils/retry.rs`（新增） | 无 `retryAssistantCall`/`isRetryableAssistantError`/`RetryPolicy`（TS v0.81.0 #6901：bounded retries + 生命周期回调 + abort 归一化） | 功能翻译遗漏（新 API） | 移植 `retry_assistant_call`（abort 归一化为 aborted AssistantMessage、非 retryable 快速失败、指数退避 + abortable sleep）、`is_retryable_assistant_error`（NON_RETRYABLE/RETRYABLE 模式表）、`RetryPolicy`/`RetryCallbacks`；补 6 个测试 |
| `utils/text.rs`（新增） | 无 `contentText`（TS v0.81.0 #6840：从 message content 提取 joined text） | 功能翻译遗漏（新 API） | 移植 `content_text`（只取 text block，join separator）；补 3 个测试 |
| `utils/uuid.rs`（新增） | 无 `uuidv7`（TS v0.81.0 #6834：时间有序 ID，单调 sequence 保证同毫秒有序） | 功能翻译遗漏（新 API） | 移植 `uuid_v7`（全局 Mutex 状态 + 单调 sequence + 版本/变体位）；补 2 个测试 |
| `env_api_keys.rs` + `providers/anthropic.rs` | 无 `ANTHROPIC_AUTH_TOKEN` bearer 认证（TS v0.82.1 #5871/#6148：Anthropic 兼容网关用 `Authorization: Bearer`；`getEnvApiKey` 跳过它） | 功能翻译遗漏（新认证方式） | `get_env_var_names` 返回 anthropic 的 3 个 env var；`get_env_api_key` 跳过 AUTH_TOKEN（OAUTH_TOKEN 优先于 API_KEY）；anthropic 请求侧 resolve 顺序：explicit api_key → AUTH_TOKEN Bearer → env api_key |
| `utils/retry.rs` + `agent_session.rs` | retryable 模式缺 DNS 失败（`getaddrinfo`/`ENOTFOUND`/`EAI_AGAIN`，TS v0.82.0 #6946） | 常量翻译遗漏 | 两处 retryable_patterns 补 3 个模式 |
| `providers/openai.rs` | Moonshot 等 provider 把 usage 放在 `choice.usage` 而非标准 `chunk.usage`（TS v0.81.0 后支持 fallback） | 响应解析遗漏（fallback 缺失） | `stream_openai_inner` 在 chunk 无 usage 时读 `choice.usage` 解析（对齐 TS `if (!chunk.usage && choice.usage)`） |
| `providers/anthropic.rs:map_stop_reason` | 未知 stop_reason 时 `panic!`——stream task 崩溃、消息丢失；TS 是 `throw`（被 catch 归一化为 error 消息） | 高危陷阱：panic vs throw——Rust panic 中断 task，TS throw 走错误路径 | unknown → `StopReason::Error` + "Unhandled stop reason: {reason}"（不 panic）；更新 should_panic 测试 |
| `utils/provider_retry.rs`（新增） | 无 HTTP 请求层重试（TS `provider-retry.ts`：`retryProviderRequest`，OpenAI/Anthropic SDK 重试策略：x-should-retry 头、408/409/429/5xx/网络错误、retry-after-ms/retry-after 头、指数退避+抖动、maxRetryDelayMs 限制、abort 可中断）——4 个 provider 网络抖动/5xx 直接失败 | 功能翻译遗漏（新模块） | 移植 `retry_provider_request`/`send_with_retry`（`ProviderHttpError` 带 status/retry 头/source）；openai/anthropic/pi_messages/openai_responses 4 个 provider 请求路径全部接入（openai_responses 的本地重复实现删除，统一共享模块）；pi_messages 保留 `PiMessagesResponseError` 结构化诊断（经 `ProviderHttpError.source` downcast）；补 7 个单元测试 |
