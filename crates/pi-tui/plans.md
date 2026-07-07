# pi-coding-agent TUI 设计文档

参考对象：Codex CLI、Claude Code CLI 的终端交互形态
技术栈：Ratatui + Crossterm + Tokio
定位：为 `pi-coding-agent`（CLI 层，区别于 `pi-agent-core`）设计一个可流式渲染、可中断、可审批工具调用的终端界面

---

## 1. 目标与非目标

**目标**

- 流式展示 LLM 输出（token-by-token 或 chunk-by-chunk），不闪烁、不撕裂
- 展示工具调用（tool call）过程：调用参数 → 执行中 → 结果/diff，并支持人工审批（approve / deny / auto-approve）
- 支持多行输入、历史命令、`/` 斜杠命令、`@` 文件引用补全
- 支持 Esc 中断当前生成（类似 Claude Code 的 "Interrupted" 状态）
- 状态栏常驻展示：当前模型、上下文占用、cwd、git branch、耗时/token 用量
- 可扩展到未来的多 session / 多 agent 面板（当前先做单会话）

**非目标（v1 不做）**

- 不做鼠标交互
- 不做分屏多 session（先预留架构，不实现 UI）
- 不做图片/K线渲染（那是 trending-agent 的需求，这里保持通用 coding agent 场景）

---

## 2. 整体架构

### 2.1 分层

```
┌─────────────────────────────────────────┐
│  main.rs        - 启动/终端初始化/事件循环   │
├─────────────────────────────────────────┤
│  app/           - App 状态机 (State)      │
│    ├─ state.rs  - AppState 枚举与数据      │
│    ├─ update.rs - 消息 -> 状态转换 (reducer)│
│    └─ action.rs - Action/Event 定义       │
├─────────────────────────────────────────┤
│  ui/            - 纯渲染层 (无副作用)       │
│    ├─ layout.rs                          │
│    ├─ widgets/                           │
│    │   ├─ chat_view.rs                   │
│    │   ├─ input_box.rs                   │
│    │   ├─ tool_card.rs                   │
│    │   ├─ diff_view.rs                   │
│    │   ├─ status_bar.rs                  │
│    │   └─ approval_dialog.rs             │
│    └─ theme.rs                           │
├─────────────────────────────────────────┤
│  agent_bridge/  - 与 pi-agent-core 对接    │
│    ├─ stream.rs - 订阅 agent 的流式事件     │
│    └─ commands.rs - 发送用户输入/中断信号    │
└─────────────────────────────────────────┘
```

这个分层遵循你 CLAUDE.md 里的 "data-fetching/inference separation" 原则的 TUI 版本：**渲染层（ui/）永远是纯函数式的 `fn draw(frame, &AppState)`，不做任何异步/IO**；所有异步交互收敛在 `agent_bridge` 和事件循环里。

### 2.2 事件循环（Elm 架构 / TEA 模式）

Codex CLI 和 Claude Code 的 TUI 本质上都是 **TEA（The Elm Architecture）**：`Event -> Action -> State -> View`。建议 pi-coding-agent 也采用这个模式，而不是在渲染代码里直接改状态。

```rust
enum Event {
    Key(KeyEvent),
    Tick,                     // 定时器，驱动 spinner 动画
    AgentStream(AgentEvent),  // 来自 pi-agent-core 的流式事件
    Resize(u16, u16),
}

enum AgentEvent {
    TextDelta(String),
    ToolCallStart { id: String, name: String, args: serde_json::Value },
    ToolCallResult { id: String, result: ToolResult },
    ApprovalRequired { id: String, request: ApprovalRequest },
    TurnFinished { usage: TokenUsage },
    Error(String),
}

async fn run(mut app: App) -> anyhow::Result<()> {
    let mut terminal = init_terminal()?;
    let (tx, mut rx) = mpsc::channel::<Event>(256);

    spawn_input_reader(tx.clone());      // crossterm event stream -> Event::Key
    spawn_tick_timer(tx.clone());        // 每 80ms 发一个 Tick
    spawn_agent_bridge(tx.clone(), app.agent_handle.clone());

    loop {
        terminal.draw(|f| ui::draw(f, &app.state))?;

        match rx.recv().await {
            Some(event) => {
                let action = app.map_event(event);
                if let Some(action) = action {
                    app.update(action); // 纯状态转换，无 IO
                }
            }
            None => break,
        }

        if app.should_quit {
            break;
        }
    }

    restore_terminal(terminal)
}
```

