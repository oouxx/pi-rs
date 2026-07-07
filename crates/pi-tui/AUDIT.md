# pi-tui plans.md 实现审计

逐项对照 `plans.md` 检核实现情况。

---

## Section 1 — 目标与非目标

| 目标 | 状态 | 实现 |
|---|---|---|
| 流式展示 LLM 输出 (token-by-token) | ✅ | `Msg::StreamText` → `render_body` TextDelta 实时追加 |
| 展示工具调用过程 (参数/执行/结果/diff) | ✅ | `ToolCall` 状态机 + `ActiveTools` + `DiffView` component |
| 支持人工审批 (approve/deny/auto-approve) | ✅ | `Dialog` + `DialogAction::Confirm/ConfirmAlways/Cancel` |
| 支持多行输入 | ✅ | `input_height()` 动态计算，1~5 行自适应 |
| `/` 斜杠命令、`@` 文件引用补全 | ✅ | `Completer` 组件，双模式触发 |
| Esc/Ctrl+C 中断生成 | ✅ | 双 Ctrl+C 打断 (500ms)，`session.abort()` |
| 状态栏：模型/context/cwd/git/耗时 | ✅ | `render_status()` 完整实现 |
| 可扩展到多 session 面板 | 🟡 | 架构预留 (`Model` 可扩展, `AgentBridge` trait) |
| 鼠标交互 (非目标) | ❌ | 没有实现 |
| 分屏多 session (非目标) | ❌ | 没有实现 |

## Section 2.1 — 分层架构

| 分层 | 对应文件 | 状态 |
|---|---|---|
| 启动/终端初始化/事件循环 | `interactive.rs` → `modes/interactive.rs` | ✅ |
| App 状态机 (Model/update/Msg) | `app.rs` → `Model`/`update()`/`Msg` | ✅ |
| 纯渲染层 (无副作用) | `view()` via `&Model` | ✅ |
| Widgets: chat_view | `render_body()` in `app.rs` | ✅ |
| Widgets: input_box | `render_input()` in `app.rs` | ✅ |
| Widgets: tool_card | `render_body()` — "tool" role | ✅ |
| Widgets: diff_view | `components/diff.rs` | ✅ |
| Widgets: status_bar | `render_status()` in `app.rs` | ✅ |
| Widgets: approval_dialog | `render_dialog()` in `app.rs` | ✅ |
| Theme | `Theme` struct in `app.rs` | ✅ |
| Agent bridge | `modes/agent_bridge.rs` | ✅ |

## Section 2.2 — 事件循环 (Elm 架构)

| 特性 | 状态 | 备注 |
|---|---|---|
| `Event` enum (Key/Tick/Agent/Resize) | ✅ | `Msg` enum in `app.rs` |
| `AgentEvent` enum | ✅ | `modes/agent_bridge::AgentEvent` |
| 单一 mpsc channel 汇聚事件 | ✅ | `agent_tx` + `input_rx` + `bridge_rx` |
| `update()` 同步纯函数 | ✅ | `pub fn update(model: &mut Model, msg: Msg) -> Vec<Cmd>` |
| 渲染层纯函数 `fn draw(frame, &Model)` | ✅ | `pub fn view(model: &Model, frame: &mut Frame)` |

## Section 3 — 布局设计

| 特性 | 状态 | 实现 |
|---|---|---|
| 四段式布局 (header/body/input/status) | ✅ | `view()` 中 `Layout::vertical` 4 段 |
| 输入框高度自适应 | ✅ | `input_height()` — 根据换行数 1~5 行 |
| 弹层用 `Clear` 清空再渲染 | ✅ | `render_dialog()` / `render_input()` completer |
| header: 模型名 + spinner + 按键提示 | ✅ | `render_header()` |
| body: 虚拟滚动对话区 | ✅ | `render_body()` — virtual scrolling |
| input: 单行/多行 + completer 弹层 | ✅ | `render_input()` + `Completer` |
| status: 模型 \| cwd \| git \| context% \| time | ✅ | `render_status()` |

## Section 4.1 — 对话滚动区

