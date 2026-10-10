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

## 权威文档

- 历史对齐差距：[`docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md`](docs/archive/ALIGNMENT_GAPS_V0.79_TO_V0.82.1.md)
- 与 `../pi` 当前版本的后续差距：仅在需要时以当前代码重新审计，不单独维护差距表；
- 有意偏差：根目录及各 crate 的 `DEVIATIONS.md`
- 契约对齐：各 crate 的 `CONTRACT_ALIGNMENT.md`
- 历史可行性/专项审计：[`docs/archive/`](docs/archive/)

> 发现行为差异时，先查对应 crate 的 `DEVIATIONS.md`，再按阶段四流程处理；不要把历史差距表当作当前代码状态。
