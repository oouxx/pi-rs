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

## v0.82.1 → v0.87.1 对齐计划（进行中，2026-10-12 起）

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
3. 🔄 provider 请求构建：三个适配器入口调 `Context::collapsed()`（把 transcript system 消息折叠回 `systemPrompt`/`tools` 并丢弃，对齐 `collapseSystemMessages`）——**collapse 路径已接线**；支持 mid-convo system 的 provider 原样发送待后续
4. ✅ agent-core：`AgentMessage::System` + `declare_tool_changes` **已接线**——`run_agent_loop` 把 base prompt/tools 实体化为前置 system 消息并在 pending 前声明工具 delta；`stream_assistant_response` 改为从 transcript 构建请求（`systemPrompt`/`tools` 侧道不再传入，仅作为输入源）；直接调用者回退补前置 system 消息
5. ✅ coding-agent：`build_system_prompt_message` 把 sections 落到前置 system 消息（初始消息带 sections）；system 消息参与持久化与投影（`refresh_context` 保留）；`diff_system_prompt_sections` 已就绪。段顺序由 `preserve_order` + `IndexMap` 保证
6. ✅ provider native mid-convo（system 消息）：元数据 + Anthropic + Responses 均完成；支持模型保留 transcript 并就发送后续 system 消息，不支持者 `collapsed()`。**未做**：native tool changes（`tool_addition`/`tool_removal` + `defer_loading` 占位）——工具仍按当前集合顶层发送（与 opencode/copilot 行为一致）

- **P1-8** ModelRuntime / ai Models 运行时 API（auth signal / stored+publish / isSubscription）
- **P1-9** harness v4 lane——拟维持 pi-rs 单 lane + 登记偏差
## 权威文档

- 历史对齐差距：[`docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md`](docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md)
- 与 `../pi` 当前版本的后续差距：仅在需要时以当前代码重新审计，不单独维护差距表；
- 有意偏差：根目录及各 crate 的 `DEVIATIONS.md`
- 契约对齐：各 crate 的 `CONTRACT_ALIGNMENT.md`
- 历史可行性/专项审计：[`docs/archive/`](docs/archive/)

> 发现行为差异时，先查对应 crate 的 `DEVIATIONS.md`，再按阶段四流程处理；不要把历史差距表当作当前代码状态。
