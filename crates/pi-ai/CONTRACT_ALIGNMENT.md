# CONTRACT_ALIGNMENT.md — pi-ai

对齐基准：earendil-works/pi **v0.80**（见根目录 `MILESTONE.md`）。
本表覆盖 pi-ai 公开 API 的行为对照；"是否一致"为"否"的条目必须引用
`DEVIATIONS.md` 或 `PORTING_MISTAKES.md` 对应记录。

## Usage / 成本计算

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| `Usage.reasoning` | 可选字段，provider 报告 reasoning/thinking token 时设置（output 的子集） | `Option<u64>`，openai-completions 恒为 `Some`（缺省 0），anthropic 有 `thinking_tokens` 时设置 | 是 | 修复见 PORTING_MISTAKES.md |
| `Usage.cacheWrite1h` | 可选字段，Anthropic `cache_creation.ephemeral_1h_input_tokens` | `Option<u64>`，anthropic message_start 时设置 | 是 | 修复见 PORTING_MISTAKES.md |
| openai-completions usage 解析 | input = max(0, prompt − cacheRead − cacheWrite)；cacheRead 支持 `prompt_cache_hit_tokens` fallback；cacheWrite 读 `cache_write_tokens`；reasoning 读 `completion_tokens_details.reasoning_tokens` | 同左 | 是 | 修复见 PORTING_MISTAKES.md |
| `calculateCost` 1h cache write | `cacheWrite1h` 按 2x input 计价，其余按 cacheWrite 价 | 同左 | 是 | 修复见 PORTING_MISTAKES.md |
| `calculateCost` pricing tiers | `model.cost.tiers` 按请求总 input 选最高匹配档 | 同左 | 是 | 修复见 PORTING_MISTAKES.md |

## 思考级别

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 思考级别列表 | `["off","minimal","low","medium","high","xhigh","max"]` | 同左 | 是 | 修复见 PORTING_MISTAKES.md |
| 无 `thinkingLevelMap` 时 xhigh/max | 不支持（需显式声明） | 同左 | 是 | 修复见 PORTING_MISTAKES.md |

## Anthropic provider

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| stop reason `refusal` | error + `stop_details.explanation`（缺省 "The model refused to complete the request"） | 同左 | 是 | 修复见 PORTING_MISTAKES.md |
| stop reason `sensitive` | error "Provider stopped with: sensitive" | 同左 | 是 | 修复见 PORTING_MISTAKES.md |
| 未知 stop reason | throw `Unhandled stop reason: X` | panic `Unhandled stop reason: X` | 是 | 修复见 PORTING_MISTAKES.md |
| thinking 块回放 | redacted → `redacted_thinking` 透传；带 signature → 保留；空 signature → 转纯文本（或 allowEmptySignature 保留） | 同左 | 是 | 修复见 PORTING_MISTAKES.md |

## OpenAI-completions provider

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| overflow 检测括号形式 | `maximum context length (N)` 判为 overflow | 同左 | 是 | 修复见 PORTING_MISTAKES.md |

## 未覆盖项（相对 v0.81）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| `ModelRuntime` / live catalog refresh | v0.80.8 引入，启动/后台自动刷新 | 手动 `pi refresh` 有界子集 | 否 | 见 DEVIATIONS.md #10（已确认保留） |
| 模型覆盖度 | 全 provider + 手工补充模型 | 13 provider，元数据不全 | 否 | 见 DEVIATIONS.md #2（已确认保留） |
| Qwen Token Plan provider | v0.81.0 #6858 新增 | 未实现 | 否 | 见 DEVIATIONS.md #2（已确认保留） |
| `contentText` / `uuidv7` 工具 | v0.81.0 #6840/#6834 新增 | 已实现（`utils/text.rs`/`utils/uuid.rs`） | 是 | 修复见 PORTING_MISTAKES.md |
| `retryAssistantCall()` | v0.81.0 #6901 新增 | 已实现（`utils/retry.rs`） | 是 | 修复见 PORTING_MISTAKES.md |

## OpenAI Responses provider（`providers/openai_responses.rs`）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 请求端点 | `POST {baseUrl}/responses` | 同左 | 是 | — |
| 请求体 | `{model, input, stream, store:false, prompt_cache_key, max_output_tokens, temperature, tools, reasoning, include}` | 同左（`max_output_tokens` 下限 16、`prompt_cache_key` 截断 64） | 是 | — |
| system prompt 角色 | reasoning 模型 → `developer`，否则 `system` | 同左 | 是 | — |
| 消息转换 | `convertResponsesMessages`（input_text/input_image/message/function_call/function_call_output） | 同左 | 是 | — |
| tool-call id 归一化 | `normalizeIdPart` + `fc_` 前缀 + `shortHash` | 同左（`short_hash` 与 TS 算法一致，base36） | 是 | — |
| text signature | `encodeTextSignatureV1`/`parseTextSignature`（`{v:1,id,phase}`） | 同左 | 是 | — |
| reasoning 回放 | thinking block 带 signature → 透传 reasoning item | 同左 | 是 | — |
| 流式事件序列 | `response.output_item.added` → `thinking_start`/`text_start`/`toolcall_start`；delta 事件 → `*_delta`；`output_item.done` → `*_end`；`response.completed` → usage/stop-reason | 同左 | 是 | — |
| usage 解析 | input 减 cached+write、reasoning 从 `output_tokens_details` | 同左 | 是 | — |
| stop reason 映射 | completed→stop、incomplete+max_output_tokens→length、failed/cancelled→error、有 toolCall 且 stop→toolUse | 同左 | 是 | — |
| grammar 约束采样 | `custom_tool_call` + grammar JSON buffer | 已实现（`Tool.constrained_sampling` + `GrammarVariants` + 增量重建） | 是 | 见 DEVIATIONS.md #5 |
| deferred tools | `tool_search_call`/`tool_search_output` | 已实现（`split_deferred_tools` + `defer_loading:true`） | 是 | 见 DEVIATIONS.md #5 |
| service tier 定价 | flex/priority 倍率 | 已实现 | 是 | 见 DEVIATIONS.md #5 |
| `rawStopReason` | 输出字段 | 已实现 | 是 | 见 DEVIATIONS.md #5 |
| 早期流结束重试 | "stream ended before a terminal response event" 分类为可重试（v0.81.0 #6727） | `_is_retryable_error_message` 已含该模式 | 是 | 修复见 PORTING_MISTAKES.md |
| tool-call id 归一化（openai-completions） | pipe 分隔 ID 归一化 + isSameModel 门控 + toolCallIdMap（v0.81.0 #6854） | `normalize_tool_call_id` 已移植 | 是 | 修复见 PORTING_MISTAKES.md |
| `ToolResultMessage.usage` | 工具可报告 LLM usage（v0.81.0 #6671） | `Message::ToolResult.usage: Option<Usage>` 已加 | 是 | 修复见 PORTING_MISTAKES.md |
