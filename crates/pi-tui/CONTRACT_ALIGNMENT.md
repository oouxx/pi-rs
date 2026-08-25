# CONTRACT_ALIGNMENT.md — pi-tui

按 CLAUDE.md 阶段三的契约级对齐表记录 pi-tui 公开渲染行为与 TS 原版
（`packages/coding-agent/src/modes/interactive` + `packages/tui`）的对照。
"是否一致"列填"否"时必须引用 `DEVIATIONS.md` 对应条目。

## 主视图（transcript + dock）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 启动 header | `builtInHeader` ExpandableText：logo `Pi v{version}`（accent 粗体 + dim）+ 紧凑提示行 + `Press ctrl+o to show full startup help...` + 空行 + onboarding；Ctrl+O 展开为 19 条完整快捷键列表 | `header_lines()` 渲染相同内容（logo/紧凑/展开/onboarding），Ctrl+O 切换 | 是 | |
| 转录锚定 | ScrollView `follow: "end"`：内容不足一屏时 `scrollTop = 0`（顶对齐），溢出时跟随底部 | 内容不足一屏时顶对齐，溢出时跟随底部（alt screen 内裁剪，历史靠转录自身滚动） | 是 | |
| 渲染模式 | 默认 `tuiMode: "regular"`（TuiMainScreen，虚拟 buffer 差分渲染） | **2026-08-24 曾改为主屏幕差分渲染后回退**：alt screen + ratatui 直接渲染（对齐 HEAD 基线）；**2026-08-25 起每帧 `Terminal::clear()` + 整帧重绘**（不依赖 diff 保真度，见 DEVIATIONS.md 渲染策略条目；`view()` 内仍保留缓冲 Clear，TestBackend 单测不变） | 否 | 有意保留 alt screen + 全量重绘（见 DEVIATIONS.md；主屏模式因退出清理/ resize 问题回退） |
| 工具块位置 | chatContainer 按事件顺序追加：工具块紧跟请求它的 assistant 消息 | blocks 按共享 block-id 时间序排序穿插 | 是 | |
| 用户消息 | `UserMessageComponent`：全宽 `userMessageBg` 盒（padX=1, padY=1）+ markdown | `render_user_block`：全宽背景盒 + markdown 行 | 是 | |
| 工具调用（bash） | `bash.ts` renderer：`$ {command}` 粗体标题（非字符串 command → error 色 `[invalid arg]`，空/缺失 → toolOutput 色 `...`）+ muted ` (timeout Ns)`（truthy 判断，保留小数）；输出未展开时预览尾部 5 视觉行（`BASH_PREVIEW_LINES`=5，`truncateToVisualLines` 词边界 wrap）+ 前导空行 + `... (N earlier lines, ctrl+o to expand)`；渲染前剥离结果文本里的截断 footer（`\n\n[Showing ... Full output: path]`，避免与 warning 行重复）；muted `Elapsed {x.x}s`（partial 期间每秒刷新）/ `Took {x.x}s`（完成，`formatDuration` = `(ms/1000).toFixed(1)`，半进位舍入）；截断警告 `[Truncated: ...]`/`[Full output: ...]` | `render_bash_tool_block`：同款标题（`BashCommandArg` 三态 + f64 timeout）；`bash_display_text` 先剥 footer 再尾部 5 视觉行（`visual_lines` 词边界 wrap，tab→3 空格）；`Elapsed`/`Took` + `format_duration`（tick 刷新）；warning 色截断警告（`details.truncation` 经 bridge 传入，maxBytes 缺省回退 50KB）；输出为空（含纯空白）时不渲染标题后空行 | 是 | |
| 工具调用（read） | `formatReadCall`：`read {path}{:range}`（`read` 粗体 + accent path + warning 范围）；`formatReadResult` 未展开且非错误返回空（仅标题）；展开/错误时渲染前 10 行 + 尾部提示 + 截断警告（`[First line exceeds ...]`/`[Truncated: showing N of M lines (N line limit)]`/`[Truncated: N lines shown (X limit)]`） | `render_read_tool_block`：同款标题；`read_content_visible`（展开或 Failed）才渲染内容（前 10 raw 行词边界 wrap + 尾部提示）+ `read_warning_text` 三种警告同款 | 是 | |
| 工具调用（grep） | `formatGrepCall`：`grep /{pattern}/ in {path} ({glob}) limit {n}`；输出预览 15 raw 行（Text 词边界 wrap）+ 截断警告 `[Truncated: N matches limit, X limit, some lines truncated]`（", " 连接） | `render_grep_tool_block`：同款标题 + 15 raw 行 wrap + `grep_warning_text` 同款（`match_limit_reached`/`lines_truncated` 经 bridge 传入） | 是 | |
| 工具调用（write） | `formatWriteCall`：`write {path}`（`write` 粗体 + `renderToolPath` accent path，非字符串 path → error `[invalid arg]`，空/缺失 → toolOutput `...`）；空行 + `args.content` 前 10 行（toolOutput 色，词边界 wrap） + `... (N more lines, M total, ctrl+o to expand)`（含 total）；非字符串 content → error `[invalid content arg - expected string]`；`normalizeDisplayText`（去 `\r`）+ `trimTrailingEmptyLines` + `replaceTabs`（→3 空格）；空 content 仅标题；`formatWriteResult` 成功返回空、错误时 error 色输出 | `render_write_tool_block`：同款标题（`WritePathArg`/`WriteContentArg` 三态）+ 前 10 行 wrap + 带 total 的尾部提示 + 空/缺失 path `...` + content 标准化 + 错误结果 error 色输出；无语法高亮（TS 仅在 `getLanguageFromPath` 命中时高亮，Rust 工具渲染器不解析语言，统一走非高亮分支） | 是 | |
| 工具调用（edit） | `formatEditCall`：`edit {path}` + 专用 diff widget 渲染 | `render_edit_tool_block`：`edit {path}` 标题 + diff 文本（toolOutput 色） | 否 | diff widget（行号/着色/折叠）未复刻——diff 文本内容一致，样式为纯文本（见 DEVIATIONS.md TUI 条目） |
| 工具调用（fallback，无 renderer） | `formatToolExecution`：toolName 粗体 + 空行 + `JSON.stringify(args, null, 2)` + 空行 + 输出（`toolOutput` 色，未展开时前 10 raw 行 + 尾部 `... (N more lines, ctrl+o to expand)`），无计时器 | `render_fallback_tool_block`：同款（args/输出均词边界 wrap，args 与输出间空行，10 行 + 尾部提示） | 是 | |
| 工具输出清洗 | `getTextOutput`：`sanitizeBinaryOutput(stripAnsi(text)).replace(/\r/g, "")` 后再渲染 | `set_tool_output` 统一 `sanitize_output_text`（strip ANSI + 控制字符过滤 + 去 `\r`），bash/read/grep/edit/fallback 共用 | 是 | |
| 工具输出换行 | TS `Text` 渲染器词边界 wrap（`wrapTextWithAnsi`），tab→3 空格，每行 trimEnd | `visual_lines` 词边界 wrap（`break_long_token` CJK 感知、tab→3 空格、trimEnd） | 是 | |
| 工具输出颜色 | 统一 `toolOutput` 色（bash 与 fallback renderer 均不按 diff 着色） | 统一 `toolOutput` 色（diff +/- 着色已移除） | 是 | |
| 工具状态色 | pending/success/error 三态背景 `toolPendingBg`/`toolSuccessBg`/`toolErrorBg` | `tool_bg()` 三态同色 | 是 | |
| assistant 文本 | `AssistantMessageComponent`：markdown 纯文本（outputPad=1） | `render_assistant_block`：markdown 纯文本 | 是 | |
| assistant thinking | thinking 块渲染为 `thinkingText` 色 + 斜体 markdown | `render_assistant_block` thinking 段：`thinkingText` + ITALIC | 是 | |
| 终止原因提示 | `stopReason === "length"` → `Response was truncated before completion.`；`aborted` → `Operation aborted`/errorMessage；`error` → `Error: {msg}`（error 色，前有空行） | `StopReason` 三态同文案同色 | 是 | |
| 系统消息 | 状态通知为 dim/muted 纯文本 | `render_system_block`：muted 纯文本 | 是 | |
| 块间距 | 每个块前有 Spacer(1)（用户/工具/assistant 内部） | gap_after=1（user/tool/system/header），assistant 无前置空行 | 否 | 见 DEVIATIONS.md TUI 条目（assistant 前置空行未复刻，历史布局决定） |

