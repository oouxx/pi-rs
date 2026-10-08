# 对齐差距清单（v0.81 → v0.82）

> 对齐目标：earendil-works/pi **v0.82**（见 `MILESTONE.md`）。
> 本清单从 v0.81.1 → v0.82.1 的 releases changelog（https://github.com/earendil-works/pi/releases）
> 中筛出与 pi-ai / pi-agent-core / pi-coding-agent / pi-cli 相关的变更，逐项核对 pi-rs 现状。
>
> 分类：**A** = 回归 bug（翻译遗漏/实现错误，直接修）；**B** = 未登记偏差（需确认或暂缓）；
> **C** = 范围外（TUI/打包/文档）；**D** = 已登记偏差（见各 crate `DEVIATIONS.md`）。

## pi-ai

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 1 | v0.82.1 #5871/#6148 | `ANTHROPIC_AUTH_TOKEN` bearer 认证（Anthropic 兼容网关，`Authorization: Bearer`；`getEnvApiKey` 跳过它，请求侧单独处理） | `env_api_keys.rs`/`anthropic.rs` 无此支持 | A | ✅ 已修 |
| 2 | v0.82.0 #6946 | DNS 失败（`getaddrinfo`/`ENOTFOUND`/`EAI_AGAIN`）触发自动重试 | `_is_retryable_error_message`/`is_retryable_assistant_error` 缺这些模式 | A | ✅ 已修 |
| 3 | v0.82.0 #6618 | compaction/branch-summary 请求用 fresh routing session ID + `cacheRetention: "none"`（避免不可复用的 cache 写入） | `complete_summarization` 未设 cacheRetention/sessionId | A | ✅ 已修 |
| 4 | v0.82.0 #6341 | `Tool.constrainedSampling` 严格 JSON Schema（prefer/require）+ Lark/regex grammar | 已实现（`constrained_sampling` + `GrammarVariants` + `JsonSchemaStrict`，v0.80 已移植） | D | 已实现（见 DEVIATIONS #5） |
| 5 | v0.82.0 #6928 | 生成目录只暴露 provider 验证的 reasoning levels | 生成流程已确认保留 | D | 已确认保留（见 DEVIATIONS #1） |
| 6 | v0.82.0 #5dc40fee3 | 生成模型类型从 JSON 派生 | 生成流程已确认保留 | D | 已确认保留（见 DEVIATIONS #1） |
| 7 | v0.82.0 #7016 | `getBuiltinModelDataGeneratedAt`（目录 freshness 用生成时间而非文件 mtime） | 生成流程已确认保留 | D | 已确认保留（见 DEVIATIONS #1） |
| 8 | v0.82.1 #4cf0a729c | `ModelsError` 消息追加底层原因 | pi-rs 无 SDK 层（直接 reqwest，错误文本含 body） | D | 不适用（架构差异） |
| 9 | v0.82.1 #7081 | Claude Opus 5（Anthropic/Bedrock） | 模型覆盖度受限 | D | 已确认保留（见 DEVIATIONS #2） |
| 10 | v0.82.0 #6927/#6935 | OpenRouter / Kimi Code OAuth 登录 | 无 OAuth 登录流程 | C | 范围外（OAuth 登录） |
| 11 | v0.82.0 #6941 | OpenRouter Anthropic cache breakpoints + `~anthropic/*-latest` 别名 cache control | 模型元数据 | D | 已确认保留（见 DEVIATIONS #2） |
| 12 | v0.82.1 #a9f5b1c12 | Radius OAuth 走 gateway | pi-messages 相关，无 OAuth | C | 范围外（OAuth） |
| 13 | v0.82.0 #6955 | Codex WebSocket `previous_response_not_found` 重试 | 无 codex | D | 已确认保留（见 DEVIATIONS #2） |

