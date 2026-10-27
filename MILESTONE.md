# 对齐里程碑

> 本文件记录 pi-rs 与同级 `../pi`（TypeScript 原版）的版本基准和对齐范围。

## 当前版本基准

| 项目 | 说明 |
| --- | --- |
| 对齐/验收基准 | 见根目录 [`.version`](.version)（TS 原版版本 + commit） |
| Rust workspace 版本 | 见根目录 [`Cargo.toml`](Cargo.toml) `[workspace.package].version` |
| 同级 TS 参考 | `../pi` 的当前检出，仅用于识别未来差距，不改变 `.version` 基准 |

## 当前结论

pi-rs 已基本完成 `.version` 所记录版本的核心行为对齐，覆盖：

- `pi-ai`：Anthropic/OpenAI 主流调用、流式响应、工具调用、reasoning、usage、重试和模型注册；
- `pi-agent-core`：Agent loop、工具循环、队列、abort、compaction、retry 和事件流；
- `pi-coding-agent`：内置工具、session、prompt template、skills、扩展基础能力、RPC/ACP 和模型目录；
- `pi-tui`：基础交互、工具渲染、slash/file completion、队列、压缩和重试状态显示。

pi-rs 当前以 `.version` 记录的版本作为对齐和验收基准。同级 `../pi` 的更新版本仅作为后续差距识别的参考，不改变本项目的对齐目标。

| 范围 | 当前状态 |
| --- | --- |
| Anthropic/OpenAI 核心 Agent 路径 | 继续对齐，优先级高 |
| session、model runtime、MCP 基础行为 | 部分已实现，需要对照 `../pi` 当前版本重新审计 |
| OAuth、完整 provider/model catalog | 有界支持；差异见各 crate `DEVIATIONS.md` |
| codemode、image/classifier | 未完整移植，暂不作为核心里程碑 |
| client/server/protocol | 不在当前核心范围 |
| Durable/Pico/Chord | 不在当前核心范围 |
| TUI 组件逐行复刻 | 不在范围；核心交互状态机仍需保持一致 |

## v0.82.1 → v0.87.1 对齐计划（已收口，2026-10-12 起）

`.version` 已切到 `v0.87.1`。按“先正确性/契约（P0），再架构大改（P1）”推进；
TUI 方向不在本轮范围。权威差距来源是同级 `../pi` 的 `v0.82.1..v0.87.1` 变更。

### P0（正确性 / 契约）

| 项 | 现状 | 说明 |
| --- | --- | --- |
| P0-2 `message_update` delta-only（JSON/RPC） | ✅ 已完成（此前 cherry-pick） | `modes/json_event.rs::to_json_event` 已去除 `message`/`partial`，print/RPC 均已接线 |
| P0-3 agent loop 边界：`finishTurn`/`prepareRequest`/`peekQueuedMessages` + `TurnEndEvent` 扩展 + `AgentBeforeSettleEvent`/`emitBoundary` | ✅ 完成 | agent-core 部分（`finish_turn`/`prepare_request`/`peek_queued_messages` + `terminate` 语义）已完成；扩展边界协议（`HookRunner::run_boundary` + `on_turn_end_boundary`/`on_agent_before_settle_boundary`）在 P0-4 落地 |
| P0-4 `ContextEditEntry` + SessionManager 权威化 | ✅ 完成 | SessionManager 层 + 逐请求权威投影 + 扩展边界接线（`finish_turn` → turn_end 边界、settle 前 `agent_before_settle` 边界；草稿预览/提交、`continue` 延续）已完成。残留差异：void `on_turn_end` 通知 hook 保留（pi-rs 内部通知通道）|
| P0-5 `ToolCall.arguments`/`details` 限 JSON + `user_bash` fail-closed | ✅ 完成 | 类型层面 `serde_json::Value` 已满足；`user_bash` 已 fail-closed（恰好一个字段、operations 不支持、非法结果不回落本地） |
| P0-1 / P0-6 provider / coding-agent 修复批次 | 基本完成（限三个通用适配器） | **P0-1（anthropic + openai-completions + openai-responses）**：审计显示绝大多数已对齐；本轮补 `tool_choice` 无 tools 门控（#8607）、`supportsMaxOutputTokens`（#8941）、未知端点默认非 strict + Cerebras 排除（#9816/#9804，含目录显式值）；Google/Bedrock/Mistral 适配器未实现不适用。**P0-6**：GIF 误判（#9755）、`--mode`（#9045）、`--provider` 无 `--model`（#10236）、非法 prompt frontmatter（#9830）、split-turn 摘要措辞（#9908）、write 字节计数（#8979）、信号终止 shell（#9577）、`user_bash` fail-closed（属 P0-5）；剩余与 `TranscriptContext`（P1-7）纠缠或不对应本 port 架构 |