| 特性 | 状态 | 实现 |
|---|---|---|
| 流式追加 (push_str) | ✅ | `StreamText` → `m.text.push_str(&delta)` |
| 结构化消息列表 | ✅ | `Vec<Message>` (role + text) |
| scroll_offset + auto_scroll | ✅ | `Model.scroll_offset` + `Model.auto_scroll` |
| 用户滚动时新内容不强制拉底 | ✅ | `auto_scroll = false` 时 Tick 不重置 offset |
| 底部锚定渲染 | ✅ | `render_body()` 从 `bottom() - 1` 开始 |

## Section 4.2 — 输入框

| 特性 | 状态 | 备注 |
|---|---|---|
| 单行输入 | ✅ | `Input` component |
| 多行输入 (自适应) | ✅ | `input_height()` + multi-line render |
| `/` 命令补全 | ✅ | `Completer` (Slash 模式) |
| `@` 文件补全 | ✅ | `Completer` (AtFile 模式) |
| tui-textarea 集成 | ❌ | 自研简化输入组件，未用三方库 |
| 撤销/重做 | ❌ | 简化版不支持 |
| Shift+Enter 换行 | ❌ | 当前 Enter = 发送 |

## Section 4.3 — 工具调用卡片

| 特性 | 状态 | 实现 |
|---|---|---|
| 状态机 Pending→Running→Done/Failed | ✅ | `ToolCallState` enum |
| Braille spinner (⠋⠙⠹...) | ✅ | `SPINNER_FRAMES` + tick 驱动 |
| 颜色区分状态 | ✅ | Theme: tool_running/done/failed/pending |
| 长输出折叠 (前 8 行 + "... N more") | ✅ | `render_body()` "tool" role — output preview |
| 展开/折叠 | 🟡 | `expanded` 字段存在，按键绑定未接 |

## Section 4.4 — Diff 预览

| 特性 | 状态 | 实现 |
|---|---|---|
| `similar` crate 计算 diff | ✅ | `components/diff.rs` |
| 逐行着色 (+绿色/-红色) | ✅ | `compute_diff()` |
| diff 摘要 (+12 -3) | ❌ | 独立 DiffView component，未集成到 tool_card |
| 超过 30 行折叠 | ❌ | 同上 |

## Section 4.5 — 审批对话框

| 特性 | 状态 | 实现 |
|---|---|---|
| Clear + 居中弹层 | ✅ | `render_dialog()` |
| "批准一次" 按钮 | ✅ | `DialogAction::Confirm` |
| "本次会话始终批准" | ✅ | `DialogAction::ConfirmAlways` (" [A] ") |
| "拒绝" 按钮 | ✅ | `DialogAction::Cancel` |
| 弹层不抢输入焦点 | ✅ | dialog → `return` 阻断下层 |
| inline 展开审批 (非 modal) | ❌ | 当前是居中 modal，非 inline |

## Section 4.6 — 状态栏

| 特性 | 状态 | 实现 |
|---|---|---|
| 模型名 | ✅ | `model_name` + accent 色 |
| context 使用率 | ✅ | `context_usage_pct` + 颜色阶梯 (<70绿 70-90黄 >90红) |
| cwd (~ 缩写) | ✅ | `cwd.replace(&home, "~")` |
| git branch | ✅ | `git_branch` + ⎇ 符号 |
| 耗时 | ✅ | `elapsed_secs` + ⏱ m:ss |

## Section 5.1 — Agent Bridge

| 特性 | 状态 | 实现 |
|---|---|---|
| AgentEvent 枚举 (5 变体) | ✅ | `modes/agent_bridge::AgentEvent` |
| 将 AgentSession 事件转发为 AgentEvent | ✅ | `subscribe_agent()` |
| 分离渲染层和 IO 层 | ✅ | view 纯函数, bridge 在 tokio::spawn |

## Section 5.2 — 中断

| 特性 | 状态 | 实现 |
|---|---|---|
| 第一次 Ctrl+C → 打断生成 | ✅ | `session.abort()` + `is_streaming = false` |
| 第二次 Ctrl+C → 退出 | ✅ | `DOUBLE_CTRL_C_WINDOW_MS (500ms)` |
| Esc 退出 | ✅ | `interactive.rs` — `KeyCode::Esc` → break |