关键点：

- **单一 mpsc channel** 汇聚所有事件源（键盘、tick、agent 流），事件循环串行处理，避免状态竞争
- `update()` 是同步纯函数，方便写单元测试（给定 State + Action，断言新 State）
- IO（发送用户消息给 agent、写文件）通过 `Action` 触发副作用任务，副作用任务再把结果作为新 `Event` 发回 channel，而不是直接在 `update()` 里 await

---

## 3. 布局设计

### 3.1 整体线框图

```
┌ pi-coding-agent ─────────────────────────────────── ● Ready ┐
│                                                              │
│  You                                                         │
│  帮我把 infer_exchange 函数加上北交所支持                        │
│                                                              │
│  ● Assistant                                                 │
│  好的，我先看一下现有实现...                                      │
│                                                              │
│  ┌─ 🔧 read_file ─────────────────────────────────────┐      │
│  │ path: src/exchange/infer.rs                        │      │
│  │ ✓ done (120ms)                                      │      │
│  └──────────────────────────────────────────────────┘      │
│                                                              │
│  ┌─ 🔧 edit_file ─────────────────────────────────────┐      │
│  │ path: src/exchange/infer.rs                        │      │
│  │ --- 3 lines removed, +12 lines added                │      │
│  │  fn infer_exchange(code: &str) -> Exchange {         │      │
│  │ -    match &code[0..2] {                             │      │
│  │ +    match code {                                    │      │
│  │ +        c if c.starts_with("688") => Exchange::Star,│      │
│  │  ...                                                 │      │
│  │                                                       │      │
│  │  [a] Approve   [d] Deny   [v] View full diff         │      │
│  └──────────────────────────────────────────────────┘      │
│                                                              │
├──────────────────────────────────────────────────────────────┤
│ > 在这里输入消息…                                    ⏎ 发送     │
├──────────────────────────────────────────────────────────────┤
│ deepseek-chat │ ctx 42% │ ~/proj/trending-agent │ main │ 1m20s│
└──────────────────────────────────────────────────────────────┘
```

### 3.2 Ratatui 布局代码骨架

```rust
fn draw(f: &mut Frame, state: &AppState) {
    let [header, body, input, status] = Layout::vertical([
        Constraint::Length(1),      // 顶部标题栏
        Constraint::Min(3),         // 对话滚动区
        Constraint::Length(input_height(state)), // 输入框，随内容自适应高度
        Constraint::Length(1),      // 底部状态栏
    ])
    .areas(f.area());

    draw_header(f, header, state);
    draw_chat_view(f, body, state);
    draw_input_box(f, input, state);
    draw_status_bar(f, status, state);

    // 弹层：审批对话框 / 斜杠命令补全菜单，最后画，覆盖在最上层
    if let Some(dialog) = &state.active_dialog {
        draw_dialog_overlay(f, f.area(), dialog);
    }
}
```

要点：

- **输入框高度自适应**（`input_height` 根据当前输入内容的换行数计算，1~10 行封顶），这是 Claude Code TUI 体验比较好的一点：单行时紧凑，多行粘贴代码时自动撑开
- 弹层（审批对话框、`@file` 补全、`/command` 菜单）用 `Clear` widget 先清空区域再画，避免残影：

```rust
use ratatui::widgets::Clear;

fn draw_dialog_overlay(f: &mut Frame, area: Rect, dialog: &Dialog) {
    let popup_area = centered_rect(60, 30, area);
    f.render_widget(Clear, popup_area); // 关键：先清空
    f.render_widget(dialog.widget(), popup_area);
}
```

---

## 4. 核心组件设计

### 4.1 对话滚动区（chat_view）

这是最复杂的组件，因为要处理：**流式追加文本 + 历史滚动 + Markdown 渲染 + 高度变化后的自动滚动**。

**数据结构**：不要直接存 `String`，存结构化的消息列表，渲染时才转成 `Vec<Line>`：

```rust
enum MessageBlock {
    UserText(String),
    AssistantText { content: String, is_streaming: bool },
    ToolCall {
        name: String,
        args_preview: String,
        status: ToolStatus, // Pending | Running | Approved | Denied | Done(ToolResult)
        diff: Option<DiffPreview>,
    },
    SystemNotice(String),
}

struct ChatState {
    blocks: Vec<MessageBlock>,
    scroll_offset: u16,
    auto_scroll: bool, // 用户手动往上滚过就关掉，新消息来了不再强制拉到底部
}
```

