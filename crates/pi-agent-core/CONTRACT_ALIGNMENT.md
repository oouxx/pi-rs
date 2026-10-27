# pi-agent-core 契约级对齐表

> 当前基线：见根目录 [`.version`](../../.version)。
> 同级 `../pi` 的更新版本仅作为未来差距参考。

| 行为场景 | TS 基线行为 | Rust 当前行为 | 状态 |
| --- | --- | --- | --- |
| Agent loop | 流式响应、工具调用、多轮执行，直到无 tool call 或终止 | 已实现 `agent_loop` 多轮执行 | 已对齐核心行为 |
| 工具执行事件 | 发出工具开始、更新、结束事件，并关联 tool call id | 已通过 `AgentEvent` / session event 转发 | 已对齐核心行为 |
| steering / follow-up | 支持运行中排队，并按配置消费 | 已实现队列和消费模式；`peekQueuedMessages()` 预览不消费 | 已对齐核心行为 |
| 回合边界回调 | `finishTurn` 在 `turn_end` 之前运行（error/aborted 也运行，决策忽略）；`{action:"end"}` 结束、`{action:"continue"}` 保证一次后续请求；`prepareRequest` 在每次 provider 请求（含首次）前运行并覆盖 context/model/thinking | 已实现（`agent_loop` + `Agent::finish_turn`/`prepare_request`/`peek_queued_messages`） | 已对齐（扩展边界事件 `AgentBeforeSettle`/`emitBoundary`/`TurnEndEvent` 字段待 P0-4） |
| 工具批次终止 | `terminate` 只停止工具迭代，不跳过 `finishTurn`/`turn_end` | 同款（此前提前 `agent_end` 返回，已修） | 已对齐 |
| abort | 取消当前运行并停止后续工具/模型流程 | 已实现 `abort` 及 session 侧取消路径 | 已对齐核心行为 |
| retry | provider/assistant 错误按 retry policy 重试并发出生命周期事件 | 已实现 agent 级 retry 和相关事件 | 已对齐核心行为 |
| compaction | 压缩请求使用独立 routing session，并支持重试 | 已实现；部分 branch-summary 能力受范围限制 | 有界对齐，见 crate `DEVIATIONS.md` |
| SessionStorage | 支持 session tree、compaction 边界和 cursor 查询 | Rust harness 提供等价 session 存储接口 | Rust 架构不同，需继续补字段级契约 |
| stream function | 未配置时使用默认 stream function；无法使用时显式失败 | 提供默认 stream function 配置和显式错误 | 已对齐核心行为 |
| 消息转换 | AgentMessage 转换为 pi-ai Message，保留工具、usage 和摘要信息 | `harness::messages` 提供转换 | 已对齐核心行为 |
| 流式错误保留 partial | provider catch 发出累计输出；`MessageEnd` 携带 partial 内容 | Error 内容为空时合并最后一次 partial，再写 context / 发事件 | 已对齐 | 修复见 `PORTING_MISTAKES.md`（`stream_assistant_response` terminal Error） |
| AgentMessage 线上格式 | `role` 判别联合 + camelCase 字段（`stopReason`/`toolCallId`/`isError`/`exitCode`/`customType`/`fromId`/`tokensBefore`），`User.content` 允许 `string` 或内容块数组 | 同款：`#[serde(tag = "role", rename_all_fields = "camelCase")]`，逐字段 `alias` 兼容旧 pi-rs 文件，`User.content` 接受字符串简写 | 已对齐（2026-10-26） |
| ContentBlock 线上格式 | `type` 判别联合 + `textSignature`/`thinkingSignature`/`thoughtSignature`/`mimeType` | 同款（camelCase + snake_case alias） | 已对齐（2026-10-26） |
| 工具批次并发 | `Promise.all(finalizedCalls.map(entry => entry()))`：所有已预检的调用并发启动，`tool_execution_end` 按完成序发，结果消息按调用序产出 | `futures::future::join_all` 并发启动，`end` 在各自 future 内发出，`slots` 保序回填结果 | 已对齐（2026-10-26，见 `PORTING_MISTAKES.md`） |
| durable harness | TS 后续版本的 Durable/Pico/Chord 实验性架构 | 未实现 | 范围外，未来如纳入范围再单独审计 |

## 待补充

- 逐个公开 `Agent` / `AgentOptions` / `AgentHarness` 方法的参数和错误契约；
- session storage 的 name/stats/cursor/retained-tail 字段级对照；
- 后续 `../pi` 版本中 pi-ai 与 coding-agent 调用方的事件增量；
- 使用 mock stream 固化事件序列黄金用例。

未实现或有意保留的差异必须转入 `crates/pi-agent-core/DEVIATIONS.md`，不能只留在本表的“待补充”中。
