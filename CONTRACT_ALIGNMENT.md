# CONTRACT_ALIGNMENT.md

本文件是各 crate 契约对齐表的索引，不替代 crate 级明细。

| Crate | 契约文档 | 当前范围 |
| --- | --- | --- |
| `pi-ai` | [`crates/pi-ai/CONTRACT_ALIGNMENT.md`](crates/pi-ai/CONTRACT_ALIGNMENT.md) | provider、模型、消息、流式事件 |
| `pi-agent-core` | [`crates/pi-agent-core/CONTRACT_ALIGNMENT.md`](crates/pi-agent-core/CONTRACT_ALIGNMENT.md) | Agent loop、工具循环、队列、session/compaction |
| `pi-coding-agent` | [`crates/pi-coding-agent/CONTRACT_ALIGNMENT.md`](crates/pi-coding-agent/CONTRACT_ALIGNMENT.md) | session、工具、RPC、ACP、扩展 |
| `pi-extensions` | [`crates/pi-extensions/CONTRACT_ALIGNMENT.md`](crates/pi-extensions/CONTRACT_ALIGNMENT.md) | 扩展公开工具和事件 |
| `pi-tui` | [`crates/pi-tui/CONTRACT_ALIGNMENT.md`](crates/pi-tui/CONTRACT_ALIGNMENT.md) | 核心交互和渲染状态 |

## 版本口径

- 对齐/验收基准：TS `v0.82.1`；
- TS 当前版本：`v1.0.0`，仅用于识别未来差距；
- v0.82.1 之后的差异不改变当前基准；需要启动新版本审计时另建阶段性文档；
- 有意差异必须引用对应 crate 的 `DEVIATIONS.md`，不能只在此处写“范围外”。

`pi-agent-core` 的契约表已建立；后续 TS v1.0.0 差距和逐方法公开 API 细节仅作为未来工作记录，不改变当前 v0.82.1 基准。
