# DEVIATIONS.md — pi-coding-agent

按 CLAUDE.md 阶段四的偏差日志格式记录。这里的条目状态均为"已确认
保留"：对齐检查（无论人工还是 Claude Code 自动跑）遇到下面两类差异
时，直接跳过，不得尝试"纠正"回原版实现。

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| 扩展系统（extension 模块整体） | `packages/coding-agent` 里扩展系统的具体实现方式（如插件加载、hook 注册的内部机制） | 内部实现方式不按原版逐行翻译，改用 Rust 生态更合适的机制（如 WASM / subprocess IPC，具体选型见 PORTING.md） | 用户决定 | 已确认保留 |
| 扩展 UI（notify/dialog/select/input） | interactive 模式下扩展的 `ui.*` 调用显示在 TUI 上 | **2026-08-23 已接线**：interactive 模式注入 TUI 桥接（notify/set_status→system 消息，confirm→Dialog 弹窗，select→选择列表，input→编辑器，set_editor_text→输入框，set_title→终端标题）；`set_widget`（inline widget）仍 no-op；对话框阻塞等待最长 120s | TUI 恢复后的解冻项 | 已确认保留（仅 set_widget 部分） |
| 工具执行审批门（tool approval gate，interactive 模式） | TS 原版无此功能：`beforeToolCall` 仅用于扩展 dispatch，工具调用直接执行 | **2026-08-23 新增**：interactive 模式在 agent 的 `before_tool_call` 前置审批门——工具执行前 TUI 显示 `[a] Approve [d] Deny` 提示并阻塞等待；`a` 放行（fall-through 到扩展 dispatch）、`d`/`Esc` 拒绝（生成 error 工具结果，`Tool call denied by user`，agent 继续）；审批期间键盘归审批所有；同一时刻最多一个待审批工具（Mutex 串行化）；`Agent::prepend_before_tool_call` 组合 outer 门 + inner 扩展 hook | 有意新增的用户功能（超出 TS 原版），非对齐差异 | 已确认保留 |
| 块折叠 + scrollback 布局（pi-tui） | TS 原版 TUI 无折叠概念：消息/工具输出平铺，无 Ctrl+F 折叠交互 | **2026-08-23 新增（借鉴 grok-build 哲学）**：`pi-tui/src/scrollback/`（vendored 自 grok `xai-grok-pager`，见 THIRD-PARTY-NOTICES §4）——统一块模型（消息/工具块带稳定 id）、`DisplayMode` 三态、折叠扫描+投影（连续完成工具调用聚合 "N tool calls"、超长内容 "N more" 截断）、Ctrl+F 折叠/展开最近可折叠块、滚动百分比指示。行为差异：完成工具默认折叠（TS 显示 8 行截断） | 有意新增的工程特性（grok 对齐），非对齐差异 | 已确认保留 |
| TUI 渲染层（对应原 `packages/coding-agent` 内 TUI 组件，即 `pi-tui`） | 原版 TUI 组件的具体渲染实现 | **2026-08-23 起已纳入范围（最小可用版）**：恢复历史 Elm 架构 pi-tui（ratatui 0.29），markdown 渲染整体改用 vendored grok-build `xai-grok-markdown`（见 THIRD-PARTY-NOTICES.md），不按原版自研 | 用户决定：先恢复最小可用，后续再逐模块对齐原版 | 已确认保留 |

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

- **2026-08-23 更新**：TUI 已以最小可用形态纳入范围（`pi-cli -i` →
  interactive 模式 → pi-tui Elm 架构；markdown 渲染依赖 vendored
  `xai-grok-markdown`，见 THIRD-PARTY-NOTICES.md）。本条偏差的含义收窄为：
  **组件层不按原版 TS 逐行复刻**（Ratatui + grok 管线替代原版自研组件库）。
- 仍不在范围内的：原版全部组件/选择器（session/model/theme/settings
  selector 等 17k 行 TS）尚未移植，属“后续任务”，不算差异。
