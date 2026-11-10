# pi-rs 全量行为审计（TypeScript → Rust）

> 审计对象：`pi-rs` first-party crate（`crates/pi-*`，不含 `crates/vendor`）。
> 参考基准：`../pi`（v0.87.1，`.version`: `ts_version=v0.87.1` / `ts_commit=f07218c4d`）。
> `~/rust_migrations/pi-ts` 是 v1.5.0，仅用于识别未来差距。
> 方法：7 个范围并行静态对照 + 逐条人工复核。每条标注 **已复核**（lead 亲自对照两侧代码）
> 或 **待复核**（agent 报告，未独立确认）。本文件只记录偏差，不修改代码。
>
> 已确认的有意偏差（`DEVIATIONS.md` 中 `已确认保留`）不列入。

## 0.0 版本口径（重要，影响所有结论）

本仓库 `.version` 的对齐目标是 **v0.87.1**（`../pi`，commit f07218c4d）。
但本轮 7 个范围中有 6 个对照的是 `~/rust_migrations/pi-ts`，它是 **v1.5.0**
（commit 45344bd，2026-11-09），不是对齐目标。因此报告里的"与上游不一致"
必须再分两类：

- **目标偏差（v0.87.1 也成立）**：当前移植确实没对齐。
- **未来差距（仅 v1.5.0 成立）**：上游后来改的，v0.87.1 与 Rust 一致，
  不属于当前验收范围。

已按 v0.87.1 逐条复核 high 项，结果见 §1c。中/低项若未标注，默认
"需按 v0.87.1 复核"。

**truncate 差分已用 v0.87.1 重跑**：`PI_TS_TOOLS_DIR=../pi/...`，
结果仍是 **287/500 一致、213 分歧**（`line.text` 213、`sizes` 2、
`line.truncated` 2），与 v1.5.0 结果一致。这两个缺陷属于目标偏差。

## 0.2 本轮修复状态（2026-11-10）

按 4 点顺序执行后的状态：

| 已修复（目标偏差） | 说明 |
| --- | --- |
| H3 / H6 / H30 / H20 | pi-messages、compaction、Tab 补全的字节切片 panic；`mcp login` runtime 嵌套 |
| H4 | 工具参数 schema 校验（type/enum/const/anyOf/oneOf/allOf/required/items/coercion） |
| H5 / H8 / H11 / keep_recent | compaction 估算、valid cut points、entry index 映射、默认 20000 |
| H7 | shell full-output 全量缓冲、50KB 阈值、`on_chunk`、ANSI 剥离 |
| H9 | branch summary 失败传播、options 透传、file_ops 填充 |
| abort / teardown | `request_abort` + abort flag + compaction/branch abort + teardown abort |
| H16 / H17 / H18 | CLI `--`、短选项、长选项、`--models` 拆分、`--thinking max` |
| H19 | 图片使用 `process_image` 的 mime/hints，CLI 默认 `autoResizeImages:false` |
| H23 / H24 | Responses reasoning effort + `thinkingLevelMap`；streamSimple clamp/off |
| print-mode / skills prompt / `/model` | session header + 最终消息；XML skills + hidden 过滤；真实 reasoning/context |
| slash commands | `/thinking`、`/export` 真实实现；其余识别后提示，不再发给 LLM |
| 原始 truncate 两缺陷 | `truncate_line` 改 UTF-16 计数；`format_size` 改 JS `toFixed(1)` 舍入。差分：**v1.5.0 500/500**；v0.87.1 **478/500**，22 条全部是 v0.87.1 `slice` 切断代理对产生的 lone surrogate（Rust `String` 无法表示，见 DEVIATIONS #38） |

**第二轮追加修复：** H10（idle 晚于 listeners）、H22（fuzzy 排序缩放）、H28（error/pending → `Error` 事件）、`pi-coding-agent/core/compaction.rs` 摘要格式/file lists、settings `outputPad`（数字/字符串）、auth 键名 + `expires: u64`、OAuth migration（扁平 + 启动时运行）、`length` 恢复、JSONL 坏行跳过、continue 失败事件、`prepareNextTurn` 的 `"off"`。

**第三轮追加修复：** prompt template 文件名/描述回退、skills ignore 的 `!pattern` 反选（last-match-wins）、listener abort signal、`afterToolCall` 的 `usage` 覆盖。

**第四轮追加修复：** H29（三 provider 流中断保留 partial content/usage/responseId）。

**第五轮追加修复（仅 v0.87.1 目标偏差）：** ls 错误语义/行上限/stat 跳过、edit 单对象/权限检查、file mutation queue 绝对路径、output_accumulator 单 U+FFFD、edit_diff 空模式/删除行号、pi-tui Release 过滤/Up-Down/Ctrl+J/补全光标位置。

**第六轮追加修复：** skills realpath 去重 + 重名诊断、pi-tui PageUp/Down 视口行数、SelectList 两端循环（移除 v0.87.1 未绑定的 Home/End/j/k）。

**第七轮追加修复：** `pi-ai` overflow 模式（`Prompt too long`、DS4、DashScope、Cerebras provider 门控）、retry 分类器（v0.87.1 正则集合）、OpenAI completions 的 system-message collapse 条件化。

**第八轮追加修复：** `pi-messages` 请求体改为 `TranscriptContext`（`context: { messages }`）；OpenAI completions 的 `thinkingTokenBudgetField`/`vllmPriority`。

**第九轮追加修复：** Responses reasoning `summary || content`、custom-tool 收尾 delta、`StopReason::Deferred`。

**第十轮追加修复：** `visual_lines` 纯空白文本返回 `[]`。

**第十一轮追加修复：** `AgentToolResult.structuredContent` + `afterToolCall` 设置/清除语义（64 处构造点补 `None`）。