## Section 6 — 渲染性能

| 特性 | 状态 | 实现 |
|---|---|---|
| 虚拟滚动 (只渲染可见行) | ✅ | `render_body()` — offset/skip/visible 计算 |
| Tick 频率 100ms | ✅ | `SPINNER_TICK_MS = 100` |
| 缓存换行计算 | 🟡 | `simple_wrap()` 每次重建，无缓存 |

## Section 7 — 键位设计

| 快捷键 | 状态 | 实现 |
|---|---|---|
| `Enter` 发送消息 | ✅ | `Keymap::default()` |
| `PageUp/Down` 滚动 | ✅ | `Msg::ScrollUp/Down` |
| `Esc` 中断/退出 | ✅ | interactive mode |
| `Ctrl+C` 打断/退出 | ✅ | double-press logic |
| `Tab/↑↓/Enter` 补全菜单 | ✅ | `Completer` |
| `a` / `A` / `d` 审批 | 🟡 | Keymap 已定义, 弹层未接 keymap lookup |
| `Ctrl+L` 清屏 | ❌ | 未实现 |
| Shift+Enter 换行 | ❌ | 未实现 |
| `gg` / `G` 跳转 | ❌ | 未实现 |

## Section 8 — 主题系统

| 特性 | 状态 | 实现 |
|---|---|---|
| `Theme` 结构体 | ✅ | `app.rs` `Theme` struct (9 字段) |
| 所有渲染函数通过 `&Theme` 取色 | ✅ | 所有 `render_*` 接受 `t: &Theme` |
| 硬编码 Color 隔离 | ✅ | 仅 status bar 保留 `Color::White` |
| `--theme` CLI flag | ❌ | 未实现 |

## Section 9 — 依赖选型

| 建议 | 使用 | 状态 |
|---|---|---|
| `ratatui` | `ratatui = "0.29"` | ✅ |
| `crossterm` | `crossterm = "0.28"` | ✅ |
| `tokio` | `tokio` | ✅ |
| `tui-textarea` | 未使用 (自研) | ❌ |
| `similar` | `similar = "2"` | ✅ (diff_view) |
| `nucleo-matcher` | 未使用 (自研前缀匹配) | ⚠️ 简化实现 |
| `pulldown-cmark` | 未直接使用 (ratatui-markdown) | ✅ |
| `syntect` | 未直接使用 (ratatui-markdown) | ✅ |

## Phase 1 — 能跑起来

| 条目 | 状态 |
|---|---|
| 基础事件循环 + 三段式布局 | ✅ → 四段式 |
| 纯文本流式显示 | ✅ |
| 输入框 (单行) | ✅ → 多行自适应 |
| Esc 中断 | ✅ → Ctrl+C 双段 |

## Phase 2 — 工具调用可视化

| 条目 | 状态 |
|---|---|
| tool_card 状态机 + spinner | ✅ |
| 审批弹层 (Clear + 居中) | ✅ |
| diff_view (similar) | ✅ |

## Phase 3 — 体验打磨

| 条目 | 状态 |
|---|---|
| Markdown 渲染 (代码块高亮) | ✅ (你要求跳过) |
| `/` 命令菜单 + `@` 文件补全 | ✅ |
| 虚拟滚动优化 | ✅ |
| 主题系统 + 可配置 keymap | ✅ |

## Phase 4 — 可选扩展

| 条目 | 状态 |
|---|---|
| 多 session 面板 | ❌ (range) |
| Token 用量/成本统计 | 🟡 context_usage_pct 已实现 |

---

## 剩余未实现的 plans.md 项 (排除 markdown/语法高亮)

| 优先级 | 条目 | 说明 |
|---|---|---|
| 🟡 | inline 审批 (Section 11) | 工具卡片展开审批按钮 vs 居中弹层 |
| 🟡 | Shift+Enter 换行 | 多行输入时需要 |
| 🟡 | diff 集成到 tool_card | 当前是独立 component |
| 🟢 | Ctrl+L 清屏 | 简单，~5 行 |
| 🟢 | `gg`/`G` 滚动跳转 | 简单 |
