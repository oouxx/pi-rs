# DEVIATIONS.md — pi-coding-agent

按 CLAUDE.md 阶段四的偏差日志格式记录。这里的条目状态均为"已确认
保留"：对齐检查（无论人工还是 Claude Code 自动跑）遇到下面两类差异
时，直接跳过，不得尝试"纠正"回原版实现。

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| 扩展系统（extension 模块整体） | `packages/coding-agent` 里扩展系统的具体实现方式（如插件加载、hook 注册的内部机制） | 内部实现方式不按原版逐行翻译，改用 Rust 生态更合适的机制（如 WASM / subprocess IPC，具体选型见 PORTING.md） | 用户决定 | 已确认保留 |
| 扩展 UI（notify/dialog/select/input） | interactive 模式下扩展的 `ui.*` 调用显示在 TUI 上 | **2026-08-23 已接线**：interactive 模式注入 TUI 桥接（notify/set_status→system 消息，confirm→Dialog 弹窗，select→选择列表，input→编辑器，set_editor_text→输入框，set_title→终端标题）；`set_widget`（inline widget）仍 no-op；对话框阻塞等待最长 120s | TUI 恢复后的解冻项 | 已确认保留（仅 set_widget 部分） |
| 工具执行审批门（tool approval gate，interactive 模式） | TS 原版无此功能：`beforeToolCall` 仅用于扩展 dispatch，工具调用直接执行 | **2026-08-23 新增后已移除**：曾实现 `[a] Approve [d] Deny` 审批门 + YOLO 模式（Ctrl+Y / `/yolo`）；**2026-08-24 用户决定禁用**——approval_hook 不再安装、`ToolApproval`/`yolo_mode`/`SetYolo`/`ToolApprove`/`ToolDeny`/`ToolApprovalPending` 及相关 UI（徽章、yolo footer badge、a/d 键、Ctrl+Y、`/yolo` 命令）全部删除，工具调用直接执行，行为回到 TS 原版 | 用户决定禁用（对齐 TS 原版） | 已移除 |
| 块折叠 + scrollback 布局（pi-tui） | TS 原版 TUI 无折叠概念：消息/工具输出平铺，无 Ctrl+F 折叠交互 | **2026-08-23 新增后已移除**：曾实现 grok 式块折叠（`pi-tui/src/scrollback/`、`DisplayMode` 三态、"N tool calls" 聚合、"N more" 截断、Ctrl+F、滚动百分比）；**2026-08-24 用户决定删除并复刻 TS 原版**——`scrollback/` 模块整体删除，工具块始终完整渲染（完成工具不再折叠成一行），输出默认预览 10 行 + `... (N more lines, ctrl+o to expand)`（TS `FALLBACK_PREVIEW_LINES`=10 语义），Ctrl+O 展开全部，消息永不截断（转录滚动查看），滚动百分比指示删除 | 用户决定删除（复刻 TS 原版） | 已移除 |
| TUI 渲染层（对应原 `packages/coding-agent` 内 TUI 组件，即 `pi-tui`） | 原版 TUI 组件的具体渲染实现 | **2026-08-23 起已纳入范围（最小可用版）**：恢复历史 Elm 架构 pi-tui（ratatui 0.29），markdown 渲染整体改用 vendored grok-build `xai-grok-markdown`（见 THIRD-PARTY-NOTICES.md），不按原版自研。**2026-08-23 第三轮（样式对齐）**：主视图布局与配色已按 TS 原版 interactive 模式复刻——dark.json 调色板（accent `#8abeb7`、text `#d4d4d4`、userMessageBg `#343541`、toolPending/Success/ErrorBg `#282832/#283228/#3c2828` 等）、用户消息全宽背景盒、工具调用状态色盒（粗体标题+灰色输出）、移除顶栏，改为 TS 式底部 dock（spinner 状态行 + `─` 边框编辑器 + 两行 footer：dim cwd(branch) + 着色 context% + 右对齐模型名）。**2026-08-23 第六轮（像素级复刻，复用 vendor）**：① 启动 header（TS `builtInHeader` ExpandableText）——logo `Pi v1.83.1`（accent 粗体 + dim）+ 紧凑/展开两态快捷键提示（`escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more`，展开列出全部 19 条），Ctrl+O（`app.tools.expand`）切换并同时展开全部工具输出；② footer 两行完整对齐 TS `FooterComponent`——line 1 `pwd (branch) • sessionName`，line 2 `↑in ↓out Rcache Wcache CH% $cost ctx%/window (auto)`（context 阈值着色 error>90/warning>70，`?/window` 未知态）+ 右对齐 `(provider) model • thinking`（reasoning 模型显示 thinking level）；③ 工具调用渲染 TS fallback 格式（粗体标题 + 空行 + `JSON.stringify(args, null, 2)` 默认色 + 输出）；④ assistant thinking 块（`thinkingText` 斜体）+ 终止原因提示（length/aborted/error 三态，error 色，对齐 TS `AssistantMessageComponent`）；⑤ 转录内容适配 TS ScrollView 锚定语义（内容不足一屏时顶对齐，溢出时跟随底部）；⑥ 状态栏/编辑器/对话框保持前几轮形态。工具输出预览 10 行 + `... (N more lines, ctrl+o to expand)` 提示（TS fallback 语义），Ctrl+O 展开。**2026-08-24 第七轮（工具块像素对齐）**：bash 工具走 TS 专用 renderer（`core/tools/bash.ts`）——`$ {command}` 粗体标题 + muted ` (timeout Ns)` 后缀、输出未展开时预览**尾部 5 视觉行** + 前导 `... (N earlier lines, ctrl+o to expand)`（`BASH_PREVIEW_LINES`=5）、muted `Elapsed {x.x}s`（运行中，tick 驱动每秒刷新）/ `Took {x.x}s`（完成，`formatDuration` 同款 `(ms/1000).toFixed(1)`）；其他工具保持 fallback（10 行尾部提示、无计时器）；**输出统一 toolOutput 色**——移除此前 Rust 增强的 diff +/- 着色（TS renderer 无此行为）。**2026-08-24 第八轮（主屏幕模式 + 工具块位置 + renderer 差距）**：①**主屏幕差分渲染**（对齐 TS `TuiMainScreen`）——移除 alternate screen，整个 UI 渲染进终端主屏幕（差分输出 + resize 清屏重放）；**2026-08-24 用户决定回退①**：主屏幕模式因退出清理缺失与 resize 尺寸错误问题回退为 alt screen + ratatui 直渲（`EnterAlternateScreen`/`LeaveAlternateScreen` + `ratatui_terminal().draw()`，恢复 HEAD 基线；详见 PORTING_MISTAKES.md #118），②③④保留；②**工具块位置**：tools 与 messages 按共享 block-id 时间序穿插（TS chatContainer 顺序——工具块紧跟请求它的 assistant 消息之后，不再全部堆在消息上方）；③**read/grep/edit 紧凑 renderCall**：read `read {path}{:range}`（accent path + warning 范围，未展开只显示标题——TS `formatReadResult` 空串）、grep `grep /{pattern}/ in {path} ({glob}) limit {n}` + 15 行预览、edit `edit {path}` + diff 文本（TS 的 diff widget 渲染未复刻，见差距）；④**bash/read 截断警告**：`[Full output: path. Truncated: showing N of M lines]` warning 色（从结果 `details.truncation` 提取）。 | 用户决定：先恢复最小可用，再逐模块对齐原版 | 已确认保留 |

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
- **2026-08-23 第三轮更新（样式对齐）**：主视图（transcript + dock）的布局
  与配色已与 TS 原版对齐（见上方表格条目）：顶栏移除，模型名/上下文占用量
  移入底部两行 footer；输入框改为边框式编辑器（borderMuted `─`）；用户消息
  全宽 `userMessageBg` 色块、工具调用按 pending/success/error 三态色块、
  assistant 纯文本——与 TS interactive 模式一致。工具输出预览 10 行 + `... (N more lines, ctrl+o to expand)` 提示（TS fallback 语义），Ctrl+O 展开。
