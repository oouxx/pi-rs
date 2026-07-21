# DEVIATIONS.md — pi-coding-agent

按 CLAUDE.md 阶段四的偏差日志格式记录。这里的条目状态均为"已确认
保留"：对齐检查（无论人工还是 Claude Code 自动跑）遇到下面两类差异
时，直接跳过，不得尝试"纠正"回原版实现。

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| 扩展系统（extension 模块整体） | `packages/coding-agent` 里扩展系统的具体实现方式（如插件加载、hook 注册的内部机制） | 内部实现方式不按原版逐行翻译，改用 Rust 生态更合适的机制（如 WASM / subprocess IPC，具体选型见 PORTING.md） | 用户决定 | 已确认保留 |
| TUI 渲染层（对应原 `packages/coding-agent` 内 TUI 组件，即 `pi-tui`） | 原版 TUI 组件的具体渲染实现 | 本次范围内不复刻 | 用户决定：TUI 不在 pi-coding-agent 当前移植范围内 | 已确认保留 |

## 对"已确认保留"条目的额外约束

### 扩展系统

**允许偏离的范围：内部实现。不允许偏离的范围：对外 interface 和函数
行为。** 具体来说：

- 扩展系统暴露给上层（`pi-coding-agent` 其他模块、以及未来 `pi-tui`
  如果要接入）的公开函数签名、参数、返回类型、错误类型、事件
  variant，必须和原 TS 版本在语义上一致——即阶段一分析文档里"对外
  接口"那一节列出的东西，不能因为内部实现变了就跟着变
- 阶段三的"契约级：接口行为对齐"对扩展系统模块**仍然适用，不能因为
  这条已经登记在 DEVIATIONS.md 就跳过**。DEVIATIONS.md 里免检的是
  "内部怎么实现"，不是"对外行为对不对"。换句话说，行为对照表里
  "是否一致"这一列，扩展系统模块依然要填真实结果，不能因为有这条
  登记就默认填"是"
- 如果后续发现内部实现方式的改变导致对外行为出现了不该有的偏差
  （比如错误类型变了、事件触发时机变了），这属于新的、未登记的偏差，
  走 CLAUDE.md 4.2 节的"未记录偏差"流程处理，不能套用这条已确认的
  登记

### TUI

- 本条目只覆盖"不逐行复刻/不在本次范围内实现"这件事本身，不代表
  TUI 以后也不需要对齐检查——一旦 TUI 部分被纳入某次任务范围，需要
  单独走阶段一到阶段三，这条 DEVIATIONS 记录届时应该更新或移除，不
  要留着一条过期的"已确认保留"误导后续判断
- 在此之前，任何对齐检查、契约对照表遇到 TUI 相关的公开接口缺失，
  视为"范围外"，不算作差异，不需要在对照表里体现
| `reload()` / `_buildRuntime()` | TS `reload()` 调用 `_buildRuntime()` 重建整个 ExtensionRunner（重新从磁盘加载扩展文件、重建工具注册表、重新绑定所有回调） | Rust `reload()` 只调用 `settings_manager.reload()`，不重建 ExtensionRegistry | Rust 扩展通过 `Arc<ExtensionRegistry>` 在构造时一次性注册，运行时不支持热重载。TS 扩展是文件驱动的动态加载（`.ts`/`.js` 文件 → ResourceLoader → ExtensionRunner），Rust 扩展是程序化注册的静态引用（`registry.register()` → `Arc<ExtensionRegistry>`），没有"运行时重建"的概念 | 已确认保留 |
| `bind_extensions()` | TS `bindExtensions()` 设置 `_extensionUIContext`、`_extensionMode`、`_extensionCommandContextActions`、`_extensionAbortHandler`、`_extensionShutdownHandler`、`_extensionErrorListener`，然后调用 `_applyExtensionBindings()` 和 emit `session_start` | Rust `bind_extensions()` 接受 `ExtensionBindings` 结构体，存储相关字段但不执行动态绑定 | Rust 扩展通过 `ExtensionContext` 和 `EventPublisher` 直接通信，没有 TS 的 ExtensionRunner 回调注册机制。`_applyExtensionBindings()` 的等价逻辑在构造时通过 `ExtensionContext::new()` 完成 | 已确认保留 |
| `create_replaced_session_context()` | TS 从 `_extensionRunner.createCommandContext()` 创建 `ReplacedSessionContext`，附加 `sendMessage`/`sendUserMessage` 方法 | Rust 返回一个简化的 `ReplacedSessionContext` 结构体，包含 `send_message`/`send_user_message` 闭包 | Rust 没有 ExtensionRunner 的 `createCommandContext()` 方法。等价功能通过 `ExtensionContext` 直接暴露给扩展 | 已确认保留 |
| `export_html()` 主题支持 | TS 使用 `settingsManager.getTheme()` + `createToolHtmlRenderer()` + `exportSessionToHtml()` 生成带主题和工具渲染的 HTML | Rust 使用内联 CSS 生成简化版 HTML，不支持主题切换和工具自定义渲染 | 主题系统和工具 HTML 渲染器是 TUI 层功能，不在 pi-coding-agent 当前范围内。基础 HTML 导出功能已实现 | 已确认保留 |