## pi-agent-core

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 14 | v0.82.0 #e32c1491b | AgentHarness breaking change：`ExecutionEnv` 依赖 + 无上下文 `AgentTool` 输入 → 应用定义 `toolContext` + context-aware `AgentHarnessTool`（read/write/edit/bash 工具 + async bash prepare） | harness 仍用旧 `ExecutionEnvInfo` + 无上下文工具 | B | ✅ 已确认保留（架构差异，见 DEVIATIONS #7） |
| 15 | v0.82.0 #6618 | compaction/branch-summary 用 fresh routing session ID + 禁用 prompt caching | 同 #3（harness 侧） | A | ✅ 已修（随 #3） |

## pi-coding-agent

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 16 | v0.82.0 #6971 | RPC bash 命令流式 `bash_execution_update` 事件（带 request id 关联） | RPC bash 无流式事件 | B | ✅ 已修 |
| 17 | v0.82.0 #6967 | bash 工具运行时注入 `PI_SESSION_ID`/`PI_SESSION_FILE`/`PI_PROVIDER`/`PI_MODEL`/`PI_REASONING_LEVEL` | bash 工具无 session 环境变量 | B | ✅ 已修 |
| 18 | v0.82.1 #7032 | 不可用 scoped models 从 `/models` 隐藏 → 暴露（可移除） | 未核对 | B | ✅ 已修（diagnostics 加 code 字段） |
| 19 | v0.82.0 #6999 | `/model` 打开选择器时重载 models.json | 未核对 | B | ✅ 已修（`ModelRegistry::refresh`） |
| 20 | v0.82.1 #7106 | 资源加载器跳过目录（AGENTS.md 是目录时 EISDIR 警告） | `resource_loader.rs` 用 `read_to_string` 静默跳过目录（无警告，行为等价） | D | 已实现（行为等价） |
| 21 | v0.82.0 #6210 | scoped model ids 含括号时先按字面量精确匹配再 glob | 未核对 | B | ✅ 已修 |
| 22 | v0.82.0 #7045 | `outputPad` 暴露给 custom renderers | TUI 渲染层 | C | 范围外（TUI） |
| 23 | v0.82.0 #7072/#7034 | llama.cpp 模型目录缓存 + context window output limit | llama.cpp 不做 | C | 范围外（见 DEVIATIONS #15） |
| 24 | v0.82.0 #6977 | 显式自更新绕过 `PI_SKIP_VERSION_CHECK` | 无 pi update | C | 范围外 |
| 25 | v0.82.0 #7005/#7009/#6903/#6958/#7015 | protobufjs / clipboard / 外部编辑器 / debug logs / scroll indicators | TUI/打包 | C | 范围外 |

## 范围外（C）

- TUI：outputPad、scroll indicators、clipboard、外部编辑器、debug logs
- OAuth 登录：OpenRouter / Kimi Code / Radius
- llama.cpp：模型目录缓存、context window output limit
- Codex WebSocket、protobufjs、自更新

## 处理规则

- A 类：直接修，修完跑 `cargo test` + `cargo clippy --all-targets -- -D warnings`，并把真回归补进 `PORTING_MISTAKES.md`。
- B 类：需要用户确认或排期的大项，不在本次会话内擅自实现。
- 待查：逐项核对 pi-rs 现状后归类。

---

# 对齐差距清单（v0.80 → v0.81）

> 对齐目标：earendil-works/pi **v0.81**（见 `MILESTONE.md`）。
> 本清单从 v0.80.10 → v0.81.1 的 releases changelog（https://github.com/earendil-works/pi/releases）
> 中筛出与 pi-ai / pi-agent-core / pi-coding-agent / pi-cli 相关的变更，逐项核对 pi-rs 现状。
>
> 分类：**A** = 回归 bug（翻译遗漏/实现错误，直接修）；**B** = 未登记偏差（需确认或暂缓）；
> **C** = 范围外（TUI/打包/文档）；**D** = 已登记偏差（见各 crate `DEVIATIONS.md`）。