**第十二轮追加修复：** pi-tui `visible_width` 按 grapheme cluster 计算，ZWJ 家庭 emoji 整体算 2 列。

**剩余（均为 v1.5.0 未来差距，不在本轮范围）：** `durationMs`/`nestedCalls`/`totalFromProvider`/`samplingParamsByThinkingLevel`、`argumentsError`、跨模型 thinking 丢弃、15% clamp、`estimate` 3.5、RPC `disposition`、MCP 整个 crate 等。

## 0. 覆盖范围与状态

| 范围 | 状态 |
| --- | --- |
| `pi-coding-agent/core/tools` | 已完成 |
| `pi-agent-core`（agent loop / compaction / harness） | 已完成 |
| `pi-ai`（providers / estimate / simple-options） | 已完成 |
| `pi-mcp` / `pi-extensions` / `pi-cli` / `pi-extension-api` | 已完成 |
| `pi-tui` | 已完成 |
| `pi-coding-agent` 其余 core/modes/utils | 待回 |
| 跨 crate Unicode/数值/启发式专项 | 已完成 |

---

## 1. 高严重度（已复核）

| # | 位置 | 上游 TS 行为 | Rust 实际行为 | 影响 |
| --- | --- | --- | --- | --- |
| H1 | `pi-ai/src/utils/estimate.rs:10` | `CHARS_PER_TOKEN = 3.5` | `= 4` | 所有上下文 token 估算低 ~12.5%，驱动 overflow/compaction 判定 |
| H2 | `pi-ai/src/providers/simple_options.rs:20` | `margin = max(4096, ceil(input*0.15))`；`room < 32768` 时回退 `min(32768, window-input-4096)` | 只减固定 `4096` | 大输入下请求的 `max_tokens` 可能超出窗口；OpenRouter 直接拒绝 |
| H3 | `pi-ai/src/providers/pi_messages.rs:599` | UTF-16 `.length` + surrogate-safe `ownedSlice` | `value.len()` 字节 + `&value[..8192]` | 多字节边界 **panic**；CJK 截断点错误 |
| H4 | `pi-agent-core/src/agent_loop.rs:147` | `validateToolArguments` 按 JSON Schema `parse()`，失败抛错并转 error tool result | `validate_tool_arguments` 只过滤未知字段、归一化 optional null | 缺 required / 类型 / enum / `argumentsError` 校验，错误参数直达工具 |
| H5 | `pi-agent-core/src/harness/compaction/compaction.rs:11` | UTF-16 `.length`、`Math.ceil(chars/4)`、image 4800、thinking/sections/toolsAdded 计入 | UTF-8 字节、整数除法、`.max(1)`、忽略 image/thinking/sections，块间额外加空格 | 压缩阈值/上下文百分比系统性偏移 |
| H6 | `pi-agent-core/src/harness/compaction/compaction.rs:316` | `sliceHead(text, 2000)` + `[... N more characters truncated]` | `&text[..2000]` 字节切片 + `[...truncated]` | 多字节边界 **panic**；摘要 prompt 内容不同 |
| H7 | `pi-agent-core/src/harness/utils/shell_output.rs:45` | 超过 50KB 起把每个 chunk 写入临时文件；`onChunk` 回调；`stripAnsi` + 跨 chunk 未完成转义处理 | 阈值 100KB；临时文件写的是被裁剪的滚动尾部；`on_chunk` 从未调用；未剥 ANSI | “full output” 文件不是全量；彩色转义进入模型与截断计数 |
| H8 | `pi-agent-core/src/harness/compaction/compaction.rs:186` | 全程使用 entry index；`findValidCutPoints` 排除 toolResult；turn start 有 5 种 role | 用过滤后 `messages` 的 index 查 `entries`；可在任意消息（含 toolResult）切；只认 `user` | `first_kept_entry_id` 指向错误条目；可能把 tool call 与 result 拆开 |
| H9 | `pi-agent-core/src/harness/compaction/branch_summarization.rs:225` | aborted → `{aborted:true}`；failure → `{error}`；调用方 apiKey/headers/env/signal/thinking 生效 | `Err(_) => "Branch summary unavailable"` 且返回 `Ok`；api_key 只从环境取；无 signal/headers/env/thinking | 取消无效；失败被持久化为正常摘要；调用方凭证被忽略 |
| H10 | `pi-agent-core/src/agent.rs:1153` | `agent_end` 监听器全部 settle 后才 `finishRun()` / idle | `AgentEnd` 时先 `is_streaming=false` + `notify_waiters()`，再 await listeners | `wait_for_idle()` 可能在监听器完成前返回 |
| H11 | `pi-agent-core/src/harness/compaction/compaction.rs:105` | `findCutPoint` 按 `>= keepRecentTokens`、valid cut points、`isTurnStartEntry` | `>` 预算、任意消息可切、只认 `user` | 与 H8 同源，切点语义整体简化 |
| H12 | `pi-mcp/src/oauth/flow.rs:827` | 并发 401 共享同一个 promise；leader 失败会 reject 所有 follower | follower `gate.notified().await` 后直接 `return Ok(())` | 授权失败被吞，后续用旧 token 重试才报 401 |
| H13 | `pi-mcp/src/oauth/flow.rs:816` | 已 settle 的 promise，无丢失唤醒窗口 | `Notify::notify_waiters()` 不存 permit；follower 在 `notified()` 注册前错过唤醒 | follower 可能永久挂起 |
| H14 | `pi-extensions/src/subagent.rs:1147` | `outcome.signal` → `status="failed"`，并记录 `terminationSignal` | `status.code()` 为 `None` 时 `unwrap_or(0)`；无 `terminationSignal` | 被信号杀死的子进程被报告为 `completed` |
| H15 | `pi-extensions/src/subagent.rs:759` | `details.isError = forceError \|\| results.some(isFailedResult)`，`tool_result` hook 转 `isError:true` | 并行结果硬编码 `is_error=false`，`details` 无 `isError` | 部分任务失败仍显示成功 |
| H16 | `pi-cli/src/args.rs:418` | `--` 后所有 token 作为 message / `@file`，然后 break | catch-all 匹配 `--`，`name=""`，吞掉下一个非 `-` token | `pi -p -- "- Summarize"` 丢失消息 |
| H17 | `pi-cli/src/args.rs:186-430` | `-c/-r/-n/-e/-nt/-nbt/-ne/-ns/-np/-nc/-a/-na` 有明确分支；未知短选项报错 | 无这些分支，全部落入 catch-all 成为 extension flag | 常用短选项静默失效；未知短选项不报错 |
| H18 | `pi-cli/src/args.rs:205-316` | `--append-system-prompt`、`--skill(s)`、`--prompt-template(s)`、`--theme(s)`、`--use-theme`、`--tui-mode`、`--no-mcp`、`--approve/-a`、`--no-approve/-na` | 只有 `--append-prompt/-A`、`--skills`、`--prompt-templates`、`--themes`、`--trust/--no-trust` | TS 拼写变成无效 extension flag；`--use-theme`/`--tui-mode`/`--no-mcp`/`--approve` 无实现 |
| H19 | `pi-cli/src/file_processor.rs:92` | `autoResizeImages:false`（CLI 路径）；附件用 `processed.mimeType`；写入 `processed.hints` | `auto_resize_images` 字段从未读取；始终 resize 并编码 PNG，但附件保留**原始 mime**；hints 丢弃 | 图片被改尺寸却标成原格式（PNG 数据 + `image/jpeg` 标签）；提示丢失 |
| H20 | `pi-cli/src/mcp_cli.rs:364` | `await login(...)` | 在 `#[tokio::main]` 的 async 调用链里 `Builder::new_multi_thread().build()` + `block_on` | `mcp login` **panic**（Cannot start a runtime from within a runtime） |
| H21 | `pi-coding-agent/src/core/tools/edit.rs:367` | 未改动行逐字节保留；替换处按该行原有 ending（含 lone CR） | `detect_line_ending` 选一个主 ending，`restore_line_endings` 重写所有 `\n` | 混合换行文件被统一；CR-only 文件被改成 LF |
| H22 | `pi-tui/src/fuzzy.rs:58` | `score += i * 0.1`（浮点） | `score += i as i64 / 10`（整数截断） | 模糊排序结果改变 |
| H30 | `pi-tui/src/app.rs:2273` | `findLastDelimiter` 返回 UTF-16 索引，`slice(index)` 合法 | `find_last_delimiter` 返回分隔符首字节，caller `+1` 后 `&text[start..]` | Tab 补全遇到多字节分隔符（`，`、`…`、`—`、全角字母）**panic** |

