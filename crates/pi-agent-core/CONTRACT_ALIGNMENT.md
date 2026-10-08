# pi-agent-core 契约级对齐表

> 当前基线：TS `v0.82.1`。
> TS `v1.0.0` 仅作为未来差距参考；本表不表示当前目标已切换到 v1.0.0。

| 行为场景 | TS v0.82.1 基线行为 | Rust 当前行为 | 状态 |
| --- | --- | --- | --- |
| Agent loop | 流式响应、工具调用、多轮执行，直到无 tool call 或终止 | 已实现 `agent_loop` 多轮执行 | 已对齐核心行为 |
| 工具执行事件 | 发出工具开始、更新、结束事件，并关联 tool call id | 已通过 `AgentEvent` / session event 转发 | 已对齐核心行为 |
| steering / follow-up | 支持运行中排队，并按配置消费 | 已实现队列和消费模式 | 已对齐核心行为 |
| abort | 取消当前运行并停止后续工具/模型流程 | 已实现 `abort` 及 session 侧取消路径 | 已对齐核心行为 |
| retry | provider/assistant 错误按 retry policy 重试并发出生命周期事件 | 已实现 agent 级 retry 和相关事件 | 已对齐核心行为 |
| compaction | 压缩请求使用独立 routing session，并支持重试 | 已实现；部分 branch-summary 能力受范围限制 | 有界对齐，见 crate `DEVIATIONS.md` |
| SessionStorage | 支持 session tree、compaction 边界和 cursor 查询 | Rust harness 提供等价 session 存储接口 | Rust 架构不同，需继续补字段级契约 |
| stream function | 未配置时使用默认 stream function；无法使用时显式失败 | 提供默认 stream function 配置和显式错误 | 已对齐核心行为 |
| 消息转换 | AgentMessage 转换为 pi-ai Message，保留工具、usage 和摘要信息 | `harness::messages` 提供转换 | 已对齐核心行为 |
| v1.0 durable harness | TS v1.0 的 Durable/Pico/Chord 实验性架构 | 未实现 | 范围外，未来如纳入范围再单独审计 |

## 待补充

- 逐个公开 `Agent` / `AgentOptions` / `AgentHarness` 方法的参数和错误契约；
- session storage 的 name/stats/cursor/retained-tail 字段级对照；
- v1.0.0 之后 pi-ai 与 coding-agent 调用方的事件增量；
- 使用 mock stream 固化事件序列黄金用例。

未实现或有意保留的差异必须转入 `crates/pi-agent-core/DEVIATIONS.md`，不能只留在本表的“待补充”中。