## pi-ai

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 1 | v0.81.0 #6854 | openai-completions 跨 provider 回放时 `normalizeToolCallId` 处理 pipe 分隔 ID（`{call_id}|{item_id}`，来自 Responses API），保持 tool call id 唯一 | `openai.rs` 消息转换直接透传 `tool_call_id`，无 pipe 处理 | A | ✅ 已修 |
| 2 | v0.81.0 #6727 | OpenAI Responses 早期流结束（"stream ended before a terminal response event"）分类为可重试 provider 错误 | `_is_retryable_error_message` 缺该模式 | A | ✅ 已修 |
| 3 | v0.81.0 #6840 | 新增 `contentText` 工具（从 message content 提取 joined text） | 无对应工具函数 | B | ✅ 已修（`utils/text.rs`） |
| 4 | v0.81.0 #6834 | 新增共享 `uuidv7` 工具（时间有序 ID，Codex WebSocket request id 用） | 无 uuidv7；harness entry id 用 `Uuid::new_v4` | B | ✅ 已修（`utils/uuid.rs`） |
| 5 | v0.81.0 #6901 | 新增 `retryAssistantCall()`（bounded retries + 生命周期回调 + abort 处理） | 无对应 API | B | ✅ 已修（`utils/retry.rs`） |
| 6 | v0.81.0 #6858 | 新增 Qwen Token Plan / Qwen Token Plan China 内置 provider | 无（13 provider 覆盖即最终范围） | D | 已确认保留（见 DEVIATIONS #2） |
| 7 | v0.81.0 #6668 | GitHub Copilot long-context pricing tiers 修复 | 模型元数据未生成 | D | 已确认保留（见 DEVIATIONS #2） |
| 8 | v0.81.0 #8881e1762 | Kimi Coding subscription 隐含成本 | 无 kimi-coding 模型 | D | 已确认保留（见 DEVIATIONS #2） |
| 9 | v0.81.0 #75cb0b873 | OpenCode Go 支持 OpenAI Responses API | 无 opencode-go 模型 | D | 已确认保留（见 DEVIATIONS #2） |
| 10 | v0.81.0 #6853 | GPT-5.6 Codex 默认 272K context | 无 codex 模型（openai 官方 gpt-5.6 为 1050K，正确） | D | 已确认保留（见 DEVIATIONS #2） |
| 11 | v0.81.0 #6765/#6742 | 模型生成分离（TS 形状与 JSON 值分离）+ 编译前验证 | 单 JSON 文件 + xtask 生成，已确认保留 | D | 已确认保留（见 DEVIATIONS #1） |
| 12 | v0.81.0 #a82289637/#bbb91fa8a | 新 Gemini / Qwen 生成模型 | 模型覆盖度受限 | D | 已确认保留（见 DEVIATIONS #2） |
| 13 | v0.81.1 #959cc1897 | Kimi K3 用 OpenAI thinking format + reasoning effort | 无 kimi 模型 | D | 已确认保留（见 DEVIATIONS #2） |

## pi-agent-core

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 14 | v0.81.0 #6671 | `ToolResultMessage` 加 `usage?: Usage`（工具可报告 LLM usage） | `AgentMessage::ToolResult` 缺 `usage` 字段 | A | ✅ 已修 |
| 15 | v0.81.0 #6594 | `SessionStorage` breaking change：`getPathToRootOrCompaction`（retainedTail 自包含 checkpoint）、`getSessionName`/`getSessionStats`、cursor-based `getEntries` | harness `SessionStorage` 仍为旧接口（`get_path_to_root`、无 name/stats/cursor、无 retainedTail） | B | ✅ 已修 |
| 16 | v0.81.0 #6851/#6915 | `Agent.streamFn` 必选 + `getDefaultStreamFn()` fallback（未配置时 throw） | `stream_fn: Option<StreamFn>` fallback 到返回 Err 的函数（运行时才报错） | B | ✅ 已修（`set_default_stream_fn`/`get_default_stream_fn`，未配置时 panic） |
| 17 | v0.81.0 #6834 | harness entry id 用 uuidv7 | 用 `Uuid::new_v4` | B | ✅ 已修（uuidv7 尾部 8 字符） |

