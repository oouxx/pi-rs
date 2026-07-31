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
