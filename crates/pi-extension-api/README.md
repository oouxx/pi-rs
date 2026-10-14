# pi-extension-api

扩展系统的对外 API 层：hook trait 与分发表、工具/命令/快捷键/flag 注册、扩展上下文、
边界协议，以及扩展事件与结果类型。

- 契约表：[`CONTRACT_ALIGNMENT.md`](CONTRACT_ALIGNMENT.md)
- 结构性偏差（程序化注册 vs 文件驱动加载、V8 shim、无热重载）：根目录
  [`DEVIATIONS.md`](../../DEVIATIONS.md) 的「扩展系统」小节

## 谁在用

- `pi-extensions`：内置扩展（`goal`、`web_search`、`subagent`）实现 `HookHandler`；
- `pi-coding-agent`：持有 `Arc<ExtensionRegistry>`，把 hook 接到会话/工具/边界事件上；
- `pi-tui`：经 `ExtensionUIContext` 提供交互（notify/confirm/select/input/widget/title）。
