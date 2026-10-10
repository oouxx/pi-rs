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

- 对齐/验收基准：见根目录 [`.version`](.version)（文档与对齐检查统一以此为准，不再写死版本号）；
- 同级 `../pi` 的更新版本：仅用于识别未来差距；
- 基准版本之后的差异不改变当前基准；需要启动新版本审计时另建阶段性文档；
- 有意差异必须引用对应 crate 的 `DEVIATIONS.md`，不能只在此处写“范围外”。

`pi-agent-core` 的契约表已建立；后续与 `../pi` 当前版本的差距和逐方法公开 API 细节仅作为未来工作记录，不改变 `.version` 基准。