**流式追加**：`AssistantText` 收到 `TextDelta` 时是 `push_str` 到最后一个 block，而不是重新分配整个消息列表——避免每个 token 都触发全量 diff。

**Markdown 渲染**：轻量做法是自己写一个简化的 Markdown -> `Vec<Line>` 转换器（处理代码块、粗体、列表、行内 code），不需要引入完整的 pulldown-cmark AST 渲染管线，除非你要支持表格等复杂结构。代码块用等宽 + 边框 + 语言 tag 高亮：

```
┌─ rust ──────────────────────┐
│ fn infer_exchange(code: &str)│
│     -> Exchange {             │
│ ...                           │
└───────────────────────────────┘
```

**自动滚动策略**（这是体验细节，Claude Code 处理得比较好）：

```rust
fn on_new_content(chat: &mut ChatState, viewport_height: u16) {
    let was_at_bottom = chat.scroll_offset + viewport_height >= chat.content_height();
    // ...追加内容...
    if was_at_bottom {
        chat.scroll_offset = chat.content_height().saturating_sub(viewport_height);
    }
    // 如果用户之前往上滚动查看历史，新内容到达不会打断阅读
}
```

### 4.2 输入框（input_box）

建议直接用 **`tui-textarea`** crate 而不是自己写文本编辑逻辑（光标移动、多行换行、选区这些做起来非常繁琐）。它原生支持：

- 多行编辑、软换行
- 撤销/重做
- Vim-like 快捷键（可以配合你已有的 Zed Vim mode 使用习惯）

```rust
use tui_textarea::{TextArea, Input, Key};

// 处理输入事件
match app.mode {
    InputMode::Normal => match key {
        KeyEvent { code: KeyCode::Char('i'), .. } => app.mode = InputMode::Insert,
        KeyEvent { code: KeyCode::Enter, .. } => app.submit_message(),
        KeyEvent { code: KeyCode::Char('/'), .. } => app.open_slash_menu(),
        _ => {}
    },
    InputMode::Insert => {
        if key.code == KeyCode::Esc {
            app.mode = InputMode::Normal;
        } else {
            app.textarea.input(Input::from(key));
        }
    }
}
```

**`/` 斜杠命令与 `@` 文件补全**：检测输入框光标前的 token，触发一个悬浮补全菜单（复用 4.5 的弹层机制），过滤逻辑用简单的子串/模糊匹配（可选 `nucleo-matcher` crate，Helix/fzf 同款模糊匹配算法，效果和性能都不错）。

### 4.3 工具调用卡片（tool_card）

工具调用是 coding agent TUI 的核心差异化部分。状态机：

```
Pending（等待审批）→ Running（执行中，显示 spinner）→ Done / Denied / Error
```

渲染要点：

- 用 `Borders::ALL` + 不同颜色区分状态（灰色=pending，黄色=running，绿色=done，红色=denied/error）
- Running 状态下的 spinner 用 tick 事件驱动一个字符序列：`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`（braille spinner，你在 trending-agent 里已经用过 braille 相关的东西了，这里可以复用同一套动画帧数组）
- 长输出（比如 `read_file` 读了 500 行）默认折叠只显示前 N 行 + `... 还有 480 行，按 [Enter] 展开`，避免刷屏

### 4.4 Diff 预览（diff_view）

对 `edit_file` / `write_file` 类工具调用，展示 unified diff，逐行着色：

```rust
fn render_diff_line(line: &str) -> Line<'static> {
    let (style, prefix) = match line.chars().next() {
        Some('+') => (Style::new().fg(Color::Green), ""),
        Some('-') => (Style::new().fg(Color::Red), ""),
        _ => (Style::new().fg(Color::DarkGray), ""),
    };
    Line::styled(format!("{prefix}{line}"), style)
}
```

diff 计算本身用 `similar` crate（比 `difference` 或手写 LCS 更快更准确，支持 unified diff 格式输出），不要自己实现 diff 算法。

超过一定行数（比如 30 行）默认只展示 diff 摘要（`+12 -3`）+ 一个 `[v] 查看完整 diff` 快捷键，展开后进入一个全屏 diff pager（复用 chat_view 的滚动逻辑）。