### 1b. pi-ai 高严重度（已复核）

| # | 位置 | 上游 TS 行为 | Rust 实际行为 | 影响 |
| --- | --- | --- | --- | --- |
| H23 | `pi-ai/src/providers/openai_responses.rs:1980` | `reasoningEffort ?? (reasoningSummary ? "medium" : undefined)`，经 `thinkingLevelMap` 映射，`off` 可关闭 | 固定 `{"effort":"medium","summary":"auto"}` | 请求的 reasoning 档位被忽略；无法关闭思考 |
| H24 | `pi-ai/src/providers/simple_options.rs:124` | `clampThinkingLevel` + `"off"` → `undefined` | `reasoning_effort: opts.reasoning.clone()` 原样转发 | `off` 被当成开启；`xhigh/max` 未 clamp |
| H25 | `pi-ai/src/utils/transform.rs:162` | 跨模型 thinking **丢弃**（`return []`） | 转成可见 `text(thinking)` | 新模型把旧思考当正文，污染回复 |
| H26 | `pi-ai/src/providers/openai.rs:2217`（Anthropic/Responses 同） | `finish()` 完整解析；非法 JSON → `arguments:{}` + `argumentsError` | 保留流式前缀解析结果，无 `argumentsError` | `{"a":1}garbage` 被当作 `{"a":1}` 执行 |
| H27 | `pi-ai/src/providers/anthropic.rs:669` | `splitErrorToolResultImages`：错误 tool_result 的图片移到所有结果之后 | 图片内联在 `tool_result` 里 | Anthropic 拒绝非文本 error tool_result |
| H28 | `pi-ai/src/providers/anthropic.rs:1978`（OpenAI/Responses 同） | `stopReason` 为 `error/pending` 时抛错，catch 发 `error` 事件 | 一律发 `Done { reason: Error }` | 消费者看到“成功结束 + error reason”，而非错误事件 |
| H29 | `pi-ai/src/providers/anthropic.rs:1104`（OpenAI/Responses 同） | catch 复用已累积的 `output`（内容/usage/responseId 保留） | 新建空 `AssistantMessage`，`usage=default` | 流中断后已流式内容与 usage 全部丢失 |

### 1c. high 项的 v0.87.1 判定