- 已登记的简化：~~消息 wrap 是 `word_wrap_line_with_joiners` 的简化版~~
  **2026-08-23 第二轮已升级**：wrap 现为 grok 原版 joiner 实现（vendored
  `pi-tui/src/render/wrap.rs`，含表格行保护/blockquote 续行前缀/单调游标
  线性复杂度）；joiners 已缓存（`Markdown::joiners()`）供后续复制保真接线。
  仍简化的：多行编辑已换 grok textarea（12.7k 行 widget）；slash 命令中
  `/quit`/`/exit` 静默无效、`CycleModel`/`SetThinkingLevel` 等为 no-op
  stub——属最小可用简化。
- 一旦后续对齐检查覆盖 TUI 组件，需要单独走阶段一到阶段三；本条记录
  届时更新或移除，不要留着过期的“已确认保留”误导后续判断。
| `reload()` / `_buildRuntime()` | TS `reload()` 调用 `_buildRuntime()` 重建整个 ExtensionRunner（重新从磁盘加载扩展文件、重建工具注册表、重新绑定所有回调） | Rust `reload()` 只调用 `settings_manager.reload()`，不重建 ExtensionRegistry | Rust 扩展通过 `Arc<ExtensionRegistry>` 在构造时一次性注册，运行时不支持热重载。TS 扩展是文件驱动的动态加载（`.ts`/`.js` 文件 → ResourceLoader → ExtensionRunner），Rust 扩展是程序化注册的静态引用（`registry.register()` → `Arc<ExtensionRegistry>`），没有"运行时重建"的概念 | 已确认保留 |
| `bind_extensions()` | TS `bindExtensions()` 设置 `_extensionUIContext`、`_extensionMode`、`_extensionCommandContextActions`、`_extensionAbortHandler`、`_extensionShutdownHandler`、`_extensionErrorListener`，然后调用 `_applyExtensionBindings()` 和 emit `session_start` | Rust `bind_extensions()` 接受 `ExtensionBindings` 结构体，存储相关字段但不执行动态绑定 | Rust 扩展通过 `ExtensionContext` 和 `EventPublisher` 直接通信，没有 TS 的 ExtensionRunner 回调注册机制。`_applyExtensionBindings()` 的等价逻辑在构造时通过 `ExtensionContext::new()` 完成 | 已确认保留 |
| `create_replaced_session_context()` | TS 从 `_extensionRunner.createCommandContext()` 创建 `ReplacedSessionContext`，附加 `sendMessage`/`sendUserMessage` 方法 | Rust 返回一个简化的 `ReplacedSessionContext` 结构体，包含 `send_message`/`send_user_message` 闭包 | Rust 没有 ExtensionRunner 的 `createCommandContext()` 方法。等价功能通过 `ExtensionContext` 直接暴露给扩展 | 已确认保留 |
| `ExtensionEvent` / `ExtensionAPI` → `HookHandler` / `HookRunner` | TS 使用 `ExtensionEvent` enum + `ExtensionAPI` trait 的事件分发机制，通过 `event_from_agent_event()` 转换层将 `AgentEvent` 映射为扩展事件 | Rust 使用 `HookHandler` trait（所有方法有默认空实现）+ `HookRunner`（按 priority 排序分发），void hooks 并行 fire-and-forget，modifying hooks 顺序执行可 Cancel。参考 ZeroClaw 的 Hook 系统设计 | 消除 `ExtensionEvent` enum 的转换层开销，简化扩展实现（只需实现关心的方法）。事件语义等价：`on_session_start` ≈ `session_start` event，`before_tool_call` ≈ `tool_call` event 等 | 已确认保留 |
| `export_html()` 主题支持 | TS 使用 `settingsManager.getTheme()` + `createToolHtmlRenderer()` + `exportSessionToHtml()` 生成带主题和工具渲染的 HTML | Rust 使用内联 CSS 生成简化版 HTML，不支持主题切换和工具自定义渲染 | 主题系统和工具 HTML 渲染器是 TUI 层功能，不在 pi-coding-agent 当前范围内。基础 HTML 导出功能已实现 | 已确认保留 |