- **2026-08-23 第四轮（bug 审查修复）**：工具状态链与 slash 命令审查修复（详见
  PORTING_MISTAKES.md #112）：工具事件全链按 `tool_call_id` 键（同名工具不再
  互相污染）、工具输出改为快照替换语义（修复输出从不显示/重复叠加）、bash 流式
  输出可见；slash 菜单填充内建+扩展命令（`/` 弹出、Enter 选中即执行，对齐 TS
  下拉行为）、`/model` 结果回传反馈（成功更新 footer/失败报错）、`/new` 清转录、
  Shift+Enter 换行。
- **2026-08-23 第五轮（slash 菜单样式对齐）**：slash 命令菜单改为 TS 原版
  SelectList 样式——内联渲染在编辑器内部（无边框/标题/背景），`→ ` accent
  前缀 + accent 选中文本、muted 描述对齐第二列（主列宽上限 32）、最多 5 行、
  滚动 `(n/m)` 提示、无匹配 `No matching commands`；Tab 应用补全、Right 不再
  选中（详见 PORTING_MISTAKES.md #113）。
- **2026-08-25 第六轮（补全对齐）**：Tab 改为**应用选中项**（与 TS
  `tui.input.tab` 一致、Down 才是下一个）、Enter 对 `/` 补全应用后提交 / 对
  `@` 补全应用后不提交、Tab 无弹窗时触发补全（不再插 4 个空格）、应用时替换
  整段前缀（对齐 TS `applyCompletion`）。