| H# | 主题 | v0.87.1 判定 |
| --- | --- | --- |
| H1 | pi-ai `CHARS_PER_TOKEN` 4 vs 3.5 | **未来差距**：v0.87.1 就是 4，Rust 正确 |
| H2 | pi-ai `clampMaxTokensToContext` 15% margin | **未来差距**：v0.87.1 只减固定 4096 |
| H3 | pi-messages 字节切片 | **目标偏差**：`&value[..8192]` panic 是 Rust 独有 |
| H4 | 工具参数无 schema 校验 | **目标偏差**：v0.87.1 调用 `validateToolArguments` |
| H5 | compaction token 估算 | **目标偏差**：v0.87.1 用 UTF-16/ceil/4800 |
| H6 | `&text[..2000]` | **目标偏差**：v0.87.1 用 UTF-16 + `[... N more]` |
| H7 | shell full-output 临时文件 | **目标偏差**：v0.87.1 bash-executor 写全量 |
| H8 | prepare_compaction index | **目标偏差**：v0.87.1 全程 entry index |
| H9 | branch summary 吞错 | **目标偏差**：v0.87.1 返回 aborted/error |
| H10 | idle 早于 listeners | **目标偏差**：v0.87.1 注释明确 listener 后 idle |
| H11 | find_cut_point 简化 | **目标偏差**：v0.87.1 有 valid cut points/5 种 turn start |
| H12/H13 | OAuth 并发 follower | **未来差距**：v0.87.1 没有 MCP 包 |
| H14 | subagent 信号杀死 | **未来差距**：v0.87.1 也是 `code ?? 0` |
| H15 | subagent 并行 isError | **未来差距**：v0.87.1 `makeDetails` 无 `isError` |
| H16 | CLI `--` 终止符 | **目标偏差**：v0.87.1 有显式分支 |
| H17 | CLI 短选项 | **目标偏差**：v0.87.1 有 `-c/-r/-nt/...` |
| H18 | CLI 长选项改名 | **目标偏差**：v0.87.1 用 `--append-system-prompt` 等 |
| H19 | 图片 mime/auto-resize | **目标偏差**：v0.87.1 传 `autoResizeImages:false` + `processed.mimeType` |
| H20 | `mcp login` panic | **未来功能**（v0.87.1 无 MCP），但 panic 仍是 Rust 缺陷 |
| H21 | edit 换行重写 | **未来差距**：v0.87.1 也用 `restoreLineEndings`，Rust 正确 |
| H22 | fuzzy 整数惩罚 | **目标偏差**：v0.87.1 是 `i * 0.1` |
| H23 | Responses reasoning 写死 | **目标偏差**：v0.87.1 读 `reasoningEffort`/`thinkingLevelMap` |
| H24 | streamSimple 不 clamp | **目标偏差**：v0.87.1 `off` → undefined |
| H25 | 跨模型 thinking 转文本 | **未来差距**：v0.87.1 也转文本，Rust 正确 |
| H26 | 缺 `argumentsError` | **未来差距**：v0.87.1 用 `parseStreamingJson` 宽容解析 |
| H27 | Anthropic error tool-result 图片 | **未来差距**：v0.87.1 无 `splitErrorToolResultImages` |
| H28 | error stopReason 发 Done | **目标偏差**：v0.87.1 抛错并走 catch |
| H29 | 流错误丢 partial/usage | **目标偏差**：v0.87.1 catch 复用 output |
| H30 | Tab 补全 panic | **目标偏差**：v0.87.1 返回 UTF-16 索引 |
| — | compaction summary 格式 | **目标偏差**：v0.87.1 有 `[User]/[Assistant tool calls]/[Tool result]` |
| — | file lists 未追加 | **目标偏差**：v0.87.1 计算并追加 |
| — | skills prompt 格式 | **目标偏差**：v0.87.1 XML + 过滤 `disableModelInvocation` |
| — | settings 反序列化 | **部分目标偏差**：`outputPad: 0`（数字）会失败；`"header"`/`"disabled"` 是未来差距 |
| — | auth OAuth 键名 | **目标偏差**：v0.87.1 是 `access`/`refresh` |
| — | OAuth migration | **目标偏差**：v0.87.1 扁平写入且启动时运行 |
| — | abort/teardown | **目标偏差**：v0.87.1 有 abort flag + compaction/branch abort |
| — | 首条 user message 持久化 | **未来差距**：v0.87.1 也是 `hasAssistant` |
| — | print-mode 契约 | **目标偏差**：v0.87.1 写 session header + 最终消息 |
| — | RPC `disposition` | **未来差距**：v0.87.1 无该字段 |
| — | MCP binary resources | **未来差距**：v0.87.1 无 MCP |
| — | `/model` 写死 reasoning/context | **目标偏差**：v0.87.1 调 `setModel` + footer invalidate |
| — | 未处理 slash commands | **目标偏差**：v0.87.1 有 `/settings /thinking /export /import /resume` |
| — | `length` 恢复 | **目标偏差**：v0.87.1 有 `isRecoverableLength` |

**统计：** high 项约 **30 条属于 v0.87.1 目标偏差**，约 **14 条是 v1.5.0 未来差距**
（另有 1 条部分成立）。中/低项未逐条重分类。

---

## 2. 中严重度（已复核 / 待复核）

### 2.0 pi-ai

