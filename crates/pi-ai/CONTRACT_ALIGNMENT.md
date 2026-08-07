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

## 未覆盖项（相对 v0.80）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| `openai-responses` API | openai 官方 provider 走 `/v1/responses` | 未实现，openai 模型走 `openai-completions` | 否 | 未登记偏差，见 ALIGNMENT_GAPS.md #15 |
| `ModelRuntime` / live catalog refresh | v0.80.8 引入 | 未实现 | 否 | 未登记偏差，见 ALIGNMENT_GAPS.md #16 |
| `detectCompat` fallback | v0.80.2 恢复运行时推断 | 未实现 | 否 | 未登记偏差，见 ALIGNMENT_GAPS.md #11 |
| `chat_template_kwargs` thinking | v0.79.9 引入 | 未实现 | 否 | 未登记偏差，见 ALIGNMENT_GAPS.md #12 |
| 模型覆盖度 | 全 provider + 手工补充模型 | 13 provider，元数据不全 | 否 | 见 DEVIATIONS.md #2（待确认） |
