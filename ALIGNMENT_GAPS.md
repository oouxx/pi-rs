# 对齐差距清单（v0.79 → v0.80）

> 对齐目标：earendil-works/pi **v0.80**（见 `MILESTONE.md`）。
> 本清单从 v0.79.0 → v0.80.10 的 releases changelog（https://github.com/earendil-works/pi/releases）
> 中筛出与 pi-ai / pi-agent-core / pi-coding-agent / pi-cli 相关的变更，逐项核对 pi-rs 现状。
>
> 分类：**A** = 回归 bug（翻译遗漏/实现错误，直接修）；**B** = 未登记偏差（需确认或暂缓）；
> **C** = 范围外（TUI/打包/文档）；**D** = 已登记偏差（见各 crate `DEVIATIONS.md`）。

## pi-ai

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 1 | v0.79.2 #5677 | overflow 检测支持括号形式 `maximum context length (N)` | `overflow.rs` 只匹配 `of [\d,]+ tokens?`，缺 `\s*\([\d,]+\)` 分支 | A | ✅ 已修 |
| 2 | v0.80.3 #6057 | `Usage.reasoning`（reasoning/thinking token，output 的子集） | `Usage` 缺 `reasoning` 字段 | A | ✅ 已修 |
| 3 | v0.79.4 #5738 | Anthropic 1h cache write 按 2x input 计价 | `calculate_cost` 无 `cacheWrite1h` 处理 | A | ✅ 已修 |
| 4 | v0.80.3 | `Usage.cacheWrite1h`（Anthropic `cache_creation.ephemeral_1h_input_tokens`） | `Usage` 缺字段，anthropic.rs 未解析 | A | ✅ 已修 |
| 5 | — | openai-completions `parseChunkUsage`：input 减 cacheRead/cacheWrite、`prompt_cache_hit_tokens` fallback、`cache_write_tokens`、`completion_tokens_details.reasoning_tokens` | `parse_chunk_usage` 直接取 prompt_tokens、cache_write 恒 0、无 reasoning | A | ✅ 已修 |
| 6 | v0.79.2 #5666 | Anthropic `refusal` → error + `stop_details.explanation`；`sensitive` → error；未知 stop reason → throw | `map_stop_reason` 全部静默映射为 Stop | A | ✅ 已修 |
| 7 | v0.80.6 #6457 | Anthropic thinking 块回放：redacted → `redacted_thinking` 透传；带 signature → 保留 thinking；空 signature → 转纯文本（或 allowEmptySignature 保留） | `convert_messages` 丢弃所有 thinking 块 | A | ✅ 已修 |
| 8 | v0.80.6 | `max` thinking level（EXTENDED_THINKING_LEVELS 含 "max"，xhigh/max 需 map 显式声明） | `EXTENDED_THINKING_LEVELS` 缺 "max" | A | ✅ 已修 |
| 9 | v0.80.6 | input-based pricing tiers（`model.cost.tiers`） | `ModelCost` 缺 `tiers` | A | ✅ 已修 |
| 10 | v0.80.2 | `ApiKeyCredential` discriminator `type: "api_key"` + auth.json `env` 覆盖 | pi-ai 无 credential 系统（coding-agent 有 `auth_storage.rs`） | B | 待确认 |
| 11 | v0.80.2 #6020 | openai-completions 运行时 `detectCompat` fallback（无显式 compat 的模型按 baseUrl 推断） | pi-rs 无 detectCompat | B | 待确认 |
| 12 | v0.79.9 #5673 | `chat_template_kwargs` thinking（vLLM/HF chat-template 模型） | pi-rs 无 | B | 待确认 |
| 13 | v0.79.10 #5114 | OpenAI-compatible 流式保留 reasoning_details（先于 tool call delta 到达） | 已核对：openai.rs 流式按 `reasoning_content`/`reasoning`/`reasoning_text` 处理 reasoning 块，但原实现遍历**全部**非空字段重复追加（如 chutes.ai 同时返回 reasoning_content 与 reasoning 相同内容会重复）；已改为仅取**第一个**非空字段（对齐 TS openai-completions），并补 opencode-go `reasoning`→`reasoning_content` signature 映射 | A | ✅ 已修 |
| 14 | v0.80.7 #6496 | `compat.sessionAffinityFormat`（替代 `sendSessionIdHeader`，breaking） | `OpenAIResponsesCompat` 已更新为 v0.80.7 字段（sessionAffinityFormat/supportsDeveloperRole/supportsStrictMode 等），provider 已按格式发 session affinity headers | B | ✅ 已修 |
| 15 | v0.80.0+ | `openai-responses` API 后端（openai 官方 provider 用 `/v1/responses`） | 已实现 `openai_responses.rs`（消息/工具转换 + SSE 流式 + 事件序列），openai 模型已切到 `openai-responses`；简化项见 DEVIATIONS.md #5 | B | ✅ 已修（含简化偏差） |
| 16 | v0.80.8 | `ModelRuntime` / live model catalog refresh | 未实现 | B | 大项，待排期 |
| 17 | v0.80.3/8/9/10 | 模型元数据：Claude Sonnet 5、Grok 4.5、Kimi K3、默认 gpt-5.5、openrouter/fusion、context window 272k 等 | 模型覆盖度受限 | D | DEVIATIONS #2（待确认） |
| 18 | v0.79.5 #5790 | 全局 `httpProxy` 设置 | pi-rs 无 | B | 待确认 |
| 19 | v0.79.5 #5798 | Vercel AI Gateway attribution headers | 无 Vercel provider | D | DEVIATIONS #2 |