| 位置 | 问题 | 状态 |
| --- | --- | --- |
| `types.rs:888-920` | `GrammarVariants` 用 camelCase → `openaiLark/openaiRegex`（TS `openai_lark/openai_regex`）；`Tool.constrained_sampling` 无 rename（TS `constrainedSampling`） | 已复核 |
| `providers/anthropic.rs:697` | `tool_addition` 发 `tool_reference`，TS 发 `tool_definition`；同消息重定义的工具仍发 `tool_removal` | 已复核 |
| `providers/openai.rs:1968` | 首个 reasoning chunk 直接塞进 `thinking_start.partial`，无 `thinking_delta` | 已复核 |
| `utils/overflow.rs:15` | 缺 z.ai/LiteLLM/DS4/DashScope/413 模式；无 body 的 400/413 规则对所有 provider 生效（TS 仅 cerebras） | 已复核 |
| `utils/retry.rs:60` | 用 `contains` 字面量匹配，`"500"` 命中任意数字子串；缺 anchored regex 与多项可重试/不可重试模式 | 已复核 |
| `providers/openai.rs:1485` | 有工具历史就发 `"tools":[]`（TS 仅 `requiresToolsWithToolHistory`）；缺 `requiresNonNullAssistantContent`/`thinkingTokenBudgetField`/`promptCacheKeyMode`/`vllmPriority` | 已复核 |
| `providers/anthropic.rs:197` | 无 Anthropic 专用 strict 关键字回退；legacy schema 丢 `$defs/definitions`；无 `eager_input_streaming` | 已复核 |
| `providers/openai_responses.rs:1623` | reasoning = summary + content（TS `summary \|\| content`）；custom-tool 收尾 delta 丢弃；无未完成 tool call 检查 | 已复核 |
| `providers/pi_messages.rs:767` | 请求体发折叠后的 `Context`；丢 `responseModel/providerThinkingLevel`；`contentIndex` 越界可 panic | 已复核 |
| `providers/openai.rs:944` | 无条件折叠 system 消息，忽略 `supportsMidConvoSystemMessages` | 已复核 |
| `types.rs:93-152` | 缺 `thinkingLevel/deferred/durationMs/nestedCalls/cost.totalFromProvider/samplingParamsByThinkingLevel`、`StopReason::Deferred` | 待复核 |
| `provider_retry.rs:187` | abort 期间 backoff 不停止，仍多发一次请求；HTTP-date `retry-after` 被拒 | 待复核 |
| `openai_responses.rs:513` | tool-call `itemId` 丢弃条件用 `!is_same_model`，TS 用 `isDifferentModel` | 待复核 |
| `env_api_keys.rs:26` | 缺 qwen-token-plan / typesafe / meta 映射；azure provider id 不一致 | 待复核 |
| `openai.rs:575` | 任意非 JSON thinking signature 被注入为 assistant 消息键；TS 仅允许三个键 | 待复核 |

### 2.1 pi-agent-core

| 位置 | 问题 | 状态 |
| --- | --- | --- |
| `harness/types.rs:245` | `keep_recent_tokens` 默认 8192，TS 20000 | 已复核 |
| `agent_loop.rs:192` | `prepare_tool_call` 无 abort 检查（TS 两处） | 已复核 |
| `agent.rs:1019` | `continue` 路径失败不合成 lifecycle 事件 | 已复核 |
| `agent_loop.rs:1438` | `prepareNextTurn` 的 `"off"` 存成 `Some("off")`，TS 映射为 `None` | 已复核 |
| `agent_loop.rs:342` / `types.rs:419` | `AfterToolCallResult` 无 `usage`/`structured_content`，无法覆盖 | 已复核 |
| `agent.rs:1168` | listener 收到的 abort signal 恒为 `None` | 已复核 |
| `harness/session/jsonl_storage.rs:72` | 一行损坏即整体加载失败；TS 跳过 | 已复核 |
| `harness/prompt_templates.rs:80` | 无 frontmatter `name` 即跳过；TS 用文件名 basename；`parse_command_args` 非 bash 语义 | 已复核 |
| `harness/skill_loader.rs:32` | `negated` 计算后未使用；ignore 匹配非 gitignore 语义；无 realpath 去重/重名诊断 | 已复核 |
| `harness/compaction/compaction.rs:331` | 摘要 prompt/格式不同；`maxTokens=0.8*reserve`；无 length-stop/tool-call 检查；总是覆盖 `session_id` | 待复核 |
| `harness/compaction/branch_summarization.rs:68` | `file_ops` 从未填充；忽略 `context_edit`；无超预算 summary 例外 | 已复核 |
| `harness/compaction/compaction.rs:413` | 摘要请求契约差异（system prompt、previous summary 包装、maxTokens） | 待复核 |

### 2.2 pi-coding-agent/core/tools

| 位置 | 问题 | 状态 |
| --- | --- | --- |
| `grep.rs:161` | 缺 `--no-config`，环境 `RIPGREP_CONFIG_PATH` 可改变结果 | 已复核 |
| `grep.rs:173` | `--glob` 未去前导 `./` | 已复核 |
| `grep.rs:313` | 未设 `current_dir`；glob 根/符号链接路径格式不同 | 已复核 |
| `grep.rs:393` | 到 `effective_limit` 即杀并报 “limit reached”；TS 多探一个 | 已复核 |
| `grep.rs:438` | context 靠逐 match 重读文件，重叠窗口重复；非 UTF-8 降级 | 已复核 |
| `find.rs:236` | 请求 `limit` 而非 `limit+1`，`>=` 判定导致正好 N 条误报 | 已复核 |
| `ls.rs:254` | 用默认 2000 行上限，TS 用 `MAX_SAFE_INTEGER` | 已复核 |
| `ls.rs:183` | 路径不存在/非目录/读失败返回 `Ok`（isError=false），TS reject | 已复核 |
| `edit.rs:162` | 单个 edit 对象/字符串化单对象未包成 `edits[]` | 已复核 |
| `edit.rs:84` | 只查存在性，不查 `R_OK\|W_OK`；只读文件报原始 io error | 已复核 |
| `ls.rs:160` / `find.rs:436` / `grep.rs:265` / `read.rs:192` | `as_u64/as_f64` 丢失 `Math.floor`/ToInteger/负数语义 | 已复核 |
| `file_mutation_queue.rs:16` | canonicalize 失败时用原始路径作 key；TS 先 resolve 绝对路径 | 已复核 |
| `output_accumulator.rs:303` | 临时文件 `File::create`（umask、跟随符号链接、截断）；TS `0o600` + `wx` | 已复核 |
| `bash.rs:1071` / `output_accumulator.rs` | 缺 `outputSchema`/`structuredContent`/`readFullOutput` | 已复核 |
| `ls.rs:238` | stat 失败条目被当文件包含；TS `continue` | 待复核 |
| `edit_diff.rs:370` | 空 normalized `oldText` 返回 0；TS 计 `len+1` → Duplicate | 待复核 |
| `edit_diff.rs:776` | 纯删除不设 `first_changed_line` | 待复核 |
| `output_accumulator.rs:133` | `finish()` 每个 pending 字节一个 U+FFFD；TS 整个序列一个 | 待复核 |
| `render_utils.rs:52` | `normalize_display_text` 把 tab 换 4 空格而非去 CR；`replace_tabs` 4 vs 3；模块当前无调用 | 待复核 |