## pi-coding-agent

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 18 | v0.81.0 #6695 | prompt-template 全参数默认值 `${@:-default}` / `${ARGUMENTS:-default}` / `${N:-default}` | `substitute_args` 不支持带默认值语法 | A | ✅ 已修 |
| 19 | v0.81.0 #6865 | 新增 `get_available_thinking_levels` RPC 命令 + `RpcClient.getAvailableThinkingLevels()` | RPC 命令列表无此命令 | B | ✅ 已修 |
| 20 | v0.81.1 #6901/#6647 | compaction/branch-summary 按 retry policy 重试 + `summarization_retry_scheduled/attempt_start/finished` 事件 | compaction 无 retry，无相关事件 | B | ✅ 已修（`complete_summarization` + 3 事件） |
| 21 | v0.81.0 #f1a466b19/#3da591ab7 | llama.cpp router 集成 + Hugging Face 模型搜索/下载 | 无 | B | ✅ 已确认不做（用户拍板，见 DEVIATIONS #15） |
| 22 | v0.81.0 #019e4ad68 | 扩展可注册完整 pi-ai provider（认证/模型刷新/过滤/流式） | 扩展系统无 provider 注册能力 | B | ✅ 已修（`RuntimeHandle.register_provider` → `ModelRegistry.register_provider`） |
| 23 | v0.81.0 #f1c587dde | 避免重复 session 读取（启动延迟优化） | 未核对 | B | ✅ 已修（`open()` 读一次复用 entries） |
| 24 | v0.81.0 #c889eb880/#b14250412 | 模型目录网络刷新移出启动初始化 | pi-rs 手动 `pi refresh`，已确认保留 | D | 已确认保留（见 DEVIATIONS #10） |
| 25 | v0.81.0 #54fad505b | 持久化远程目录不覆盖更新的 bundled 目录（Last-Modified 比较） | `remote_catalog.rs` 无 bundled 比较 | D | 已确认保留（见 DEVIATIONS #10） |
| 26 | v0.81.0 #35a0d5d62 | 压缩期间消息队列保持 steering/follow-up 投递 | interactive-mode（TUI 层）改动 | C | ✅ 已修（2026-09-05 TUI 层消息排队对齐：compaction 队列 + flushCompactionQueue + Esc 中止压缩 + pending 显示，见 DEVIATIONS TUI 行第十轮） |
| 27 | v0.81.0 #a2c5ee33e | read 工具错误不做语法高亮 | TUI 渲染层改动 | C | 范围外（TUI） |
| 28 | v0.81.0 #31dc078bf | brace-expansion 5.0.7 | 纯依赖更新 | C | 范围外 |

## pi-cli

| # | 来源 | TS 行为 | pi-rs 现状 | 分类 | 状态 |
| --- | --- | --- | --- | --- | --- |
| 29 | v0.81.0 #9e7fce70a | 包根导出 message/tool execution 事件类型 | 未核对 | B | ✅ 已修（`pub use AgentEvent`） |

## 范围外（C）

- TUI：cursor 修复（#6790）、ANSI wrapping CRLF/CR（#6764）、paste registry（#6844）、llama 下载进度 UI
- 打包/更新：source archives（#6913）、npm 包管理、`pi update`
- server/orchestrator 重命名（#6898）

## 处理规则

- A 类：直接修，修完跑 `cargo test` + `cargo clippy --all-targets -- -D warnings`，并把真回归补进 `PORTING_MISTAKES.md`。
- B 类：需要用户确认或排期的大项，不在本次会话内擅自实现。
- 待查：逐项核对 pi-rs 现状后归类。