- **2026-08-25 第七轮（补全功能补齐）**：
  ① `@` 文件补全——`pi-tui/src/completion.rs` 用 `ignore` 遍历（gitignore/
  hidden/follow/排除 `.git`，对齐 TS 的 fd 语义）+ `scoreEntry` 打分 + 目录优先
  + 引号/`~`/`./`/绝对路径 + 目录不加空格可继续补全；② 命令参数补全——
  `/model` 用可用模型快照 fuzzy 过滤（对齐 TS `getModelSearchText`），扩展命令
  走其 `get_argument_completions`（pi-extension-api 原有回调，此处接线）；
  `/login` 参数补全未做（Rust 无 `/login` 命令，见下方"待确认"）；③ fuzzy 匹配：
  移植 `fuzzy.ts`（子序列打分 + 数字字母换位回退），slash 命令与 `/model`
  参数共用；④ 触发上下文——行首 `/`、`@` 仅 token 边界（行首/空白后）触发、
  附件补全 20ms debounce（TS `ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS`）、Tab
  强制路径补全；⑤ `autocompleteMaxVisible` 设置接线（session settings →
  completer `max_visible`，clamp 3..=20）。实现细节：slash 命令候选同步算出
  （TS 侧 fuzzy 同步、debounce 0，行为等价；同步消除 Enter 竞态）；`@`/参数
  结果异步回填 + `request_seq` 丢弃过期 + `has_fresh_results`（Enter/Tab 只
  应用与当前输入一致的结果）。详见 PORTING_MISTAKES.md #121/#122。
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

## 剪贴板（2026-08-25，pi-tui `src/clipboard.rs` + app.rs 快捷键 + interactive `/copy`/Ctrl+X）

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| Ctrl+V 图片粘贴（`handleClipboardPaste` 的图片路径） | TS 先 `readClipboardImage()`（原生 addon `@mariozechner/clipboard` 的 hasImage/getImageBinary + photon 解码），剪贴板有图片时写临时文件（`pi-clipboard-{uuid}.{ext}`）并把文件路径插入编辑器 | 仅文本路径：`read_clipboard_text()` 有文本则插入光标处；图片路径未复刻 | 图片剪贴板需要原生剪贴板插件 + 图像解码；Rust 端无原生插件（文本路径行为一致） | 待确认 |
| 剪贴板写通道顺序（`copy_to_clipboard`） | ① 原生 addon（非 Linux）→ ② 平台工具（pbcopy/clip/termux-clipboard-set/wl-copy/xclip/xsel）→ ③ OSC 52 兜底（`ESC ] 52 ; c ; base64 ST`，>100k 编码字符丢弃）；远程会话仍先试原生/工具、最后补发 OSC 52 | 无原生 addon；远程会话（SSH_CONNECTION/SSH_CLIENT/MOSH_CONNECTION）直接发 OSC 52（不试 pbcopy/clip）；Linux 无 DISPLAY/WAYLAND_DISPLAY 时也尝试 xclip/xsel（TS 需 DISPLAY 才试） | Rust 无原生剪贴板插件；远程优先 OSC 52 避免把内容写进远端机器剪贴板（TS 注释也承认 OSC 52 写的是本地终端剪贴板）；无 DISPLAY 也试 xclip 对仅 XAUTHORITY 的场景更宽容 | 待确认 |
| 剪贴板读（`read_clipboard_text`） | macOS/Windows 用原生 addon `getText()`；Linux 上 Wayland 用 `wl-paste --no-newline --type text`，其余走原生 addon（clipboard-rs，X11）；全部静默失败返回 null | 仅平台工具：macOS `pbpaste`、Windows `powershell Get-Clipboard -Raw`、Linux `wl-paste`/`xclip`/`xsel`；失败静默返回 None | 无原生 addon；平台工具覆盖等价场景 | 待确认 |
| 渲染策略：每帧全量重绘（interactive.rs 主循环 / app.rs `run()`） | TS `TuiMainScreen` 用虚拟 buffer 差分渲染（只输出变化的 cell） | **2026-08-25 起每帧先 `Terminal::clear()`（`ESC[2J` + 重置 back buffer）再整帧重绘**——不依赖 ratatui diff 对终端的保真度 | 真实终端（tmux / Windows Terminal / CJK 宽字符局部覆盖有缺陷的终端）对 diff 输出的局部 cell 更新应用不一致，滚动时留下"每帧固定在原位"的陈旧字符（一长串残留）。全量重绘在任何终端上都不会残留；代价是每帧输出量等于整屏（30×100 量级，帧率下可接受；终端把 2J+重绘批处理为单帧呈现，无闪烁）。PTY e2e 已用 ANSI 模拟器验证滚动不变量（↑↓ 往返、gg/G、流式、resize） | 已确认保留 |