### 2.3 pi-mcp / pi-extensions / pi-cli / pi-extension-api

| 位置 | 问题 | 状态 |
| --- | --- | --- |
| `pi-mcp/src/protocol/content.rs:103` | 未知 `content[].type` 使整个 `tools/call` 失败；TS 降级为 `[unsupported MCP content X]` | 已复核 |
| `pi-mcp/src/transports/streamable_http.rs:703` | `application/json` 响应无大小上限；TS 有 `maxMessageBytes` | 已复核 |
| `pi-mcp/src/transports/stdio.rs:343` | 子进程退出后不 SIGTERM 进程组；无 host-exit hook | 已复核 |
| `pi-cli/src/args.rs:205` | `--models a,b` 不按逗号拆分 | 已复核 |
| `pi-cli/src/args.rs:298` | `--thinking max` 被拒；Rust 还 lowercases（TS 大小写敏感） | 已复核 |
| `pi-cli/src/list_models.rs:110` | 无匹配写 stderr + exit 1；TS stdout + exit 0 + login guidance | 已复核 |
| `pi-cli/src/list_models.rs:92` | 表格右对齐/分隔线；搜索 `contains` over id+provider+name；TS fuzzy over `provider id` + auth-filtered | 已复核 |
| `pi-extensions/src/subagent.rs:151` | `<1M` 分支 `count/1000` 截断；TS `Math.round` | 已复核 |
| `pi-extensions/src/subagent.rs:247` | 失败证据取第一个非空；TS 拼接带标签的 Error/Signal/stderr/Assistant output | 已复核 |
| `pi-extension-api/src/lib.rs:481` | `ctx.mode` 取值 `tui/cli/headless`，默认空串；TS `tui/rpc/json/print`，仅 tui 被设置 | 已复核 |
| `pi-extension-api/src/hook.rs:605` | `tool_result` payload 缺 `toolCallId/input/structuredContent/usage`，无法改 structured/usage | 已复核 |

### 2.3b pi-tui

| 位置 | 问题 | 状态 |
| --- | --- | --- |
| `fuzzy.rs:58` | 缺 TS 的 best-contiguous-occurrence 重打分；`i/10` 整数截断 | 已复核 |
| `fuzzy.rs:41` | 首字符无 `-5` consecutive bonus；边界只认空格；alnum swap 过宽 | 已复核 |
| `selection.rs:14` | 双击 word joiner 只有 `/` `-`；TS 还有 `+ \ ~ _ .` | 已复核 |
| `pi-coding-agent/src/modes/interactive.rs:3320` | 滚轮固定 3 行；TS 默认 auto 加速（1–6，Alt ×5） | 已复核 |
| `app.rs:2192` | PageUp/PageDown 固定 20 行；TS `viewportHeight - 4` | 已复核 |
| `app.rs:2168` | 非空输入时 Up/Down 变成左右移动；TS 上下移动光标 | 已复核 |
| `app.rs:2178` | 空输入 Home/End 滚动；Ctrl+Home/End 未处理；TS 语义相反 | 已复核 |
| `input.rs:61` | Ctrl+J 被 readline keymap 吞掉；TS 插入换行 | 已复核 |
| `app.rs:2371` | slash 补全只按 bare name 或 full name 单趟；TS 两趟合并 | 已复核 |
| `pi-coding-agent/src/modes/interactive.rs:2031` | Tab 强制路径走 fuzzy tree walk；TS 用 readdir | 已复核 |
| `app.rs:2308` | 补全上下文用整段输入；TS 用光标前文本 | 已复核 |
| `select_list.rs:47` | 上下不循环、Home/End/j/k 额外绑定、翻页固定 5；TS 循环 + `maxVisible` | 已复核 |
| `completion.rs:441` | 模糊文件结果只按 score 排序；TS 还按 path depth/length/name tie-break | 已复核 |
| `app.rs:3990` | `shorten_path` 用 `starts_with(home)`，`/Users/foo2` → `~2`；TS `isPathWithin` | 已复核 |
| `app.rs:4122` | 非字符串 read/edit/grep 参数渲染 `...`/`.`；TS `[invalid arg]` | 已复核 |
| `completion.rs:23` | `is_separator` 把整个 U+FF01–FF65 / U+3000–303F 当分隔符；TS 仅 CJK 标点 | 已复核 |
| `app.rs:5093` | 额外三处 `{:.1}/{:.3}` tie 舍入 | 已复核 |
| `app.rs:2019` | `KeyEventKind::Release` 被当 press 处理（公共 `app::run` 可达） | 待复核 |
| `app.rs:3295` | `visible_width` 按 char 宽度求和，ZWJ emoji 家庭 = 6；TS grapheme = 2 | 已复核 |

### 2.4 跨 crate Unicode / 数值