---

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
| 10 | v0.80.2 | `ApiKeyCredential` discriminator `type: "api_key"` + auth.json `env` 覆盖 | `AuthCredential::ApiKey` 增加可选 `env` 映射（`key?: string` + `env?: ProviderEnv`，序列化/反序列化保留）；`get_api_key` 解析时 credential `env` 覆盖进程环境（对齐 TS `overlayEnvAuthContext`，`resolve_config_value_with_env`） | A | ✅ 已修 |
| 11 | v0.80.2 #6020 | openai-completions 运行时 `detectCompat` fallback（无显式 compat 的模型按 baseUrl 推断） | `detect_compat`/`get_compat` 完整移植（zai/together/moonshot/openrouter/cloudflare/nvidia/ant-ling 等探测 + thinkingFormat/maxTokensField/sessionAffinity 推断），并接入请求构建：system/developer 角色、store、max_tokens vs max_completion_tokens、stream_options 条件、reasoning 参数、prompt cache、session affinity headers、kimi deferred tools、zai_tool_stream、empty tools for tool history、anthropic cache control | A | ✅ 已修 |
| 12 | v0.79.9 #5673 | `chat_template_kwargs` thinking（vLLM/HF chat-template 模型） | `chat_template_kwargs`/`chat_template_args` compat 字段 + `build_chat_template_values`/`resolve_chat_template_kwarg_value`（`$var: thinking.enabled`、`omitWhenOff`、thinkingLevelMap 映射），chat-template/qwen-chat-template/baseten thinking 分支（含 thinking_token_budget） | A | ✅ 已修 |
| 13 | v0.79.10 #5114 | OpenAI-compatible 流式保留 reasoning_details（先于 tool call delta 到达） | 已核对：openai.rs 流式按 `reasoning_content`/`reasoning`/`reasoning_text` 处理 reasoning 块，但原实现遍历**全部**非空字段重复追加（如 chutes.ai 同时返回 reasoning_content 与 reasoning 相同内容会重复）；已改为仅取**第一个**非空字段（对齐 TS openai-completions），并补 opencode-go `reasoning`→`reasoning_content` signature 映射 | A | ✅ 已修 |
| 14 | v0.80.7 #6496 | `compat.sessionAffinityFormat`（替代 `sendSessionIdHeader`，breaking） | `OpenAIResponsesCompat` 已更新为 v0.80.7 字段（sessionAffinityFormat/supportsDeveloperRole/supportsStrictMode 等），provider 已按格式发 session affinity headers | B | ✅ 已修 |
| 15 | v0.80.0+ | `openai-responses` API 后端（openai 官方 provider 用 `/v1/responses`） | 已实现 `openai_responses.rs`（消息/工具转换 + SSE 流式 + 事件序列），openai 模型已切到 `openai-responses`；简化项见 DEVIATIONS.md #5 | B | ✅ 已修（含简化偏差） |
| 16 | v0.80.8 | `ModelRuntime` / live model catalog refresh | 已实现**有界子集**：`pi refresh [--force|--offline|--catalog-url]` 手动刷新命令 + `remote_catalog.rs`（拉取 `pi.dev/api/models/providers/{provider}`、解析 array/`{models:[...]}`/keyed-object 三种格式、`ModelRegistry::upsert_models` 合并、models-store.json 缓存、fresh 窗口/304/404/失败保留缓存、`--offline` 离线恢复）。未实现：启动/后台自动刷新、provider 可用性检查、credential 同步 | B | ✅ 部分完成（手动刷新可用；自动刷新部分经用户确认暂不做，见 pi-coding-agent DEVIATIONS #10） |
| 17 | v0.80.3/8/9/10 | 模型元数据：Claude Sonnet 5、Grok 4.5、Kimi K3、默认 gpt-5.5、openrouter/fusion、context window 272k 等 | 模型覆盖度受限 | D | 已确认保留（用户拍板：主流仅需 anthropic + openai 格式，不扩展其他 provider，见 DEVIATIONS #2） |
| 18 | v0.79.5 #5790 | 全局 `httpProxy` 设置 | `apply_http_proxy_settings`：全局设置 httpProxy → HTTP_PROXY/HTTPS_PROXY 环境变量（`??=` 语义，仅未设置时写入；reqwest 自动读取），在 `create_agent_session_services` 应用 | A | ✅ 已修 |
| 19 | v0.79.5 #5798 | Vercel AI Gateway attribution headers | 无 Vercel provider | D | 已确认保留（同 #17，用户拍板不扩展 provider，见 DEVIATIONS #2） |

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