## Dock

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 状态区 | `Loader`：`["", spinner + message]`（accent spinner + muted 文案） | `render_status`：空行 + spinner 行（busy 时） | 是 | |
| 编辑器 | `CustomEditor`：`─` 边框（borderMuted），内容随输入增长，`max(5, 30% 行高)` | `render_input`：`─` 上下边框 + 内容行 + 光标跟随 | 是 | |
| slash 菜单 | SelectList 内联在编辑器内（无边框/标题，`→ ` accent 前缀，muted 描述对齐，最多 5 行 + `(n/m)`） | `Completer::render_rows` 同款 | 是 | |
| footer line 1 | `pwd (branch) • sessionName`（dim，`~` 替换 HOME） | `render_footer` line 1 同款 | 是 | |
| footer line 2 | `↑in ↓out Rcache Wcache CH% $cost ctx%/window (auto)`（dim，context 阈值着色 error>90/warning>70，`?/window` 未知态）+ 右对齐 `(provider) model • thinking`（reasoning 模型） | `render_footer` line 2 同款（`format_tokens` 与 TS `formatTokens` 逐分支一致） | 是 | |
| footer line 3 | 扩展状态行（`getExtensionStatuses()`） | 无 | 否 | Rust 扩展系统无 footer data provider 等价物（见 DEVIATIONS.md 扩展系统条目） |