| 位置 | 问题 | 状态 |
| --- | --- | --- |
| `pi-agent-core/src/harness/skill_loader.rs:572` | name/description 上限按 UTF-8 字节；TS UTF-16 | 已复核 |
| `pi-coding-agent/src/core/mcp_servers.rs:707` | section 大小/截断按 scalar；TS UTF-16 | 已复核 |
| `pi-coding-agent/src/core/mcp_tools.rs:125` | 中间截断 marker 为 `... (truncated) ...`；TS `…N chars truncated…` | 已复核 |
| `pi-mcp/src/transports/streamable_http.rs:317` | 500 单位错误片段按 scalar；TS UTF-16 | 已复核 |
| `pi-coding-agent/src/core/prompt_templates.rs:215` | 60 字符描述按 scalar；TS UTF-16 | 已复核 |
| `pi-extensions/src/subagent.rs:1382` | 工具预览按 scalar；TS UTF-16 | 已复核 |
| `pi-tui/src/components/input.rs:110` | 大粘贴阈值/标记按 scalar；TS UTF-16 | 已复核 |
| `pi-tui/src/components/completer.rs:410` | 补全拼接按 `chars()`；TS 按 UTF-16 索引 | 已复核 |
| `pi-tui/src/app.rs:5412` | 面板标签 padding 按 scalar；TS `visibleWidth`（CJK 宽度 2） | 待复核 |
| `cache_warmer.rs:799` / `subagent.rs:139` / `app.rs:5102,5117` | 原始 `{:.3}/{:.4}/{:.1}` 的 tie 舍入与 `toFixed` 不同 | 已复核 |

---

## 3. 重复 helper 清单

| helper | 副本 | 结论 |
| --- | --- | --- |
| `format_size` | `pi-coding-agent/core/tools/truncate.rs:63` | 不一致（`{:.1}` tie） |
| `format_size` | `pi-tui/app.rs:543` | 不一致（同上） |
| `format_tokens` | `pi-tui/app.rs:347` | 一致（含 `round_one_decimal`） |
| `format_tokens` | `pi-extensions/subagent.rs:151` | 不一致（`<1M` 截断 + tie） |
| `format_tokens` | `pi-coding-agent/core/context_usage.rs:59` | 不一致（阈值不同、大写 K、tie） |
| `truncate_line` | `pi-coding-agent/core/tools/truncate.rs:283` | 不一致（scalar vs UTF-16） |
| `truncate_line` | `pi-agent-core/harness/utils/truncate.rs:203` | 不一致（同上） |
| `estimate_tokens` | `pi-ai/utils/estimate.rs:42` | 不一致（divisor 4 vs 3.5） |
| `estimate_tokens` | `pi-agent-core/harness/compaction/compaction.rs:11` | 不一致（bytes/floor/max1） |
| `estimate_tokens` | `pi-coding-agent/core/compaction.rs:820` | **一致**（`utf16_len.div_ceil(4)`，正确副本） |
| `estimate_tokens` | `pi-coding-agent/core/context_usage.rs:69` | 不一致（bytes，死代码） |
| `format_duration` | `pi-tui/app.rs:684` | 一致 |

**结论：** 同一语义被多次移植且保真度不同，是本次审计最系统性的问题来源。

---

## 4. 建议修复顺序（只列 v0.87.1 目标偏差）

1. **panic 风险**：H3（pi-messages 字节切片）、H6（compaction 字节切片）、H30（Tab 补全字节索引）、H20（`mcp login` runtime 嵌套，虽属未来功能但会崩）。
2. **状态/数据正确性**：H4（工具参数校验）、H8/H11（compaction 切点）、H7（full output）、H9（branch summary 吞错）、H10（idle 早于 listeners）、compaction summary 格式/file lists、abort/teardown、`length` 恢复。
3. **用户可见契约**：H16/H17/H18（CLI 参数）、H19（图片 mime）、H22（模糊排序）、H23/H24（reasoning）、H28/H29（stop reason/partial）、print-mode、RPC（除 disposition）、skills prompt、`/model`、slash commands、auth 键名、OAuth migration、settings `outputPad`。
4. **统一 helper**：`format_size`/`format_tokens`/`truncate_line` 收敛到单一实现；`estimate_tokens` 以 `pi-coding-agent/core/compaction.rs:820` 的正确副本为准，替换 pi-agent-core 的字节版。
5. **补差分 harness**：优先覆盖 `truncate_*`、`format_*`、CLI 参数解析、compaction 切点、工具参数校验。

## 5. 未来差距（仅 v1.5.0 成立，不纳入 v0.87.1 验收）

pi-ai `CHARS_PER_TOKEN=3.5`、`clampMaxTokensToContext` 15% margin、跨模型 thinking 丢弃、`argumentsError`、Anthropic error tool-result 图片拆分；整个 MCP crate（OAuth、binary resources、output-limit、`mcp login`）；RPC `disposition`；首条 user message 持久化；subagent 信号杀死/并行 `isError`；settings `quietStartup:"header"`/`httpIdleTimeoutMs:"disabled"`；edit 逐行换行保留。

> 建议：这些先登记为"未来版本差距"，等 `.version` 升级到 v1.5.0 时再纳入验收；不要按 v0.87.1 的契约去"修复"，否则会引入超前行为。

## 6. 待补工作

- 中/低严重度项尚未逐条按 v0.87.1 重分类（约 50 条）。
- 建议把 7 个范围的 TS 参考统一切到 `../pi`（v0.87.1）后重跑，避免再次混用 v1.5.0。
- 差分 harness 目前只覆盖 `truncate`；其余模块的 A/B 一致性仍未可执行验证。

---

## 7. 第十三轮（2026-11-10）：A 类剩余项的 v0.87.1 复核对齐