## V8 扩展加载器 — JS 依赖 shim（新增）

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| `js_shims.rs` — typebox shim | 扩展通过 `import { Type } from "typebox"` 使用完整 typebox 库（v1.1.38，含 Memory/Create/Settings 等内部机制） | 提供最小化 typebox shim，仅实现扩展实际使用的 9 个 `Type.*` 方法（String/Number/Boolean/Object/Array/Optional/Literal/Union/Unsafe），`~kind`/`~optional` 标记为 non-enumerable（与 typebox 默认 `enumerableKind: false` 一致），`JSON.stringify` 产出干净的 JSON Schema | V8 运行时无 `node_modules`，无法加载完整 typebox 包。扩展仅使用上述 9 个方法，shim 行为与原版在 JSON Schema 输出上完全一致 | 已确认保留 |
| `js_shims.rs` — @earendil-works/pi-ai shim | 扩展通过 `import { StringEnum } from "@earendil-works/pi-ai"` 使用 typebox-helpers 中的 StringEnum 函数 | 提供等价的 StringEnum 实现，委托 typebox shim 的 `Type.Unsafe` 产出 `{type:"string", enum:[...], ...options}` | 同上，V8 运行时无 node_modules。同时 re-export `Type` 供 `import { Type } from "@earendil-works/pi-ai"` 的扩展使用 | 已确认保留 |
| `js_shims.rs` — @earendil-works/pi-tui shim | 扩展通过 `import { Box, Text, matchesKey, ... } from "@earendil-works/pi-tui"` 使用 TUI 组件 | 所有导出为 stub：组件类构造时抛出 "TUI unavailable"，工具函数调用时抛出。仅 import 不调用时扩展可正常加载 | TUI 渲染层不复刻（见上方 TUI 偏差条目）。扩展在 `ctx.mode === "tui"` 条件下才调用 TUI 组件，非 TUI 模式下 import 但不调用 | 已确认保留 |
| `js_shims.rs` — @earendil-works/pi-coding-agent shim | 扩展通过 `import { VERSION, CONFIG_DIR_NAME, defineTool, getAgentDir, ... } from "@earendil-works/pi-coding-agent"` 使用 coding-agent 的公开 API | 提供常量（VERSION/CONFIG_DIR_NAME 等）、简单工具函数（defineTool/parseFrontmatter/getAgentDir/truncateHead 等）的等价实现；复杂工具工厂（createBashTool/createReadTool 等）为抛出 stub | V8 运行时无法提供 Rust 实现的工具工厂。需要调用工具工厂的扩展（5/68）在 factory load 阶段失败，属预期行为 | 已确认保留 |
| `js_shims.rs` — Node.js built-in shims | 扩展通过 `import * as fs from "node:fs"` / `import { homedir } from "os"` 等使用 Node.js 内置模块 | 提供 `node:fs`/`fs`/`node:path`/`path`/`node:os`/`os`/`node:child_process`/`child_process`/`node:util`/`util`/`node:url`/`url`/`node:module`/`node:readline`/`node:process`/`process`/`node:fs/promises`/`fs/promises` 的 stub。`node:path` 提供等价的字符串操作实现；`node:os` 提供基于 Rust 传入全局的 homedir/tmpdir/platform；其余模块函数调用时抛出 | V8 运行时非 Node.js 环境，无法提供真实 fs/child_process 等。扩展在 event handler 中使用这些模块（非 factory load 阶段），import 可解析但调用时抛出 | 已确认保留 |
| `js_runtime.rs` — Node.js globals | 扩展直接使用 `process.cwd()`、`process.env`、`process.stdout.write()` 等 Node.js 全局变量 | 在 V8 runtime bootstrap 中注入 `process` 全局对象，提供 `cwd()`/`env`/`platform`/`stdout.write()` 等最小实现 | V8 运行时非 Node.js 环境，但扩展代码中直接引用 `process` 全局（非 import），需要提供以避免 ReferenceError | 已确认保留 |
| `js_runtime.rs` — 5 个扩展加载失败（预期） | TS 原版可加载全部 68 个示例扩展 | 63/68（93%）可成功加载。5 个失败：`bash-spawn-hook.ts`、`built-in-tool-renderer.ts`、`minimal-mode.ts`、`ssh.ts`（均在 factory load 阶段调用 `createBashTool`/`createReadTool` 等工具工厂，需要 Rust 工具的 JS 等价实现）；`preset.ts`（在 factory load 阶段调用 `Key.ctrlShift` TUI 匹配器） | 工具工厂需要完整 Rust 工具的 JS 桥接（超出当前范围）；TUI Key 匹配器需要 TUI 层实现（已确认偏差）。这 5 个扩展的失败是上述两条已确认偏差的直接后果 | 已确认保留 |