## 交互

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| Ctrl+O | `app.tools.expand`：展开 header + 全部工具输出 | `Msg::ToggleToolExpansion`：切换 header 展开态 + 工具块强制 Expanded | 是 | |
| Ctrl+C | `app.clear` → `handleCtrlC`：500ms 内连按两次 shutdown，否则清空编辑器（setText 同时 cancelAutocomplete） | 同款（仅 Chat 模式）：第一次清空 + 关补全弹窗，500ms 内第二次返回 `Cmd::Quit`；对话框/选择器聚焦时不拦截 | 是 | |
| Ctrl+D | 空编辑器时 `app.exit` → shutdown（CustomEditor 非空时交给编辑器 deleteCharForward，默认绑定 ctrl+d） | 空输入返回 `Cmd::Quit`；非空 `delete()`（删除光标后一字符）；仅 Chat 模式 | 是 | |
| Ctrl+V | `app.clipboard.pasteImage` → `handleClipboardPaste`：先读图片（有则插入临时文件路径），否则读文本插入光标处；失败静默 | 仅文本路径：`read_clipboard_text()` 插入光标处，失败静默；仅 Chat 模式 | 否 | 图片粘贴未复刻（见 DEVIATIONS.md 剪贴板条目） |
| Ctrl+X | `app.message.copy` → `handleCopyCommand`（复制最后一条 assistant 消息；alt screen 上 flash "Copied!"） | `Cmd::CopyLastMessage` → interactive 模式 agent 任务执行，system 消息回显三态结果；仅 Chat 模式 | 是 | flash 换成 system 消息（Rust 无 flash 概念） |
| `/copy` | slash 命令 → `handleCopyCommand`（清空编辑器） | slash_command 清空编辑器 → 同一 `AgentCmd::CopyLastMessage` 任务 | 是 | |
| 块折叠 / Ctrl+F | 无（工具/消息平铺渲染） | 无（已删除，2026-08-24） | 是 | 见 DEVIATIONS.md 块折叠条目（已移除） |
| 工具输出截断 | fallback：未展开时前 10 行 + `... (N more lines, ctrl+o to expand)`（`FALLBACK_PREVIEW_LINES`=10），展开时全部 | `FALLBACK_PREVIEW_LINES`=10 + 同款提示（muted 文案 + dim 键名），Ctrl+O 展开 | 是 | |
| 工具执行 | 无审批门：`beforeToolCall` 仅用于扩展 dispatch，工具调用直接执行 | 无审批门：approval_hook 不安装，工具调用直接执行 | 是 | 审批门已于 2026-08-24 移除（见 DEVIATIONS.md） |
| 滚动 | ScrollView scrollBy/scrollTo（PageUp/Down、gg/G） | `ScrollUp/ScrollDown/ScrollToBottom` + gg/G | 是 | |