本轮把 §0.2 之后仍标为"未修"的 A 类项逐条**按 v0.87.1 源码**复核后再动手。
结果一半是真偏差（已修），另一半其实是 v1.5.0 未来差距（审计当时误记）。

### 7.1 已修（真 · v0.87.1 目标偏差）

| 项 | 位置 | 修复 |
| --- | --- | --- |
| 约束采样线上形状 | `pi-ai/types.rs:GrammarVariants` / `Tool.constrained_sampling` | 去掉错误的 `rename_all="camelCase"`（TS `openai_lark`/`openai_regex` 是 snake_case）；字段加 `rename="constrainedSampling"` + `deserialize_with`（`false`/`null`/缺省 → `None`） |
| 补全分隔符过宽 | `pi-tui/completion.rs:is_separator` | 按 `autocompleteSeparatorRegex` 逐码位对齐：全角字母数字（U+FF10–FF5A）不再切断 token |
| 模糊匹配 | `pi-tui/fuzzy.rs` | 词边界用 `\s`（原来只认字面空格）；alnum 换位要求字母段/数字段各自连续（`a1b2` 不再换位） |
| 滚轮行数 | `pi-tui/terminal.rs` + `app.rs:run` + `interactive.rs` | `wheelScrollLines=1`，按住 Alt ×5（原来固定 3 行、忽略 Alt）；事件携带行数 |
| 渲染 helper | `pi-coding-agent/render_utils.rs` | `normalizeDisplayText` 改为去 CR；`replaceTabs` 4→3 空格；`invalidArgText` → `[invalid arg]` |
| `tool_result` hook | `pi-extension-api/hook.rs` + `coding-agent/extensions/dispatcher.rs` | 新增 `ToolResultHookEvent`/`ToolResultHookPatch`：扩展可见 `toolCallId`/`input`/`usage`，可覆盖 `usage` |
| `ctx.mode` | `pi-extension-api/lib.rs` + rpc/print 模式 | 取值改为 `tui/rpc/json/print`，默认 `print`，三条路径各自设置 |
| `pi config` | `pi-cli/run.rs` | `<key> <value>` 真正落盘（`-l` 写项目级），不再假成功 |
| `pi update` 目标 | `pi-cli/run.rs` / `update.rs` | `--models` 转 `refresh`；`--self`/`self`/`pi` 接受为默认目标 |
| `--list-models` | `pi-cli/list_models.rs` | 无命中 stdout + exit 0；搜索改 `fuzzyFilter(provider + id)`；表格全列左对齐、无分隔线；无模型走登录引导 |
| subagent usage | `pi-extensions/subagent.rs` | `10k–1M` 档 `Math.round`，`toFixed(1)` tie 语义 |

### 7.2 复核后判定为 v1.5.0 未来差距（本轮不动，纠正 §1/§2 的旧记录）

| 审计原记录 | 复核结论 |
| --- | --- |
| `anthropic.rs:697` 应发 `tool_definition` | v0.87.1（`anthropic-messages.ts:1260`）同样发 `tool_reference` —— Rust 正确 |
| `grep.rs` 缺 `--no-config` / 未去 `./` / 未设 `current_dir` / limit 到点即杀 | v0.87.1 `grep.ts` 也没有这些行为（同样 `matchCount >= effectiveLimit` 即杀） |
| `find.rs` 应请求 `limit+1` | v0.87.1 直接传 `limit` |
| `output_accumulator.rs` 应用 `0o600` + `wx` | v0.87.1 用 `createWriteStream` 默认 flag/mode |
| `bash` 缺 `outputSchema`/`structuredContent`/`readFullOutput` | v0.87.1 `bash.ts` 无这些符号 |
| `app.rs:shorten_path` 应用 `isPathWithin` | v0.87.1 `shortenPath` 就是裸 `startsWith(home)`（代码注释已写明） |
| `openai.rs` 缺 `requiresToolsWithToolHistory` / `requiresNonNullAssistantContent` / `promptCacheKeyMode` | v0.87.1 类型里没有这三个 compat 开关 |
| `fuzzy.rs` 缺 best-contiguous 重打分 | v0.87.1 `fuzzy.ts` 无该逻辑 |
| `selection.rs` word joiner 应含 `+ \ ~ _ .` | v0.87.1 只有 `/` `-` |

### 7.3 仍未修 / 待决（本轮明确不动）

| 项 | 现状 | 原因 |
| --- | --- | --- |
| `read/ls/find/grep` 的 `as_u64()/as_f64()` | 仍是整数取参，丢 `Math.floor` / 负数语义 | 复核后确认实际可观察差异只在「模型传入小数 `limit`」这类无效输入上（`offset` 的小数/负数三种写法结果相同），优先级低 |
| `pi package update`（`--extensions` / `pi update <source>`） | 仍打印 "Update not yet implemented" 并 exit 1 | 需要完整的 npm 包管理器语义（TS `PackageManager.updateInstalled`），属功能级缺口，不是行为偏差 |
| TUI `Home`/`End` | 空输入时才滚动；`Ctrl+Home/End` 未处理 | v0.87.1 里同一键同时绑 `tui.altScreen.top` 与 `tui.editor.cursorLineStart`，实际生效取决于渲染器（alt-screen 的 input listener 先于组件、main-screen 不拦），属需要产品决策的 TUI 语义，本轮不改 |
| `grep.rs` context 行逐 match 重读文件 | Rust 无 `fileCache` | 只影响性能（结果相同），不是行为偏差 |

**结论：** 中/低项里"已复核"的标签不能直接当结论用；判定前必须打开 `../pi`
（v0.87.1）对应文件确认。§6 的"中/低项未逐条重分类"仍然是本文件最大的欠账。
