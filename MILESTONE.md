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
| P0-3 agent loop 边界：`finishTurn`/`prepareRequest`/`peekQueuedMessages` + `TurnEndEvent` 扩展 + `AgentBeforeSettleEvent`/`emitBoundary` | 部分完成 | agent-core 部分（`finish_turn`/`prepare_request`/`peek_queued_messages` + `terminate` 语义）已完成；扩展边界事件（`AgentBeforeSettle`/`emitBoundary`/`TurnEndEvent` 字段）依赖 P0-4 的 session 投影/`ContextEditEntry`，随 P0-4 落地 |
| P0-4 `ContextEditEntry` + SessionManager 权威化 | 部分完成 | SessionManager 层已完成：`ContextEdit` 条目、`build_session_projection`/`ProjectedSessionEntry`/`SessionProjection`、`append_context_edit`（校验 + 内容归一化）、`append_compaction(first_kept: Option<&str>)`（`None` 自保留）；`build_context` 改为经 projection。剩余：agent-session 逐请求投影（typed AgentMessage）+ `refreshContext` + 边界事件（`AgentBeforeSettle`/`emitBoundary`/`TurnEndEvent` 字段） |
| P0-5 `ToolCall.arguments`/`details` 限 JSON + `user_bash` fail-closed | 部分 | 类型层面 `serde_json::Value` 已满足；`user_bash` 目前 fail-open（`operations` 未接线，非法结果回落本地执行），需对齐或登记偏差 |
| P0-1 / P0-6 provider / coding-agent 修复批次 | 长尾，待逐项审计 | 与 `TranscriptContext`（P1-7）纠缠的修复留给 P1 |

### P1（架构大改，单独排期）

- P1-7 pi-ai `Context`→`TranscriptContext` + transcript-backed 中途 system/tool 变更（最长杠杆）
- P1-8 ModelRuntime / ai Models 运行时 API
- P1-9 harness v4 lane（拟维持 pi-rs 单 lane + 登记偏差）

## 权威文档

- 历史对齐差距：[`docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md`](docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md)
- 与 `../pi` 当前版本的后续差距：仅在需要时以当前代码重新审计，不单独维护差距表；
- 有意偏差：根目录及各 crate 的 `DEVIATIONS.md`
- 契约对齐：各 crate 的 `CONTRACT_ALIGNMENT.md`
- 历史可行性/专项审计：[`docs/archive/`](docs/archive/)

> 发现行为差异时，先查对应 crate 的 `DEVIATIONS.md`，再按阶段四流程处理；不要把历史差距表当作当前代码状态。