### P1（架构大改）

**P1-7 `Context` → `TranscriptContext`（进行中）**：TS `9e05370b2` 把 system prompt / tool 声明改成 transcript 里的 system 消息（`toolsAdded`/`toolsRemoved`/`sections`），provider 用 `getCurrentSystemPrompt`/`getCurrentTools` 重放。分阶段：

1. ✅ pi-ai transcript 纯函数（`utils/transcript.rs`：normalize / collapse / resolve / replay / tool-delta 探测）+ `ToolReference` 类型（7 测试）
2. ✅ 类型层：`SystemMessage` 变体进 `Message`（`SystemContent` string|blocks、`sections`、`toolsAdded`/`toolsRemoved`）+ 迁移全部匹配点（providers / estimate / transform / serialize_conversation；provider 暂以 no-op arm 占位，待第 3 步接管）；serde 往返测试
3. ✅ provider 请求构建：不支持 mid-convo 的模型走 `Context::collapsed()`；支持者（Anthropic/Responses）native 发送（见第 6 步）
4. ✅ agent-core：`AgentMessage::System` + `declare_tool_changes` **已接线**——`run_agent_loop` 把 base prompt/tools 实体化为前置 system 消息并在 pending 前声明工具 delta；`stream_assistant_response` 改为从 transcript 构建请求（`systemPrompt`/`tools` 侧道不再传入，仅作为输入源）；直接调用者回退补前置 system 消息
5. ✅ coding-agent：`build_system_prompt_message` 把 sections 落到前置 system 消息（初始消息带 sections）；system 消息参与持久化与投影（`refresh_context` 保留）；`diff_system_prompt_sections` 已就绪。段顺序由 `preserve_order` + `IndexMap` 保证
6. ✅ provider native mid-convo：元数据 + Anthropic（system 消息 + **tool changes**：`tool_removal`/`tool_addition`、初始工具缓存断点、placeholder/later `defer_loading`、beta flag）+ Responses（system 消息；工具变更由已有 `split_deferred_tools` 覆盖），不支持者 `collapsed()`。**P1-7 全部完成**

- **P1-8 ModelRuntime / ai `Models` 运行时 API**（基本完成，OAuth 除外）
  1. ✅ 访问器：`getError`、`getAvailableSnapshot`、`getModel`、`getRegisteredProviderIds`、`isUsingSubscription`、`getProviderAuthStatus`
  2. ✅ 运行时密钥：`setRuntimeApiKey`/`removeRuntimeApiKey`、`listCredentials`（共享 `AuthStorage` 接入 `ModelRegistry`，优先于文件 resolver）
  3. ✅ provider-resolved `streamSimple`/`completeSimple`（解析 auth 后调 pi-ai）
  4. N/A：`refresh` 已支持 abort signal；`context.stored`/`publish` 是自定义 provider 动态刷新契约，而 pi-rs `register_provider` 不支持 `refreshModels`（`DEVIATIONS.md` #23），不适用
  5. ⛔ OAuth `login`/`logout`：用户明确不做（已登记范围外）

- **P1-9** harness v4 lane——拟维持 pi-rs 单 lane + 登记偏差

**P0 / P1 的收口结论已于 2026-10-14 复核并修正。** 当时的“均已收口”不成立：
复核发现 P0（mid-convo 模型请求体丢失工具声明）、P1 若干（边界守卫、`agent_settled` 顺序、
`_pendingCustomMessages`、内置工具采样、`findCutPoint` 回退、`toolcall_start` 线格式、
请求级 deferred 拆分、per-model 压缩预算、retry 退避上限、`context_with_system` 等）
并未真正对齐，且“剩余未做项均为不适用/已登记偏差/范围外”也不准确——其中多项当时
既未实现**也未登记**。

修正后的口径：

- 上述 P0/P1 已完成修复，逐条见 `PORTING_MISTAKES.md` 与各 crate 的 `CONTRACT_ALIGNMENT.md`；
- 未实现的**特性**已逐条登记为偏差（状态 `待确认`）：`pi-ai/DEVIATIONS.md` #14–#17
  （per-turn thinking effort、reasoning_details 流式、image input limits、meta/radius provider）、
  `pi-coding-agent/DEVIATIONS.md` #27、#29、#30（cache warming 展示面、`/bug`、PowerShell 工具）；