## pi-agent-core

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 20 | v0.80.2 | `ExecutionEnvExecOptions` → `ShellExecOptions` 重命名 | 仍用旧名 `ExecutionEnvExecOptions` | A | ✅ 已修 |
| 21 | v0.79.2 #5573 | 工具结算后的迟到 progress 回调忽略（不发 stale `tool_execution_update`） | `agent_loop.rs` 有 `accepting_updates` 标志，结算后忽略迟到更新 | A | ✅ 已实现 |
| 22 | v0.80.0 #5999 | session 名称换行符规范化 | `session_manager.rs` 有 `\r\n` → 空格 + trim 处理 | A | ✅ 已实现 |

## pi-coding-agent

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 23 | v0.79.10 #5962 | 扩展 `session_before_compact`/`session_compact` 事件带 `reason`/`willRetry` | `dispatch_session_before_compact(registry, reason, will_retry, ...)` 已带参数 | A | ✅ 已实现 |
| 24 | v0.79.10 #5960 | `find` 工具尊重嵌套 git 仓库边界（父 `.gitignore` 在嵌套仓库边界处停止） | 已改用 `ignore` crate（gitignore-aware walker）：hidden 包含、`.gitignore`/`.ignore`/`.git/info/exclude` 尊重、git 仓库内父规则在嵌套边界停止、仓库外等价 `--no-require-git` | A | ✅ 已修 |
| 25 | v0.79.9 #5899 | fuzzy `edit` 保留未触碰行块 | `edit.rs` 有 fuzzy fallback 匹配 | A | ✅ 已实现 |
| 26 | v0.79.4 #5753 | bash 工具子进程退出后继续 drain stdout/stderr | `bash_executor.rs` stdout/stderr 任务 `read_to_end` 到 EOF，子进程退出后仍 drain | A | ✅ 已实现 |
| 27 | v0.79.8 #5877 | 压缩结果/事件带压缩后 token 估算 | `estimate_agent_messages_tokens` 已实现 | A | ✅ 已实现 |
| 28 | v0.80.3 #6175 | 扩展 `session_info_changed` 事件 | `agent_session.rs` 有 `SessionInfoChanged` + dispatch | A | ✅ 已实现 |
| 29 | v0.79.7 #5869/#5756 | 导出 `CONFIG_DIR_NAME`、edit diff helpers | `config::CONFIG_DIR_NAME` 存在 | A | ✅ 已实现 |
| 30 | v0.79.0 #5332 | project trust（项目信任门控） | `project_trust.rs` 已实现 | A | ✅ 已实现 |

## pi-cli

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 31 | v0.79.7 #5868 | RPC unknown-command 错误带 request id | `mod.rs` 已按 TS 返回 `error(id, type, "Unknown command: ...")` | A | ✅ 已实现 |
| 32 | v0.80.3 #6078 | RPC `get_entries`/`get_tree` | `rpc/handler.rs` 已实现 | A | ✅ 已实现 |

## 范围外（C）

- TUI：Ctrl+J 换行（v0.80.0）、主题/自动主题（v0.79.4/7）、`outputPad`/`externalEditor`（v0.80.3）、Warp 图片（v0.79.7）、Markdown 流式渲染等
- 打包/更新：`pi update` 流程、SHA256SUMS、npm 包管理、Bun 二进制
- 文档/示例

## 处理规则

- A 类：直接修，修完跑 `cargo test` + `cargo clippy --all-targets -- -D warnings`，并把真回归补进 `PORTING_MISTAKES.md`。
- B 类：需要用户确认或排期的大项，不在本次会话内擅自实现。
- 待查：逐项核对 pi-rs 现状后归类。