### 4.5 审批对话框（approval_dialog）

这是 coding agent 区别于普通 chatbot 的关键交互：工具执行前需要人工确认（除非用户开了 auto-approve）。

```
┌─ 需要确认 ─────────────────────────────┐
│                                        │
│  Agent 想要执行:                        │
│  edit_file(src/exchange/infer.rs)      │
│                                        │
│  [a] 批准一次   [A] 本次会话内始终批准    │
│  [d] 拒绝       [Esc] 取消              │
│                                        │
└────────────────────────────────────────┘
```

设计原则：

- **默认拒绝危险操作**（删除文件、执行 shell 命令），需要显式按键，不能靠 Enter 误触发批准
- 提供 "本次会话内始终批准此类工具" 的选项，减少高频交互摩擦（这也是 Claude Code 的实际做法）
- 审批请求到达时，如果输入框正在被用户打字，**不要抢焦点清空输入内容**——弹层浮在上层，回车键在弹层打开期间被弹层拦截，关闭后原输入内容保留

### 4.6 状态栏（status_bar）

单行，右对齐分段展示，用 `│` 分隔：

```
model │ context 使用率 │ cwd（相对路径，过长时省略中间） │ git branch │ 用时/token
```

context 使用率建议做成简易进度条或者用颜色阶梯提示（<70% 白色，70-90% 黄色，>90% 红色），提前提示用户快要触发压缩/截断。

---

## 5. 状态管理与数据流细节

### 5.1 与 pi-agent-core 的桥接

`pi-coding-agent`（CLI 层）和 `pi-agent-core`（推理核心）按你之前的探索是分离的，TUI 侧只应该依赖 `pi-agent-core` 暴露的一个流式接口，不应该感知内部 ReAct 循环细节：

```rust
// agent_bridge/stream.rs
trait AgentSession: Send {
    fn send_user_message(&self, text: String) -> BoxFuture<'_, Result<()>>;
    fn interrupt(&self);
    fn subscribe(&self) -> mpsc::Receiver<AgentEvent>;
}
```

TUI 只依赖这个 trait，方便：

1. 单测时用 mock `AgentSession` 驱动固定的事件序列，验证 UI 状态机而不需要真实跑一次 LLM 请求
2. 未来如果要支持多 session 面板，只需要维护多个 `Box<dyn AgentSession>`

### 5.2 中断（Esc 处理）

Codex CLI / Claude Code 的 Esc 中断有两级语义，建议照做：

- 第一次按 Esc：中断当前生成中的回复（发送 cancel 信号给 agent，UI 立刻把 streaming block 标记为 `interrupted`）
- 连续两次按 Esc（比如 500ms 内）：清空当前输入框内容（类似 shell 里 Ctrl-C 两次的手感）

```rust
enum EscAction { InterruptGeneration, ClearInput, Nothing }

fn handle_esc(app: &mut App) -> EscAction {
    let now = Instant::now();
    if app.is_generating {
        app.request_interrupt();
        EscAction::InterruptGeneration
    } else if now.duration_since(app.last_esc_at) < Duration::from_millis(500) {
        app.textarea = TextArea::default();
        EscAction::ClearInput
    } else {
        app.last_esc_at = now;
        EscAction::Nothing
    }
}
```

---

## 6. 渲染性能

Ratatui 每帧是全量 diff 后只写变化的 cell，本身已经做了这层优化，但应用层还有几个坑要注意：

1. **不要每个 tick 都重建整个 `Vec<Line>`**：Markdown -> Line 的转换结果应该缓存，只有对应 block 的内容变化时才重新转换（可以给每个 `MessageBlock` 加一个 dirty flag 或者简单地用内容 hash 判断）
2. **Tick 频率**：spinner 动画用 80~120ms 一次 tick 足够，不需要 16ms（60fps）刷新，coding agent 场景不需要那么高帧率，省 CPU
3. **大量历史消息的滚动**：长会话可能有几百个 block，全部转换成 `Line` 再交给 `Paragraph` 会有性能问题。建议做**虚拟滚动**：只对当前 viewport 附近的 block 做渲染转换，其余的只存文本不做样式计算
4. **终端 resize**：`Resize` 事件到达时清一次缓存的换行计算（因为 wrap 宽度变了，之前缓存的行数不准）