- 仍未决的既有偏差：`pi-coding-agent/DEVIATIONS.md` #13（mistral 模型在目录但无适配器）；
- 其余范围外项不变：TUI 组件逐行复刻、MCP 高级（codemode/exposure/resources/OAuth）、
  OAuth 登录、Google/Bedrock/Mistral 适配器、harness 多 lane。

**结论：以 `.version` 为基准的核心行为已对齐；剩余差距以 `DEVIATIONS.md` 的登记清单为准，
不再用“已收口”这类笼统表述。**

## 2026-10-26 偏差文档复核（一轮 bug 修复）

对根 + 4 个 crate 的 `DEVIATIONS.md` 逐条复核（TS 基准 `f07218c4d` / v0.87.1），
结论：约一半条目是"实现方式不同"的真偏差，另有 13 条实质是把 bug/缺失登记成了
"有意保留"，已按下面顺序修完并更新文档：

1. `read` 工具的 Ctrl+O 展开（此前恒截 10 行）、Ctrl+C 空闲清编辑器（此前为空操作，
   但 header 写着 `ctrl+c clear`）、RPC `confirm`/`editor`/`timeout`、RPC `fork`
   的 `position:"before"` 语义（此前分支包含被选中的消息，客户端重发会重复）；
2. 扩展会话生命周期事件（`session_before_switch`/`session_before_fork`/
   `session_shutdown` 三个 dispatcher 此前是死代码，扩展无法取消切换、退出也不清理）；
3. 工具批次真并发（"并行"此前逐个 `await`，实为串行）、`ctx.hasUI` 在 RPC/ACP 下
   为 false；
4. 目录里 279 个没有后端适配器的模型不再出现在 `/model`/初始模型解析里（此前会选中
   并必然报 `No API provider registered`），并加目录一致性测试；
5. `AgentMessage`/`ContentBlock` 线上格式改为 TS 的 camelCase（此前 `--mode json`、
   RPC、pi-acp、TS 会话文件全不兼容），`User.content` 接受 `string` 简写；
6. 文档：删掉不存在的 V8 shim 整节与 10 条已回退条目，改正 20+ 条不实/过期描述，
   把当时仍未实现的能力（anthropic managed-effort、openai `reasoning_details`、
   `inputLimits`、`session_before_compact` 完整 payload）从"待确认"如实标成
   "待排期（未实现）"——这些在第二轮已全部补上。

细节见各 crate 的 `PORTING_MISTAKES.md`（pi-coding-agent #156–#165、pi-tui #25/#26、
pi-agent-core 三条、pi-ai 五条）与 `PORTING.md` 高危陷阱 #33–#35。

**第二轮（同日）把"待排期"的缺口也补了**：

7. anthropic managed per-turn effort（`providerThinkingLevel` + 逐轮 `output_config` 标记
   + `block_binding` + 两个 beta）——opus-5 / fable-5-1 的多轮 400 不再出现；
8. openai `reasoning_details` 流式合并与回放（含补发的 `thinking_end`），OpenRouter 类
   网关多轮签名保留；
9. `Model.inputLimits` 三层打通（类型 + `ResizeProfile` 合并 + `read` 与工具结果图片按当前
   模型缩放 + 非 vision 提示）、read 非 UTF-8 文件改 lossy 解码（对齐 TS）；
10. `session_before_compact` 完整 payload/result（preparation/branchEntries/
    customInstructions/signal + 扩展直接提供 compaction），派发时机改到 prepare 之后；
11. 扩展 UI 补齐 `set_widget`（TUI 编辑器上/下内联行）、`set_working_message`/
    `set_working_visible`、`get_editor_text`；仍缺的只剩需要组件树的
    `custom`/`set_footer`/`set_header`/`on_terminal_input`/补全 provider/编辑器组件/主题
    查询（已在根 DEVIATIONS「扩展 UI」逐项列明）。

## 权威文档

- 历史对齐差距：[`docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md`](docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md)
- 与 `../pi` 当前版本的后续差距：仅在需要时以当前代码重新审计，不单独维护差距表；
- 有意偏差：根目录及各 crate 的 `DEVIATIONS.md`
- 契约对齐：各 crate 的 `CONTRACT_ALIGNMENT.md`
- 历史可行性/专项审计：[`docs/archive/`](docs/archive/)

> 发现行为差异时，先查对应 crate 的 `DEVIATIONS.md`，再按阶段四流程处理；不要把历史差距表当作当前代码状态。