## 数据流（pi-coding-agent → pi-tui）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 工具 args | `ToolExecutionComponent(args)` 渲染 `JSON.stringify(args, null, 2)` | `AgentEvent::ToolStart(call_id, name, args_json)` → `Msg::ToolStart` | 是 | |
| thinking 流 | `thinking_delta` 事件 → thinking 块 | `MessageEnd { thinking, ... }` 携带完整 thinking（非逐 delta） | 是 | 渲染结果一致；流式 thinking 逐 delta 未接线（agent 事件桥只转发 text delta） |
| 终止原因 | `Done`/`Error` 事件携带 `stopReason`/`errorMessage` | `MessageEnd` 携带 `stop_reason`/`error_message` | 是 | |
| footer 数据 | `FooterComponent` 从 session entries 累计 usage totals + `getContextUsage()` | `usage_totals_from_entries()` + `refresh_status()`（1s 节流，run 间查询） | 是 | 刷新节流 1s（TS 每次 message_end 更新） |

## 剪贴板（`src/clipboard.rs`，2026-08-25 新增）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 写（macOS） | 原生 addon setText → pbcopy | `pipe_to("pbcopy")`（远程会话先 OSC 52） | 是（有意偏差） | 通道顺序差异见 DEVIATIONS.md 剪贴板条目 |
| 写（Windows） | 原生 addon setText → clip | `pipe_to("clip")` | 是（有意偏差） | 同上 |
| 写（Linux） | termux-clipboard-set（TERMUX_VERSION）→ wl-copy（Wayland，spawn + 等退出码）→ xclip → xsel（X11，需 DISPLAY） | 同款顺序；无 DISPLAY 也试 xclip/xsel | 是（有意偏差） | 同上 |
| 写（OSC 52） | `\x1b]52;c;{base64}\x07`，`Buffer.toString("base64")`，>100k 编码字符丢弃；远程或全失败时发出 | 同款序列 + 手写 RFC 4648 base64（测试向量对齐），>100k 丢弃 | 是 | |
| 读（macOS/Windows） | 原生 addon getText（null on fail） | pbpaste / `powershell Get-Clipboard -Raw`（None on fail） | 是（有意偏差） | 见 DEVIATIONS.md 剪贴板条目 |
| 读（Linux） | Wayland：wl-paste --no-newline --type text；否则原生 addon | wl-paste → xclip → xsel | 是（有意偏差） | 同上 |
| 失败语义 | 读静默 null；写全通道失败 throw `Error("Failed to copy to clipboard")` | 读静默 None；写全通道失败 `Err` → interactive 模式 system 消息 | 是 | |
| 对话框 Editor（AppMode::Editor）剪贴板 | TS dialog editor 自身处理 ctrl+c/v/x（复制选区/粘贴/剪切） | vendored textarea 原生处理（`SystemClipboard` provider：ctrl+v 粘贴、ctrl+x 剪切选区、鼠标选复制），应用级快捷键不拦截 | 是 | |