---

## 7. 键位设计（建议）

| 场景 | 按键 | 行为 |
|---|---|---|
| Normal 模式 | `i` / `a` | 进入输入模式 |
| Normal 模式 | `j`/`k` 或 `↑`/`↓` | 滚动对话历史 |
| Normal 模式 | `g g` / `G` | 跳到顶部/底部 |
| Insert 模式 | `Enter` | 发送消息 |
| Insert 模式 | `Shift+Enter` / `Alt+Enter` | 插入换行不发送 |
| 任意模式 | `Esc` | 中断生成 / 二次按清空输入 |
| 任意模式 | `Ctrl+C` | 退出程序（需二次确认或 500ms 内两次） |
| 审批弹层 | `a` / `A` / `d` | 批准一次 / 本会话始终批准 / 拒绝 |
| 补全菜单 | `Tab` / `↓↑` / `Enter` | 选择补全项 |
| 全局 | `Ctrl+L` | 清屏（保留 scrollback，仅清空可见区域，类似 shell） |

---

## 8. 主题系统

```rust
struct Theme {
    user_msg: Style,
    assistant_msg: Style,
    tool_pending: Style,
    tool_running: Style,
    tool_done: Style,
    tool_error: Style,
    diff_add: Style,
    diff_del: Style,
    status_bar_bg: Color,
    accent: Color,
}
```

用 CSS 变量式的思路（虽然是终端）：所有 widget 渲染时只从 `Theme` 取颜色，不要硬编码 `Color::Red` 散落在各个 widget 文件里。方便以后加 `--theme dark|light|dracula` 这类 CLI flag。

---

## 9. 依赖选型（Cargo.toml 建议）

| 用途 | crate | 备注 |
|---|---|---|
| TUI 框架 | `ratatui` | |
| 终端后端 | `crossterm` | 跨平台，Windows 也能跑 |
| 异步运行时 | `tokio` | 已在用 |
| 多行文本编辑 | `tui-textarea` | 省去自己写光标/选区逻辑 |
| Diff 计算 | `similar` | unified diff 输出，比手写 LCS 靠谱 |
| 模糊匹配（补全） | `nucleo-matcher` | Helix 同款，性能好 |
| Markdown 简化解析 | 自研 或 `pulldown-cmark`（仅取事件流，不用它的 HTML renderer） | 自研更可控，代码量也不大 |
| 语法高亮（代码块） | `syntect` | 如果要高亮 diff/代码块里的语言；注意加载 syntax set 有一次性开销，启动时预加载 |

---

## 10. 分阶段实现路线图

**Phase 1 — 能跑起来**
- 基础事件循环 + 三段式布局（header/body/input）
- 纯文本流式显示（不做 Markdown，先用 `Paragraph` 顶上）
- 输入框（先不用 tui-textarea，单行够用）
- Esc 中断

**Phase 2 — 工具调用可视化**
- tool_card 状态机 + spinner
- 审批弹层（Clear + 居中弹层机制）
- diff_view（接入 `similar`）

**Phase 3 — 体验打磨**
- Markdown 渲染（代码块高亮）
- `/` 命令菜单、`@` 文件补全（nucleo-matcher）
- 虚拟滚动优化
- 主题系统 + 可配置 keymap

**Phase 4 — 可选扩展**
- 多 session 面板（左侧 session 列表 + 右侧当前对话，类似 Codex 的多任务视图）
- Token 用量/成本的实时统计面板

---

## 11. 与 Claude Code / Codex CLI 的关键差异点提示

如果你要参考这两者的实际手感，有几个细节值得注意：

- **Claude Code** 的审批弹层是"就地展开"而不是模态弹窗盖住全屏——工具卡片本身展开出批准按钮，上下文不丢失，这个体验比全屏 modal 更好，建议 pi-coding-agent 优先做这种"inline 展开"而不是居中弹层（4.5 节的设计可以改成这种 inline 展开模式）
- **Codex CLI** 在长时间运行的工具（比如跑测试）上会显示实时 stdout 流（滚动的日志窗口），这需要 tool_card 支持一个"可展开的实时日志区域"，而不只是静态的 pending/done 状态
- 两者都对**首屏加载体感**很敏感——终端初始化、agent session 建立要在 100ms 内给出可感知反馈（哪怕只是一个 "Starting..." 提示），避免用户以为程序卡住了
