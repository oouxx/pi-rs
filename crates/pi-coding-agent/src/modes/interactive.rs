//! Interactive TUI mode — Elm architecture with Action/Effect dispatch.
//!
//! Layer layout mirrors grok-build's pager (`app/event_loop.rs` +
//! `app/dispatch/` + `app/effects.rs`):
//!
//! ```text
//! event sources (keyboard / agent events / tick)
//!      │  Event → [`Action`]
//!      ▼
//! [`update`]  — pure: Action → state mutation + [`Effect`]s
//!      │
//!      ▼
//! [`execute_effect`] — async side effects (agent commands, abort)
//! ```
//!
//! `update` never performs I/O: it only mutates `AppState` (which owns the
//! pi-tui Elm `Model`) and returns a list of `Effect`s for the event loop to
//! run. The event loop is the only place that touches channels / the agent.

use std::sync::Arc;
use std::time::Instant;

use pi_agent_core::agent::Agent;
use pi_tui::app;
use tokio::sync::Mutex;

use crate::core::agent_session::AgentSession;
use crate::core::extensions::ExtensionUIContext;

const DOUBLE_CTRL_C_WINDOW_MS: u64 = 500;
const SPINNER_TICK_MS: u64 = 100;

// ============================================================================
// AgentCmd — commands that need the session mutex (executed by the
// background task, which holds the lock across an agent run)
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum AgentCmd {
    SendMessage(String),
    AbortBash,
    SetModel(String, String),
    CycleModel(String),
    CycleThinkingLevel,
    NewSession(Option<String>),
    SetSessionName(String),
    ExtensionCommand(String, String),
    ReloadExtensions,
    /// Esc 中断后：把排队 steering/follow-up 文本（+ 当前编辑器文本）还原
    /// 回编辑器（TS `restoreQueuedMessagesToEditor` 的编辑器部分）。参数为
    /// Esc 按下时输入框里的文本。
    RestoreQueuedToEditor(String),
    /// `/copy`：复制最后一条 assistant 消息到系统剪贴板（TS
    /// `handleCopyCommand`）。
    CopyLastMessage,
}

// ============================================================================
// Action — everything the event loop observes, normalized to one enum
// ============================================================================

enum Action {
    /// A pressed key (release events are filtered before dispatch).
    Key(crossterm::event::KeyEvent),
    /// Agent events already converted to pi-tui messages by the bridge.
    Agent(pi_tui::Msg),
    /// Extension UI bridge requests (notify/dialogs/editor text).
    Ui(UiAction),
    /// Spinner/streaming tick.
    Tick,
}

// ============================================================================
// Effect — side effects requested by `update`, executed by the event loop
// ============================================================================

// Effect 是内部私有枚举：derive Debug/PartialEq 仅为测试断言服务，
// 无行为影响。
#[derive(Debug, PartialEq)]
enum Effect {
    /// Send a command to the agent background task (session-locked).
    AgentCommand(AgentCmd),
    /// Abort the running agent directly (never through the session mutex —
    /// the lock is held for the whole run by `add_user_text`).
    Abort,
    /// Esc 中断（参数为按下时编辑器里已有的文本）：先清掉 agent 队列再中止
    /// run，随后把排队 steering/follow-up 文本还原回编辑器（对齐 TS
    /// `restoreQueuedMessagesToEditor({ abort: true })`）。
    AbortAndClearQueues(String),
    /// 异步计算补全候选（slash fuzzy / 命令参数 / `@` 文件走查）。
    RequestCompletion(pi_tui::CompletionRequest),
}

// ============================================================================
// Extension UI bridge — requests from extensions rendered by the TUI
// ============================================================================

/// Requests the extension UI bridge sends to the event loop.
enum UiAction {
    Notify(String),
    SetStatus(String, String),
    /// Synchronous confirm: the reply uses a std channel because the
    /// `confirm` closure is a plain `Fn` (it blocks its worker thread with
    /// `recv_timeout`; the TUI event loop runs on another worker).
    Confirm {
        title: String,
        message: String,
        reply: std::sync::mpsc::Sender<bool>,
    },
    Select {
        title: String,
        options: Vec<String>,
        reply: tokio::sync::oneshot::Sender<Option<String>>,
    },
    Input {
        title: String,
        initial: String,
        reply: tokio::sync::oneshot::Sender<Option<String>>,
    },
    SetEditorText(String),
}

/// An extension dialog the TUI is waiting for the user to answer.
enum PendingUi {
    Confirm { reply: std::sync::mpsc::Sender<bool> },
    Select { reply: tokio::sync::oneshot::Sender<Option<String>> },
    Input { reply: tokio::sync::oneshot::Sender<Option<String>> },
}

/// `ui.select` handler shape: title, options, payload → user choice.
type SelectFn = std::sync::Arc<
    dyn Fn(
            &str,
            &[String],
            Option<&serde_json::Value>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<String>> + Send + 'static>,
        > + Send
        + Sync,
>;

/// `ui.set_widget` handler shape (inline widgets are not rendered by the
/// minimal TUI — no-op bridge).
type WidgetFn = std::sync::Arc<
    dyn Fn(&str, Option<&[String]>, Option<&serde_json::Value>) + Send + Sync,
>;

/// `ui.input` handler shape: title, initial text, payload → user text.
type InputFn = std::sync::Arc<
    dyn Fn(
            &str,
            Option<&str>,
            Option<&serde_json::Value>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<String>> + Send + 'static>,
        > + Send
        + Sync,
>;

/// Build the extension UI context wired to a TUI event stream.
/// Fire-and-forget calls (notify/status/editor text) become messages on the
/// stream; dialogs block until the user answers in the TUI.
fn create_tui_ui_context() -> (
    ExtensionUIContext,
    tokio::sync::mpsc::UnboundedSender<UiAction>,
    tokio::sync::mpsc::UnboundedReceiver<UiAction>,
) {
    use std::sync::Arc;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<UiAction>();

    let notify = {
        let tx = tx.clone();
        Arc::new(move |title: &str, _payload: &serde_json::Value| {
            let _ = tx.send(UiAction::Notify(title.to_string()));
        })
    };
    let set_status = {
        let tx = tx.clone();
        Arc::new(move |status: &str, message: &str| {
            let _ = tx.send(UiAction::SetStatus(status.to_string(), message.to_string()));
        })
    };
    let confirm = {
        let tx = tx.clone();
        Arc::new(move |title: &str, message: &serde_json::Value| {
            let (reply, rx) = std::sync::mpsc::channel::<bool>();
            let _ = tx.send(UiAction::Confirm {
                title: title.to_string(),
                message: message.to_string(),
                reply,
            });
            // Blocking wait (plain Fn, no async): the TUI event loop runs on
            // another worker thread and resolves the dialog. Times out to
            // false so a stuck dialog cannot wedge the extension forever.
            rx.recv_timeout(std::time::Duration::from_secs(120)).unwrap_or(false)
        })
    };
    let select = {
        let tx = tx.clone();
        let select: SelectFn = std::sync::Arc::new(
            move |title: &str,
                  options: &[String],
                  _opts: Option<&serde_json::Value>| {
                let tx = tx.clone();
                let title = title.to_string();
                let options = options.to_vec();
                Box::pin(async move {
                    let (reply, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(UiAction::Select {
                        title,
                        options,
                        reply,
                    });
                    tokio::time::timeout(std::time::Duration::from_secs(120), rx)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .flatten()
                })
            },
        );
        select
    };
    let input = {
        let tx = tx.clone();
        let input: InputFn = std::sync::Arc::new(
            move |title: &str,
                  initial: Option<&str>,
                  _opts: Option<&serde_json::Value>| {
                let tx = tx.clone();
                let title = title.to_string();
                let initial = initial.unwrap_or("").to_string();
                Box::pin(async move {
                    let (reply, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(UiAction::Input {
                        title,
                        initial,
                        reply,
                    });
                    tokio::time::timeout(std::time::Duration::from_secs(120), rx)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .flatten()
                })
            },
        );
        input
    };
    // Inline widgets are not rendered by the minimal TUI (deviation).
    let set_widget: WidgetFn = std::sync::Arc::new(|_, _, _| {});
    let set_title = Arc::new(|title: &str| {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::SetTitle(title));
    });
    let set_editor_text = {
        let tx = tx.clone();
        Arc::new(move |text: &str| {
            let _ = tx.send(UiAction::SetEditorText(text.to_string()));
        })
    };

    (
        ExtensionUIContext {
            notify,
            set_status,
            confirm,
            select,
            input,
            set_widget,
            set_title,
            set_editor_text,
        },
        tx,
        rx,
    )
}

// ============================================================================
// AppState — shared state (owns the pi-tui model)
// ============================================================================

struct AppState {
    model: pi_tui::Model,
    last_ctrl_c: Instant,
    /// TS `lastEscapeTime`：空编辑器双击 Esc（500ms 内）触发
    /// double-escape action（TS 默认弹 tree 选择器；本 port 未实现，
    /// 见 DEVIATIONS）。
    last_esc: Instant,
    /// app::update 产生的 pi-tui Cmd（当前只有补全请求），由 update 收集
    /// 成 Effect 交给事件循环执行。
    pending_cmds: Vec<pi_tui::Cmd>,
    quit: bool,
    /// Names of slash commands registered by extensions (snapshot taken at
    /// startup; used to route `/name` to the extension executor).
    ext_commands: Vec<String>,
    /// Extension dialog awaiting a user answer (at most one at a time).
    pending_ui: Option<PendingUi>,
    /// When the current streaming/working run started (for the status-bar
    /// elapsed timer).
    stream_started_at: Option<Instant>,
    /// Last time the session-derived status bar (context usage) was queried.
    last_status_refresh: Instant,
    /// 行级差分渲染器（对齐 TS TuiAltScreen.doRender）：整行比较、整行
    /// 重写，首帧/尺寸变化时全量清屏重绘。
    line_screen: pi_tui::line_screen::LineScreen,
}

impl AppState {
    fn new(width: u16, height: u16, ext_commands: Vec<String>) -> Self {
        let model = pi_tui::Model::new(width, height);
        Self {
            model,
            last_ctrl_c: Instant::now() - std::time::Duration::from_millis(DOUBLE_CTRL_C_WINDOW_MS + 100),
            last_esc: Instant::now() - std::time::Duration::from_millis(DOUBLE_CTRL_C_WINDOW_MS + 100),
            pending_cmds: Vec::new(),
            quit: false,
            ext_commands,
            pending_ui: None,
            stream_started_at: None,
            last_status_refresh: Instant::now() - std::time::Duration::from_secs(10),
            line_screen: pi_tui::line_screen::LineScreen::new(),
        }
    }
}

// ============================================================================
// update — pure: Action → (state mutation, effects)
// ============================================================================

/// Outcome of one [`update`] pass.
struct UpdateOutcome {
    effects: Vec<Effect>,
    /// Whether the view changed and needs a redraw.
    redraw: bool,
}

fn update(state: &mut AppState, action: Action) -> UpdateOutcome {
    match action {
        Action::Tick => {
            // The spinner only runs while the agent is working: streaming or
            // a tool call is in flight (finished tool rows stay in the
            // transcript but the agent is idle).
            let active = state.model.is_streaming
                || state
                    .model
                    .active_tools
                    .iter()
                    .any(|t| matches!(t.state, pi_tui::app::ToolCallState::Running | pi_tui::app::ToolCallState::Pending));
            if active {
                app::update(&mut state.model, pi_tui::Msg::Tick);
            }
            UpdateOutcome { effects: vec![], redraw: active }
        }
        Action::Agent(msg) => {
            app::update(&mut state.model, msg);
            UpdateOutcome { effects: vec![], redraw: true }
        }
        Action::Key(key) => {
            let mut effects = handle_key(state, key);
            // Ctrl+C 连按两次 / Ctrl+D（空输入）→ app.rs 返回 Cmd::Quit，
            // 这里置退出标志（对齐 TS `handleCtrlC`/`handleCtrlD`）。
            let mut quit = false;
            effects.extend(state.pending_cmds.drain(..).filter_map(|cmd| match cmd {
                pi_tui::Cmd::RequestCompletion(req) => Some(Effect::RequestCompletion(req)),
                // Ctrl+X（TS `app.message.copy`）→ 和 `/copy` 同一个 agent
                // 任务路径（读最后一条 assistant 消息 + 写系统剪贴板）。
                pi_tui::Cmd::CopyLastMessage => {
                    Some(Effect::AgentCommand(AgentCmd::CopyLastMessage))
                }
                pi_tui::Cmd::Quit => {
                    quit = true;
                    None
                }
            }));
            if quit {
                state.quit = true;
            }
            UpdateOutcome { effects, redraw: true }
        }
        Action::Ui(action) => {
            handle_ui_action(state, action);
            UpdateOutcome { effects: vec![], redraw: true }
        }
    }
}

/// Handle an extension UI request: notifications become system messages,
/// dialogs become TUI overlays whose answer resolves the pending request.
fn handle_ui_action(state: &mut AppState, action: UiAction) {
    match action {
        UiAction::Notify(text) => {
            app::update(&mut state.model, pi_tui::Msg::NewMessage("system".into(), text));
        }
        UiAction::SetStatus(status, message) => {
            app::update(
                &mut state.model,
                pi_tui::Msg::NewMessage("system".into(), format!("[{status}] {message}")),
            );
        }
        UiAction::SetEditorText(text) => {
            app::update(&mut state.model, pi_tui::Msg::SetEditorText(text));
        }
        UiAction::Confirm { title, message, reply } => {
            if state.pending_ui.is_some() {
                let _ = reply.send(false);
                return;
            }
            state.pending_ui = Some(PendingUi::Confirm { reply });
            app::update(
                &mut state.model,
                pi_tui::Msg::ShowDialog(pi_tui::Dialog {
                    title,
                    message,
                    buttons: vec![
                        pi_tui::DialogButton { label: "Confirm", action: pi_tui::DialogAction::Confirm },
                        pi_tui::DialogButton { label: "Cancel", action: pi_tui::DialogAction::Cancel },
                    ],
                    selected: 0,
                }),
            );
        }
        UiAction::Select { title, options, reply } => {
            if state.pending_ui.is_some() {
                let _ = reply.send(None);
                return;
            }
            state.pending_ui = Some(PendingUi::Select { reply });
            state.model.mode = pi_tui::AppMode::Select {
                list: pi_tui::SelectList::new(options).with_title(&title),
            };
        }
        UiAction::Input { title, initial, reply } => {
            if state.pending_ui.is_some() {
                let _ = reply.send(None);
                return;
            }
            state.pending_ui = Some(PendingUi::Input { reply });
            state.model.mode = pi_tui::AppMode::Editor {
                editor: Box::new(pi_tui::Editor::new(&initial)),
                title,
            };
        }
    }
}

/// Key dispatch — pure: mutates model/state and returns effects.
/// Mirrors the key handling from TS `interactive-mode.ts` (Ctrl+C abort with
/// double-press quit, Ctrl+L clear, Ctrl+D quit, Esc interrupt, Ctrl+P/T/B
/// commands, Enter submits, slash commands).
fn handle_key(state: &mut AppState, key: crossterm::event::KeyEvent) -> Vec<Effect> {
    use crossterm::event::{KeyCode, KeyModifiers};

    // Modal overlays own the keyboard while visible (extension dialogs).
    if state.model.dialog.is_some() {
        return handle_dialog_key(state, key);
    }
    match &state.model.mode {
        pi_tui::AppMode::Select { .. } => return handle_select_key(state, key),
        pi_tui::AppMode::Editor { .. } => return handle_editor_key(state, key),
        pi_tui::AppMode::Chat => {}
    }

    match key.code {
        // Ctrl+C: abort if streaming, double Ctrl+C within the window quits
        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
            let now = Instant::now();
            let elapsed = now.duration_since(state.last_ctrl_c).as_millis() as u64;
            state.last_ctrl_c = now;
            if elapsed < DOUBLE_CTRL_C_WINDOW_MS {
                state.quit = true;
                return vec![];
            }
            if state.model.is_streaming || !state.model.active_tools.is_empty() {
                // Immediate UI feedback; the agent event stream then closes
                // via Done → MessageEnd → StreamEnd.
                state.model.is_streaming = false;
                vec![Effect::Abort]
            } else {
                vec![]
            }
        }
        // Ctrl+L: clear screen
        KeyCode::Char('l') if key.modifiers == KeyModifiers::CONTROL => {
            app::update(&mut state.model, pi_tui::Msg::ClearScreen);
            vec![]
        }
        // Ctrl+D: quit
        KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
            state.quit = true;
            vec![]
        }
        // Ctrl+P: cycle model (matching original Ctrl+P behavior)
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            vec![Effect::AgentCommand(AgentCmd::CycleModel("next".into()))]
        }
        // Ctrl+T: cycle thinking level
        KeyCode::Char('t') if key.modifiers == KeyModifiers::CONTROL => {
            vec![Effect::AgentCommand(AgentCmd::CycleThinkingLevel)]
        }
        // Ctrl+B: abort bash
        KeyCode::Char('b') if key.modifiers == KeyModifiers::CONTROL => {
            vec![Effect::AgentCommand(AgentCmd::AbortBash)]
        }
        // Esc: 对齐 TS `onEscape`（interactive-mode.ts setupKeyHandlers）——
        // **不退出**（退出是 Ctrl+D / 双击 Ctrl+C / /quit）：
        // 1) 流式/运行中 → 中断 + 清排队 follow-up（TS restoreQueuedMessagesToEditor）
        // 2) bash 运行中 → abort bash
        // 3) 空编辑器双击 Esc → TS 弹 tree/fork 选择器（本 port 未实现，见
        //    DEVIATIONS）→ 无操作，仅记录时间。
        KeyCode::Esc => {
            // 补全弹窗打开时 Esc 只关闭弹窗（app.rs Completer 语义），不
            // 触发 onEscape 链。
            if state.model.completer.visible {
                state.model.completer.deactivate();
                return vec![];
            }
            let now = Instant::now();
            let elapsed = now.duration_since(state.last_esc).as_millis() as u64;
            state.last_esc = now;
            if state.model.is_streaming || !state.model.active_tools.is_empty() {
                // Immediate UI feedback; the agent event stream then closes
                // via Done → MessageEnd → StreamEnd.
                state.model.is_streaming = false;
                // 当前编辑器文本随 effect 带出，用于还原时与队列文本合并
                // （TS `[queuedText, currentText].join("\n\n")`）。
                let current = state.model.input.value().to_string();
                vec![Effect::AbortAndClearQueues(current)]
            } else if state.model.active_tools.iter().any(|t| {
                t.name == "bash"
                    && matches!(t.state, pi_tui::app::ToolCallState::Running)
            }) {
                vec![Effect::AgentCommand(AgentCmd::AbortBash)]
            } else {
                // 空编辑器双击 Esc（500ms 内）：TS 默认 "tree" 选择器；本
                // port 未实现 → no-op（不退出、不清编辑器）。
                let _ = elapsed;
                vec![]
            }
        }
        // Enter: submit message or slash command (Shift/Alt+Enter insert a
        // newline instead — matches the TS editor's multi-line behavior).
        // While the slash-command menu is open, Enter picks the highlighted
        // completion and runs it (TS dropdown behavior).
        KeyCode::Enter => {
            if key.modifiers == KeyModifiers::SHIFT || key.modifiers == KeyModifiers::ALT {
                app::update(&mut state.model, pi_tui::Msg::InputNewline);
                return vec![];
            }
            if state.model.completer.visible {
                let trigger = state.model.completer.trigger;
                // 只应用"与当前输入一致"的新鲜结果（对齐 TS：Enter 应用的是
                // 当前文本对应的列表；过期/空结果视为无补全，按普通输入提交）。
                if state.model.completer.has_fresh_results() {
                    // 对齐 TS applyCompletion：替换整段前缀（slash 命令补全后
                    // 加空格；`@` 文件目录不加空格、文件加空格；参数不加）。
                    if let Some(new_value) = state
                        .model
                        .completer
                        .apply_selected(state.model.input.value())
                    {
                        state.model.input.set_value(&new_value);
                    }
                    state.model.completer.deactivate();
                    // 对齐 TS：仅 `/` 补全应用后继续走（提交执行命令）；
                    // `@`/文件补全应用后停留编辑，不提交消息。
                    if trigger != Some(pi_tui::components::CompletionTrigger::Slash) {
                        return vec![];
                    }
                } else {
                    // 过期/空结果：视为没有补全，正常提交当前输入。
                    state.model.completer.deactivate();
                }
            }
            let text = state.model.input.value().to_string();
            if text.is_empty() {
                return vec![];
            }
            if text.starts_with('/') {
                // Slash commands are pure too: they push system messages and
                // return the corresponding agent effect.
                return slash_command(state, &text);
            }
            state.model.input.clear();
            state.model.push_message("user", text.clone());
            state.model.is_streaming = true;
            vec![Effect::AgentCommand(AgentCmd::SendMessage(text))]
        }
        _ => {
            let cmds = app::update(&mut state.model, pi_tui::Msg::Key(key));
            state.pending_cmds.extend(cmds);
            vec![]
        }
    }
}

/// Resolve the pending extension dialog with a string answer (select/input).
fn resolve_pending_ui_text(state: &mut AppState, value: Option<String>) {
    if let Some(pending) = state.pending_ui.take() {
        match pending {
            PendingUi::Select { reply } => {
                let _ = reply.send(value);
            }
            PendingUi::Input { reply } => {
                let _ = reply.send(value);
            }
            PendingUi::Confirm { .. } => {}
        }
    }
}

/// Resolve the pending extension dialog with a boolean answer (confirm).
fn resolve_pending_ui_bool(state: &mut AppState, value: bool) {
    if let Some(pending) = state.pending_ui.take() {
        match pending {
            PendingUi::Confirm { reply } => {
                let _ = reply.send(value);
            }
            PendingUi::Select { .. } | PendingUi::Input { .. } => {}
        }
    }
}

/// Keys while an extension confirm dialog is visible.
fn handle_dialog_key(state: &mut AppState, key: crossterm::event::KeyEvent) -> Vec<Effect> {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Tab | KeyCode::Right => {
            app::update(&mut state.model, pi_tui::Msg::DialogNext);
        }
        KeyCode::Left => {
            app::update(&mut state.model, pi_tui::Msg::DialogPrev);
        }
        KeyCode::Enter => {
            let confirmed = state
                .model
                .dialog
                .as_ref()
                .and_then(|d| d.buttons.get(d.selected))
                .is_some_and(|b| matches!(b.action, pi_tui::DialogAction::Confirm | pi_tui::DialogAction::ConfirmAlways));
            app::update(&mut state.model, pi_tui::Msg::DialogConfirm);
            resolve_pending_ui_bool(state, confirmed);
        }
        KeyCode::Esc => {
            app::update(&mut state.model, pi_tui::Msg::DismissDialog);
            resolve_pending_ui_bool(state, false);
        }
        _ => {}
    }
    vec![]
}

/// Keys while an extension select dialog is visible.
fn handle_select_key(state: &mut AppState, key: crossterm::event::KeyEvent) -> Vec<Effect> {
    use crossterm::event::KeyCode;
    match key.code {
        KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k')
        | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown => {
            if let pi_tui::AppMode::Select { list } = &mut state.model.mode {
                list.handle_key(&key);
            }
        }
        KeyCode::Enter => {
            let selected = if let pi_tui::AppMode::Select { list } = &state.model.mode {
                list.selected_item().map(ToString::to_string)
            } else {
                None
            };
            app::update(&mut state.model, pi_tui::Msg::ExitSelect);
            resolve_pending_ui_text(state, selected);
        }
        KeyCode::Esc => {
            app::update(&mut state.model, pi_tui::Msg::ExitSelect);
            resolve_pending_ui_text(state, None);
        }
        _ => {}
    }
    vec![]
}

/// Keys while the extension input dialog (editor) is visible:
/// Ctrl+S submits, Esc cancels, everything else edits.
fn handle_editor_key(state: &mut AppState, key: crossterm::event::KeyEvent) -> Vec<Effect> {
    use crossterm::event::{KeyCode, KeyModifiers};
    match key.code {
        KeyCode::Char('s') if key.modifiers == KeyModifiers::CONTROL => {
            let text = if let pi_tui::AppMode::Editor { editor, .. } = &state.model.mode {
                editor.text()
            } else {
                String::new()
            };
            app::update(&mut state.model, pi_tui::Msg::EditorDone(String::new()));
            resolve_pending_ui_text(state, Some(text));
        }
        KeyCode::Esc => {
            app::update(&mut state.model, pi_tui::Msg::EditorDone(String::new()));
            resolve_pending_ui_text(state, None);
        }
        _ => {
            if let pi_tui::AppMode::Editor { editor, .. } = &mut state.model.mode {
                editor.handle_key(&key);
            }
        }
    }
    vec![]
}

/// Slash-command handling (pure, mirrors `interactive-mode.ts`).
fn slash_command(state: &mut AppState, text: &str) -> Vec<Effect> {
    // Slash commands consume the input (matching the original: the prompt is
    // cleared once a command is dispatched).
    state.model.input.clear();
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let command = parts[0];
    let args = parts.get(1).copied().unwrap_or("");

    let system = |state: &mut AppState, msg: String| {
        state.model.push_message("system", msg);
    };

    match command {
        "/new" => {
            // The session manager is swapped to a fresh session; clear the
            // transcript so the TUI matches the new session (TS `/new`
            // clears the chat).
            app::update(&mut state.model, pi_tui::Msg::ClearScreen);
            state.model.is_streaming = false;
            let parent = if args.is_empty() { None } else { Some(args.to_string()) };
            system(state, "New session created".into());
            vec![Effect::AgentCommand(AgentCmd::NewSession(parent))]
        }
        "/name" => {
            if !args.is_empty() {
                system(state, format!("Session name set to: {args}"));
                vec![Effect::AgentCommand(AgentCmd::SetSessionName(args.to_string()))]
            } else {
                vec![]
            }
        }
        "/model" => {
            if let Some(eq_idx) = args.find('/') {
                let provider = args[..eq_idx].to_string();
                let model_id = args[eq_idx + 1..].to_string();
                system(state, format!("Switching to {provider}/{model_id}..."));
                vec![Effect::AgentCommand(AgentCmd::SetModel(provider, model_id))]
            } else {
                // Malformed: give the user the syntax instead of silently
                // dropping the input (regression: the prompt used to be
                // cleared with no feedback).
                system(state, "Usage: /model <provider>/<model-id>".into());
                vec![]
            }
        }
        "/theme" => {
            // Switch the active palette. `/theme` toggles dark/light;
            // `/theme dark` / `/theme light` (or any prefix) picks one
            // explicitly. Unknown names report the valid choices instead
            // of silently doing nothing.
            let toggled = |state: &mut AppState| {
                let next = if state.model.theme.name == "dark" {
                    pi_tui::Theme::light()
                } else {
                    pi_tui::Theme::default()
                };
                let name = next.name;
                app::update(&mut state.model, pi_tui::Msg::SetTheme(next));
                system(state, format!("Theme switched to {name}"));
            };
            let arg = args.trim().to_lowercase();
            match arg.as_str() {
                "" => toggled(state),
                // 前缀匹配：参数是主题名的前缀即可（`/theme lig` → light），
                // 而不是参数以主题名开头。"dark"/"light" 无公共前缀，
                // 不会歧义；参数为 "" 已被上一分支截走。
                a if "dark".starts_with(a) => {
                    app::update(&mut state.model, pi_tui::Msg::SetTheme(pi_tui::Theme::default()));
                    system(state, "Theme switched to dark".into());
                }
                a if "light".starts_with(a) => {
                    app::update(&mut state.model, pi_tui::Msg::SetTheme(pi_tui::Theme::light()));
                    system(state, "Theme switched to light".into());
                }
                _ => {
                    system(
                        state,
                        format!("Unknown theme '{args}'. Usage: /theme [dark|light]"),
                    );
                }
            }
            vec![]
        }
        "/help" => {
            let mut help = "Commands: /new, /name <name>, /model <provider>/<id>, /theme [dark|light], /help, /quit".to_string();
            if !state.ext_commands.is_empty() {
                help.push_str(&format!("\nExtension: /{}", state.ext_commands.join(", /")));
            }
            system(state, help);
            vec![]
        }
        "/reload" => {
            system(state, "Reloading extensions...".into());
            vec![Effect::AgentCommand(AgentCmd::ReloadExtensions)]
        }
        "/copy" => {
            // 复制在 agent 任务里执行（需要 session 读最后一条 assistant
            // 消息），结果经 result_tx 回显状态（对齐 TS `handleCopyCommand`：
            // 成功/失败/空消息三态提示，无中间状态）。
            vec![Effect::AgentCommand(AgentCmd::CopyLastMessage)]
        }
        "/quit" | "/exit" => {
            state.quit = true;
            vec![]
        }
        _ => {
            // Extension-registered slash commands: route to the extension
            // executor when the name matches; unknown commands fall through
            // to a regular message (matching the original behavior).
            let cmd_name = &command[1..];
            if state.ext_commands.iter().any(|c| c == cmd_name) {
                system(state, format!("Running extension command: /{cmd_name}"));
                vec![Effect::AgentCommand(AgentCmd::ExtensionCommand(cmd_name.to_string(), args.to_string()))]
            } else {
                state.model.input.clear();
                state.model.push_message("user", text.to_string());
                state.model.is_streaming = true;
                vec![Effect::AgentCommand(AgentCmd::SendMessage(text.to_string()))]
            }
        }
    }
}

// ============================================================================
// effects executor (grok `app/effects.rs`)
// ============================================================================

// ============================================================================
// 补全（对齐 TS completion：命令 fuzzy / 命令参数 / `@` 文件走查）
// ============================================================================

/// 补全所需的不变数据快照：命令列表（含参数补全回调）+ cwd。
struct CompletionSources {
    commands: Vec<pi_tui::CompletionCommand>,
    cwd: String,
}

/// `/model <provider>/<id>` 参数补全（对齐 TS `createBaseAutocompleteProvider`
/// 里 modelCommand.getArgumentCompletions：fuzzy 过滤可用模型快照）。
fn model_argument_completions(models: Vec<pi_agent_core::pi_ai_types::Model>) -> pi_tui::ArgumentCompletionsFn {
    // TS getModelSearchText：`id provider provider/id provider id name`。
    let search: Vec<String> = models
        .iter()
        .map(|m| {
            let name = if m.name.is_empty() { String::new() } else { format!(" {}", m.name) };
            format!("{} {} {}/{} {} {}{name}", m.id, m.provider, m.provider, m.id, m.provider, m.id)
        })
        .collect();
    let items: Vec<pi_tui::CompletionItem> = models
        .iter()
        .map(|m| pi_tui::CompletionItem::new(format!("{}/{}", m.provider, m.id), m.id.clone(), m.provider.clone()))
        .collect();
    std::sync::Arc::new(move |prefix: String| {
        let search = search.clone();
        let items = items.clone();
        Box::pin(async move {
            // fuzzy_filter_indices 用 search 文本排序，再映射回 item。
            let idx = pi_tui::fuzzy::fuzzy_filter_indices(&search, &prefix, |t| t.clone());
            if idx.is_empty() {
                None
            } else {
                Some(idx.into_iter().map(|(i, _)| items[i].clone()).collect())
            }
        })
    })
}

/// 包装扩展命令的 `get_argument_completions`（pi-extension-api → pi-tui 类型）。
fn wrap_extension_argument_completions(
    f: pi_extension_api::ArgumentCompletionsFn,
) -> pi_tui::ArgumentCompletionsFn {
    std::sync::Arc::new(move |prefix: String| {
        let f = f.clone();
        Box::pin(async move {
            f(prefix).await.map(|items| {
                items
                    .into_iter()
                    .map(|i| pi_tui::CompletionItem {
                        value: i.value,
                        label: i.label,
                        description: i.description.unwrap_or_default(),
                    })
                    .collect()
            })
        })
    })
}

/// 构建补全命令列表（对齐 TS `createBaseAutocompleteProvider`）：
/// 内建命令（`/model` 带模型参数补全）+ 扩展命令（带各自参数补全）。
fn build_completion_commands(session: &AgentSession) -> Vec<pi_tui::CompletionCommand> {
    let mut commands = vec![
        pi_tui::CompletionCommand::new("/help", "Show commands", "help"),
        pi_tui::CompletionCommand::new("/new", "Start a new session", "new"),
        pi_tui::CompletionCommand::new("/name <name>", "Set the session name", "name"),
        pi_tui::CompletionCommand::new("/quit", "Quit", "quit"),
        pi_tui::CompletionCommand::new("/theme [dark|light]", "Switch theme (dark/light)", "theme"),
        pi_tui::CompletionCommand::new("/reload", "Reload extensions", "reload"),
    ];
    // `/model`：参数补全 = 可用模型列表（对齐 TS modelCommand.getArgumentCompletions）。
    let models = session.get_model_registry().get_available();
    commands.push(
        pi_tui::CompletionCommand::new("/model <provider>/<id>", "Switch model", "model")
            .with_argument_completions(model_argument_completions(models)),
    );
    if let Some(registry) = session.get_extension_registry() {
        for rc in registry.commands() {
            let mut cmd = pi_tui::CompletionCommand::new(
                format!("/{}", rc.name),
                rc.description.clone(),
                rc.name.clone(),
            );
            if let Some(ac) = rc.get_argument_completions.clone() {
                cmd = cmd.with_argument_completions(wrap_extension_argument_completions(ac));
            }
            commands.push(cmd);
        }
    }
    commands
}

/// 解析一次补全请求（slash fuzzy / 命令参数 / `@` 文件走查）。
async fn resolve_completion(
    sources: &CompletionSources,
    req: &pi_tui::CompletionRequest,
) -> Vec<pi_tui::CompletionItem> {
    use pi_tui::components::CompletionTrigger;
    match req.trigger {
        CompletionTrigger::Slash => {
            // TS：命令 fuzzy 过滤（name = 命令名）。
            let idx = pi_tui::fuzzy::fuzzy_filter_indices(&sources.commands, &req.query, |c| c.insert_text.clone());
            idx.into_iter()
                .map(|(i, _)| {
                    let c = &sources.commands[i];
                    pi_tui::CompletionItem::new(c.insert_text.clone(), c.label.clone(), c.description.clone())
                })
                .collect()
        }
        CompletionTrigger::Argument => {
            let Some(name) = &req.command else { return Vec::new() };
            let Some(cmd) = sources.commands.iter().find(|c| c.insert_text == *name) else {
                return Vec::new();
            };
            let Some(f) = &cmd.argument_completions else {
                return Vec::new();
            };
            f(req.query.clone()).await.unwrap_or_default()
        }
        CompletionTrigger::At => {
            // `@` 附件 → fuzzy 走查（ignore ≈ fd）；Tab 强制普通路径 → readdir。
            let cwd = sources.cwd.clone();
            let prefix = req.prefix.clone();
            let (raw, is_at, is_quoted) = pi_tui::completion::parse_path_prefix(&prefix);
            let is_at = is_at || req.force;
            tokio::task::spawn_blocking(move || {
                if is_at {
                    pi_tui::completion::fuzzy_file_suggestions(&cwd, &raw, is_quoted, true, 20)
                } else {
                    pi_tui::completion::file_suggestions(&cwd, &prefix)
                }
            })
            .await
            .unwrap_or_default()
        }
    }
}

/// Run one effect. This is the only place that performs I/O / touches the
/// agent. `Abort` bypasses the session mutex deliberately (see [`Effect`]).
async fn execute_effect(
    effect: Effect,
    agent_handle: &Arc<Agent>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AgentCmd>,
    completion_sources: &Arc<CompletionSources>,
    result_tx: &tokio::sync::mpsc::UnboundedSender<pi_tui::Msg>,
) {
    match effect {
        Effect::Abort => {
            agent_handle.abort().await;
        }
        Effect::AbortAndClearQueues(current_text) => {
            // 对齐 TS restoreQueuedMessagesToEditor({abort:true}) 的顺序：
            // 先清 agent 队列（防止中止后 settled 循环消费排队 follow-up
            // 自动续跑），再中断当前 run。agent 队列与 session 锁无关，可在
            // 后台任务持有 session 锁期间安全调用；队列文本的镜像保存在
            // session 上，由随后的 RestoreQueuedToEditor 命令（run 结束后
            // 拿到锁）还原回编辑器。
            agent_handle.clear_all_queues().await;
            agent_handle.abort().await;
            let _ = cmd_tx.send(AgentCmd::RestoreQueuedToEditor(current_text));
        }
        Effect::AgentCommand(cmd) => {
            let _ = cmd_tx.send(cmd);
        }
        Effect::RequestCompletion(req) => {
            // 异步补全（对齐 TS requestAutocomplete + debounce + abort）：
            // 过期结果由 Completer 的 request_seq 丢弃。
            let sources = Arc::clone(completion_sources);
            let result_tx = result_tx.clone();
            tokio::spawn(async move {
                if req.debounce_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(req.debounce_ms)).await;
                }
                let items = resolve_completion(&sources, &req).await;
                let _ = result_tx.send(pi_tui::Msg::CompletionResults { seq: req.seq, items });
            });
        }
    }
}

// ============================================================================
// Agent command background task (session-locked commands)
// ============================================================================

fn spawn_agent_command_task(
    session: Arc<Mutex<AgentSession>>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<AgentCmd>,
    exit_flag: Arc<std::sync::atomic::AtomicBool>,
    result_tx: tokio::sync::mpsc::UnboundedSender<pi_tui::Msg>,
) {
    tokio::spawn(async move {
        while !exit_flag.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    let mut sess = session.lock().await;
                    match cmd {
                        AgentCmd::SendMessage(text) => sess.add_user_text(&text).await,
                        AgentCmd::AbortBash => sess.abort().await,
                        AgentCmd::SetModel(provider, model_id) => {
                            // Resolve the full model (api/base_url/etc.) from the
                            // model registry, matching TS `findExactModelMatch`.
                            // Constructing a Model by hand with an empty `api`
                            // used to panic in pi-ai's `resolve_api_provider`
                            // when a message was sent right after the switch.
                            let model = sess.get_model_registry().find(&provider, &model_id);
                            let Some(model) = model else {
                                let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                    "system".into(),
                                    format!("Model not found: {provider}/{model_id}"),
                                ));
                                continue;
                            };
                            // Surface the outcome (the slash command already
                            // claimed success optimistically; set_model can
                            // fail on auth, and a silent drop left the footer
                            // model name stale).
                            match sess.set_model(model).await {
                                Ok(()) => {
                                    let _ = result_tx.send(pi_tui::Msg::SetModelName(model_id.clone()));
                                    let _ = result_tx.send(pi_tui::Msg::SetProvider(Some(provider.clone())));
                                    let _ = result_tx.send(pi_tui::Msg::SetReasoning(false));
                                    let _ = result_tx.send(pi_tui::Msg::SetContextWindow(128000));
                                    let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                        "system".into(),
                                        format!("Switched to {provider}/{model_id}"),
                                    ));
                                }
                                Err(e) => {
                                    let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                        "system".into(),
                                        format!("Failed to switch model: {e}"),
                                    ));
                                }
                            }
                        }
                        AgentCmd::CycleModel(direction) => {
                            // Cycle through available models (real API) and
                            // surface the result in the UI.
                            if let Some((model, _tl, _scoped)) = sess.cycle_model(&direction).await {
                                let _ = result_tx.send(pi_tui::Msg::SetModelName(model.id.clone()));
                                let _ = result_tx.send(pi_tui::Msg::SetProvider(Some(model.provider.clone())));
                                let _ = result_tx.send(pi_tui::Msg::SetReasoning(model.reasoning));
                                let _ = result_tx.send(pi_tui::Msg::SetContextWindow(model.context_window));
                                let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                    "system".into(),
                                    format!("Model: {}", model.id),
                                ));
                            }
                        }
                        AgentCmd::CycleThinkingLevel => {
                            // Cycle through thinking levels (real API).
                            if let Some(level) = sess.cycle_thinking_level().await {
                                let _ = result_tx.send(pi_tui::Msg::SetThinkingLevel(Some(level.clone())));
                                let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                    "system".into(),
                                    format!("Thinking level: {level}"),
                                ));
                            }
                        }
                        AgentCmd::NewSession(parent) => {
                            sess.session_mgr_new(parent.as_deref()).await;
                        }
                        AgentCmd::SetSessionName(name) => {
                            sess.set_session_name(&name);
                        }
                        AgentCmd::CopyLastMessage => {
                            // `/copy`（TS `handleCopyCommand`）：复制最后一条
                            // assistant 消息；没有消息时给错误提示。
                            match sess.get_last_assistant_text().await {
                                Some(text) => {
                                    if let Err(e) = pi_tui::clipboard::copy_to_clipboard(&text) {
                                        let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                            "system".into(),
                                            format!("Failed to copy to clipboard: {e}"),
                                        ));
                                    } else {
                                        let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                            "system".into(),
                                            "Copied last agent message to clipboard".into(),
                                        ));
                                    }
                                }
                                None => {
                                    let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                        "system".into(),
                                        "No agent messages to copy yet.".into(),
                                    ));
                                }
                            }
                        }
                        AgentCmd::ExtensionCommand(cmd_name, args) => {
                            // Run a slash command registered by an extension.
                            // The handler runs on a blocking thread with its
                            // own current-thread runtime: extensions may block
                            // synchronously (e.g. `ui.confirm` waits for the
                            // user), and a block must never occupy a worker of
                            // the main runtime that drives the TUI event loop.
                            let (cmd, ctx_opt) = {
                                if let Some(registry) = sess.get_extension_registry() {
                                    let cmd = registry
                                        .commands()
                                        .iter()
                                        .find(|c| c.name == cmd_name)
                                        .cloned();
                                    (cmd, Some(sess.get_extension_context().clone()))
                                } else {
                                    (None, None)
                                }
                            };
                            drop(sess); // release the lock before blocking
                            if let Some(cmd) = cmd {
                                // 命令执行可能同步入队 follow-up（如 /goal 的
                                // owned prompt）。等命令结束后在 session 上下文
                                // 里消费 settled 队列启动 run（对齐
                                // prompt() 的 _tryExecuteExtensionCommand 尾部）。
                                let handle = tokio::task::spawn_blocking(move || {
                                    let rt = match tokio::runtime::Builder::new_current_thread()
                                        .enable_all()
                                        .build()
                                    {
                                        Ok(rt) => rt,
                                        Err(e) => {
                                            eprintln!("extension runtime: {e}");
                                            return;
                                        }
                                    };
                                    rt.block_on((cmd.execute)(args, ctx_opt.as_ref()));
                                });
                                let _ = handle.await;
                            }
                            // 消费扩展命令入队的 follow-up（对齐 TS 交互模式）。
                            let sess = session.lock().await;
                            sess.run_settled_continuations().await;
                        }
                        AgentCmd::RestoreQueuedToEditor(current_text) => {
                            // TS restoreQueuedMessagesToEditor：clearAllQueues
                            // 拿回排队文本（镜像在 Esc 时未被清，仍保留），与
                            // 当前编辑器文本合并后还原。
                            let (steering, follow_up) = sess.clear_all_queues().await;
                            let mut parts: Vec<String> = steering
                                .into_iter()
                                .chain(follow_up)
                                .filter(|t| !t.trim().is_empty())
                                .collect();
                            if !current_text.trim().is_empty() {
                                parts.push(current_text);
                            }
                            let combined = parts.join("\n\n");
                            if !combined.is_empty() {
                                let _ = result_tx.send(pi_tui::Msg::SetEditorText(combined));
                            }
                        }
                        AgentCmd::ReloadExtensions => {
                            // Extension reload is not applicable for Rust
                            // native extensions.
                        }
                    }
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
            }
        }
    });
}

/// Bridge agent events → pi-tui messages (assistant message opened on first
/// delta so deltas never stick to the user message).
fn spawn_agent_bridge_task(mut bridge_rx: tokio::sync::mpsc::UnboundedReceiver<crate::modes::agent_bridge::AgentEvent>) -> tokio::sync::mpsc::UnboundedReceiver<pi_tui::Msg> {
    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel::<pi_tui::Msg>();
    let atx = agent_tx.clone();
    tokio::spawn(async move {
        use crate::modes::agent_bridge::AgentEvent as BE;
        let mut assistant_stream_open = false;
        while let Some(ev) = bridge_rx.recv().await {
            let msgs: Vec<pi_tui::Msg> = match ev {
                BE::TextDelta(d) => {
                    let mut v = Vec::new();
                    if !assistant_stream_open {
                        v.push(pi_tui::Msg::NewMessage("assistant".into(), String::new()));
                        assistant_stream_open = true;
                    }
                    v.push(pi_tui::Msg::StreamText(d));
                    v
                }
                BE::MessageEnd { text, thinking, stop_reason, error_message } => {
                    let mut v = Vec::new();
                    if !assistant_stream_open && !text.is_empty() {
                        v.push(pi_tui::Msg::NewMessage("assistant".into(), text));
                    }
                    let stop = stop_reason.map(|r| match r {
                        pi_agent_core::pi_ai_types::StopReason::Length => pi_tui::app::StopReason::Length,
                        pi_agent_core::pi_ai_types::StopReason::Aborted => pi_tui::app::StopReason::Aborted,
                        _ => pi_tui::app::StopReason::Error,
                    });
                    v.push(pi_tui::Msg::MessageEnd {
                        thinking,
                        stop_reason: stop,
                        error_message,
                    });
                    assistant_stream_open = false;
                    v
                }
                BE::ToolStart(call_id, name, args) => vec![pi_tui::Msg::ToolStart(call_id, name, args)],
                BE::ToolEnd(call_id, name, e) => vec![pi_tui::Msg::ToolEnd(call_id, name, e)],
                BE::ToolOutput(call_id, name, o) => vec![pi_tui::Msg::SetToolOutput(call_id, name, o)],
                BE::ToolTruncation(call_id, t) => vec![pi_tui::Msg::SetToolTruncation(call_id, t)],
            };
            for m in msgs {
                if atx.send(m).is_err() {
                    break;
                }
            }
        }
    });
    agent_rx
}

// ============================================================================
// run (grok `app/event_loop.rs`)
// ============================================================================

/// Run the interactive TUI mode.
/// Mirrors the original InteractiveMode.run().
pub async fn run_interactive_mode(mut session: AgentSession) -> i32 {
    // ── Theme auto-detection（对齐 TS `InteractiveThemeController`）──────
    // TS：`detectTerminalBackgroundFromEnv()`（COLORFGBG）→ `applyFromSettings`
    // 里无显式设置时 `detectTerminalBackgroundTheme`（OSC 11 查询 → env →
    // dark fallback），high confidence 时持久化到 settings。
    // OSC 11 查询需要 raw mode（canonical 模式 stdin 等到换行才返回，OSC
    // 响应以 BEL 结尾）且必须在 crossterm EventStream 启动前完成（它读
    // stdin 时会吞掉无法解析的 OSC 响应）。
    let theme_detection = {
        let _ = crossterm::terminal::enable_raw_mode();
        let detection = pi_tui::detect::detect_terminal_background_theme(
            std::time::Duration::from_millis(100),
        );
        let _ = crossterm::terminal::disable_raw_mode();
        detection
    };
    let theme_setting = session.get_theme_setting();
    let resolved_theme = resolve_theme_setting(theme_setting.as_deref(), theme_detection.theme);
    // TS `applyFromSettings`：无显式设置且探测为 high confidence 时把
    // 探测结果写入 settings（避免每次启动重复探测）。
    if theme_setting.is_none() && theme_detection.confidence_high() {
        session.set_theme_setting(theme_detection.theme.name());
    }

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
    );

    let mut terminal = match pi_tui::Terminal::new() {
        Ok(t) => t,
        Err(e) => { restore_terminal(); eprintln!("Failed to initialize terminal: {e}"); return 1; }
    };

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

    // ── Extension command snapshot + initial model name ──────────────────
    // (plain &self accessors — the session mutex is only needed inside the
    // background task; the model was already resolved at session creation)
    let (ext_commands, initial_model_name, cwd) = {
        let cmds = session
            .get_extension_registry()
            .map(|r| r.commands().iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        let model_id = session.get_state().await.model.id.clone();
        let cwd = session.get_cwd().to_string();
        (cmds, model_id, cwd)
    };

    // ── Extension UI bridge: wire dialogs/notifications to the TUI ───────
    // (must happen before the session is shared with the background task)
    let (ui_ctx, ui_tx, mut ui_rx) = create_tui_ui_context();
    session.set_extension_ui_context(ui_ctx);
    // 扩展错误 → 系统消息（对齐 TS interactive-mode `showExtensionError`：
    // 错误以文本形式进入聊天区，而不是写 stderr 污染备用屏幕）。
    session.set_extension_error_listener(Some(Box::new(move |err| {
        let _ = ui_tx.send(UiAction::Notify(format!(
            "Extension \"{}\" error: {}",
            err.extension_path, err.error
        )));
    })));
    // TUI 模式标记：扩展（如 pi-goal）据此展示 TUI 菜单（对齐 TS
    // `ctx.mode === "tui"`）。
    session.set_extension_mode("tui");

    let mut state = AppState::new(cols, rows, ext_commands);

    // ── 补全（对齐 TS createBaseAutocompleteProvider + autocompleteMaxVisible）──
    let completion_commands = build_completion_commands(&session);
    state
        .model
        .completer
        .set_commands(completion_commands.clone());
    state
        .model
        .completer
        .set_max_visible(session.get_autocomplete_max_visible() as usize);
    let completion_sources = Arc::new(CompletionSources {
        commands: completion_commands,
        cwd: cwd.clone(),
    });

    // 探测/设置解析出的初始主题（TS `initTheme(activeThemeName)`）；
    // 只有 "light" 内置主题名会切到浅色，其余（含自定义主题名，pi-rs
    // 无自定义主题系统）落到 dark。
    if resolved_theme.as_deref() == Some("light") {
        state.model.theme = pi_tui::Theme::light();
    }
    state.model.model_name = initial_model_name;
    state.model.cwd = cwd.clone();
    state.model.git_branch = current_git_branch(&cwd);
    // ── TS footer data: session name + model identity (provider, reasoning,
    //    context window, thinking level) — plain &self accessors. ────────
    state.model.session_name = session.get_session_name();
    {
        let model = session.get_state().await.model;
        state.model.provider = Some(model.provider);
        state.model.reasoning = model.reasoning;
        state.model.context_window = model.context_window;
        state.model.thinking_level = Some(session.get_thinking_level().await);
    }

    let (mut input_rx, shutdown_guard) = match terminal.start() {
        Ok(r) => r,
        Err(e) => { restore_terminal(); eprintln!("Failed to start terminal input: {e}"); return 1; }
    };

    // ── Agent command channel + background task ───────────────────────────
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<AgentCmd>();
    // Result channel: the background task reports command outcomes (model
    // changes, thinking level) back into the TUI event stream.
    let (result_tx, mut result_rx) = tokio::sync::mpsc::unbounded_channel::<pi_tui::Msg>();
    let session = Arc::new(Mutex::new(session));
    let bg_session = session.clone();
    let bg_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_exit_flag = bg_exit.clone();
    spawn_agent_command_task(bg_session, cmd_rx, bg_exit_flag, result_tx.clone());

    // ── Subscribe agent events (lock-and-release) ───────────────────────
    // Keep an `Arc<Agent>` handle for abort: `abort()` is `&self` and must
    // NOT be routed through the session mutex — `add_user_text` holds the
    // lock for the whole run, so a queued Abort command would only execute
    // after the run finishes (never, for a long run).
    let (bridge_tx, bridge_rx) = tokio::sync::mpsc::unbounded_channel::<crate::modes::agent_bridge::AgentEvent>();
    let agent_handle = {
        let mut sess = session.lock().await;
        crate::modes::agent_bridge::subscribe_agent(&mut sess, bridge_tx).await;
        sess.agent_handle()
    }; // Lock released — background task can now use the session

    // Keep a session handle for the event loop to refresh status-bar data
    // (context usage) between runs — never during a run (mutex is held).
    let ui_session = session.clone();

    let mut agent_rx = spawn_agent_bridge_task(bridge_rx);

    // ── Event loop: Action → update → effects → execute ──────────────────
    // Event-driven rendering (Elm style): draw only when state changed.
    // 首次渲染（行级差分渲染器，见下方主循环）。
    {
        let _ = terminal.ratatui_terminal().autoresize();
        let mut frame = terminal.ratatui_terminal().get_frame();
        let area = frame.area();
        app::view(&mut state.model, &mut frame);
        let cursor = state.model.cursor_pos.take();
        let lines = pi_tui::line_screen::buffer_to_lines(frame.buffer_mut());
        let _ = state
            .line_screen
            .render(&lines, cursor, area.width, area.height);
        terminal.ratatui_terminal().swap_buffers();
    }

    let mut tick_timer = tokio::time::interval(tokio::time::Duration::from_millis(SPINNER_TICK_MS));
    loop {
        if state.quit {
            break;
        }

        let action = tokio::select! {
            _ = tick_timer.tick() => Action::Tick,
            Some(input) = input_rx.recv() => {
                match input {
                    pi_tui::terminal::InputEvent::Key(key) => {
                        use crossterm::event::KeyEventKind;
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }
                        Action::Key(key)
                    }
                    pi_tui::terminal::InputEvent::ScrollUp => Action::Agent(pi_tui::Msg::ScrollUp(3)),
                    pi_tui::terminal::InputEvent::ScrollDown => Action::Agent(pi_tui::Msg::ScrollDown(3)),
                    // 尺寸变化：更新模型 + 强制全量重绘（见下方 resize 分支）。
                    pi_tui::terminal::InputEvent::Resize(w, h) => {
                        Action::Agent(pi_tui::Msg::Resize(w, h))
                    }
                }
            }
            Some(msg) = agent_rx.recv() => Action::Agent(msg),
            Some(msg) = result_rx.recv() => Action::Agent(msg),
            Some(action) = ui_rx.recv() => Action::Ui(action),
        };

        let is_tick = matches!(&action, Action::Tick);
        let outcome = update(&mut state, action);
        let mut redraw = outcome.redraw;
        if is_tick {
            redraw |= refresh_status(&mut state, &ui_session).await;
        }
        for effect in outcome.effects {
            execute_effect(effect, &agent_handle, &cmd_tx, &completion_sources, &result_tx).await;
        }
        if state.quit {
            break;
        }
        if redraw {
            // 行级差分渲染（对齐 TS TuiAltScreen.doRender / grok-build）：
            // 整行比较、整行重写（`ESC[{row};1H` + `ESC[2K` + 整行），只在
            // 首帧/尺寸变化时全量清屏重绘，更新批次用 synchronized output
            // 包裹。整行重写不存在 cell 级局部更新，宽字符（CJK）不会被
            // 半个覆盖——终端上不会留下"每帧固定在原位"的陈旧字符。
            // `get_frame()` 不触发 autoresize（`draw()` 才会），尺寸变化
            // 必须手动同步 buffer，否则渲染仍按旧尺寸输出。
            let _ = terminal.ratatui_terminal().autoresize();
            let mut frame = terminal.ratatui_terminal().get_frame();
            let area = frame.area();
            app::view(&mut state.model, &mut frame);
            let cursor = state.model.cursor_pos.take();
            let lines = pi_tui::line_screen::buffer_to_lines(frame.buffer_mut());
            let _ = state
                .line_screen
                .render(&lines, cursor, area.width, area.height);
            terminal.ratatui_terminal().swap_buffers();
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────────────
    bg_exit.store(true, std::sync::atomic::Ordering::SeqCst);
    shutdown_guard.shutdown();
    // Kill any background processes left running by bash commands (matching
    // TS interactive-mode shutdown which calls `killTrackedDetachedChildren()`).
    crate::utils::shell::kill_tracked_detached_children();
    restore_terminal();
    0
}

/// Best-effort current git branch of `cwd` (`git rev-parse --abbrev-ref
/// HEAD`). `None` when the directory isn't a git worktree or git is absent.
fn current_git_branch(cwd: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Cumulative token/cost totals from all session entries (TS
/// `createUsageTotals` + `addUsageToTotals`): assistant messages, tool
/// results, compaction and branch summaries.
fn usage_totals_from_entries(entries: &[&crate::core::session_manager::SessionEntry]) -> pi_tui::app::UsageTotals {
    let mut totals = pi_tui::app::UsageTotals::default();
    for entry in entries {
        let entry: &crate::core::session_manager::SessionEntry = entry;
        let usage: Option<&serde_json::Value> = match entry {
            crate::core::session_manager::SessionEntry::Message { message, .. } => {
                match message.get("role").and_then(|r| r.as_str()) {
                    Some("assistant") | Some("toolResult") => message.get("usage"),
                    _ => None,
                }
            }
            crate::core::session_manager::SessionEntry::Compaction { usage, .. }
            | crate::core::session_manager::SessionEntry::BranchSummary { usage, .. } => {
                usage.as_ref()
            }
            _ => None,
        };
        if let Some(u) = usage {
            totals.input += u.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
            totals.output += u.get("output").and_then(|v| v.as_u64()).unwrap_or(0);
            totals.cache_read += u.get("cacheRead").and_then(|v| v.as_u64()).unwrap_or(0);
            totals.cache_write += u.get("cacheWrite").and_then(|v| v.as_u64()).unwrap_or(0);
            totals.cost += u
                .get("cost")
                .and_then(|c| c.get("total"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            // Latest assistant message's cache hit rate (TS
            // `latestCacheHitRate`).
            if matches!(
                entry,
                crate::core::session_manager::SessionEntry::Message { message, .. }
                    if message.get("role").and_then(|r| r.as_str()) == Some("assistant")
            ) {
                let input = u.get("input").and_then(|v| v.as_u64()).unwrap_or(0);
                let cache_read = u.get("cacheRead").and_then(|v| v.as_u64()).unwrap_or(0);
                let cache_write = u.get("cacheWrite").and_then(|v| v.as_u64()).unwrap_or(0);
                let prompt_tokens = input + cache_read + cache_write;
                totals.cache_hit_rate = if prompt_tokens > 0 {
                    Some(cache_read as f64 / prompt_tokens as f64 * 100.0)
                } else {
                    None
                };
            }
        }
    }
    totals
}

/// Refresh status-bar data derived from the session (context usage, throttled
/// to ~1/s) and the in-memory streaming elapsed timer. Returns true if the
/// view changed and needs a redraw.
async fn refresh_status(
    state: &mut AppState,
    session: &Arc<Mutex<AgentSession>>,
) -> bool {
    let mut changed = false;

    // Context usage + footer stats: only query between runs (the agent
    // holds the session mutex during a run) and throttled to once per
    // second.
    if state.last_status_refresh.elapsed().as_secs() >= 1 {
        state.last_status_refresh = Instant::now();
        if let Ok(sess) = session.try_lock() {
            if let Some(usage) = sess.get_context_usage().await {
                let pct = usage.usage_percentage();
                if (pct - state.model.context_usage_pct).abs() > f64::EPSILON {
                    state.model.context_usage_pct = pct;
                    changed = true;
                }
                if usage.context_window != state.model.context_window {
                    state.model.context_window = usage.context_window;
                    changed = true;
                }
                if !state.model.context_usage_known {
                    state.model.context_usage_known = true;
                    changed = true;
                }
            }
            // Cumulative token/cost totals (TS FooterComponent reads the
            // session entries on every render).
            let totals = usage_totals_from_entries(&sess.get_session_manager().get_entries());
            if totals != state.model.usage_totals {
                state.model.usage_totals = totals;
                changed = true;
            }
        }
    }

    // Streaming elapsed timer.
    if state.model.is_streaming {
        let start = *state.stream_started_at.get_or_insert_with(Instant::now);
        let secs = start.elapsed().as_secs();
        if secs != state.model.elapsed_secs {
            state.model.elapsed_secs = secs;
            changed = true;
        }
    } else if state.stream_started_at.take().is_some() {
        state.model.elapsed_secs = 0;
        changed = true;
    }

    changed
}

fn restore_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = crossterm::terminal::disable_raw_mode();
}

// ============================================================================
// Theme setting resolution (TS `theme.ts` parseAutoThemeSetting /
// resolveThemeSetting)
// ============================================================================

/// TS `parseAutoThemeSetting`: `"lightTheme/darkTheme"` (exactly one `/`,
/// both sides non-empty) is an auto theme setting.
fn parse_auto_theme_setting(setting: &str) -> Option<(&str, &str)> {
    let mut parts = setting.splitn(3, '/');
    let light = parts.next()?;
    let dark = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let light = light.trim();
    let dark = dark.trim();
    if light.is_empty() || dark.is_empty() {
        None
    } else {
        Some((light, dark))
    }
}

/// TS `resolveThemeSetting`: auto setting resolves through the detected
/// terminal theme; a setting containing `/` that is not a valid auto pair
/// means "no explicit theme" (undefined); anything else is used verbatim.
fn resolve_theme_setting(
    setting: Option<&str>,
    terminal_theme: pi_tui::detect::TerminalTheme,
) -> Option<String> {
    let setting = setting?;
    if let Some((light, dark)) = parse_auto_theme_setting(setting) {
        return Some(
            match terminal_theme {
                pi_tui::detect::TerminalTheme::Light => light,
                pi_tui::detect::TerminalTheme::Dark => dark,
            }
            .to_string(),
        );
    }
    if setting.contains('/') {
        return None;
    }
    Some(setting.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn state() -> AppState {
        AppState::new(120, 80, Vec::new())
    }


    // ── Esc 行为（对齐 TS onEscape：不退出，流式→中断+还原队列文本）──────

    /// Esc 流式中：不退出，立即停 spinner，返回带当前编辑器文本的
    /// AbortAndClearQueues（队列文本还原由后台任务完成）。
    #[test]
    fn esc_while_streaming_aborts_with_current_editor_text() {
        let mut s = state();
        s.model.is_streaming = true;
        s.model.input.set_value("half-typed");
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        let effects = handle_key(&mut s, key);
        assert!(!s.quit, "Esc 不能退出 TUI");
        assert!(!s.model.is_streaming, "UI 立即停止 spinner");
        assert_eq!(
            effects,
            vec![Effect::AbortAndClearQueues("half-typed".to_string())]
        );
    }

    /// Esc 空闲时（第一下/500ms 内第二下）：不退出、无副作用
    /// （TS 默认弹 tree/fork 选择器，本 port 未实现 → no-op）。
    #[test]
    fn esc_while_idle_does_not_quit() {
        let mut s = state();
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        let first = handle_key(&mut s, key);
        assert!(!s.quit, "第一下 Esc 不退出");
        assert_eq!(first, vec![]);
        let second = handle_key(&mut s, key);
        assert!(!s.quit, "双击 Esc 不退出（选择器未实现）");
        assert_eq!(second, vec![]);
    }

    // ── 补全应用（对齐 TS applyCompletion）──────────────────────────────

    /// Enter 应用 slash 补全后**继续提交**（TS：`/` 前缀应用后 fall
    /// through 到 submit，执行命令）。
    #[test]
        fn enter_applies_slash_completion_then_submits() {
        let mut s = state();
        s.model.completer.set_commands(vec![
            pi_tui::CompletionCommand::new("/new", "Start a new session", "new"),
        ]);
        s.model.input.set_value("/n");
        s.model
            .completer
            .begin(pi_tui::components::CompletionTrigger::Slash, "/n", "n");
        s.model.completer.apply_results(
            1,
            vec![pi_tui::CompletionItem::new("new", "/new", "Start a new session")],
        );
        assert!(s.model.completer.visible);
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let effects = handle_key(&mut s, key);
        // "/new " 提交 → slash_command 清空输入并返回 NewSession 命令。
        assert_eq!(
            effects,
            vec![Effect::AgentCommand(AgentCmd::NewSession(None))]
        );
        assert_eq!(s.model.input.value(), "", "提交后输入被清空");
        assert!(!s.model.completer.visible);
    }

    /// Enter 应用 `@` 文件补全后**不提交**（对齐 TS：非 `/` 前缀应用后
    /// 停留编辑），且插入完整路径（从最后的 '@' 替换，不丢 @src/ 前缀、
    /// 不加强制空格）。
    #[test]
    fn enter_applies_at_completion_without_submitting() {
        let mut s = state();
        s.model.input.set_value("@src/mai");
        s.model
            .completer
            .begin(pi_tui::components::CompletionTrigger::At, "@src/mai", "mai");
        s.model.completer.apply_results(
            1,
            vec![pi_tui::CompletionItem::new(
                "@src/main.rs",
                "main.rs",
                "src/main.rs",
            )],
        );
        assert!(s.model.completer.visible);
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        let effects = handle_key(&mut s, key);
        assert_eq!(effects, vec![], "@ 补全应用后不提交消息");
        // TS @ 分支：文件加一个空格，目录不加（可继续补全）。
        assert_eq!(
            s.model.input.value(),
            "@src/main.rs ",
            "插入完整路径，文件后跟空格"
        );
        assert!(!s.model.completer.visible);
    }

    /// 补全弹窗打开时 Esc 只关弹窗，不触发 onEscape 链也不退出。
    #[test]
    fn esc_closes_completer_popup_without_quitting() {
        let mut s = state();
        s.model.completer.begin(
            pi_tui::components::CompletionTrigger::Slash,
            "",
            "",
        );
        assert!(s.model.completer.visible);
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        );
        let effects = handle_key(&mut s, key);
        assert!(!s.model.completer.visible, "Esc 关闭补全弹窗");
        assert!(!s.quit);
        assert_eq!(effects, vec![]);
    }

    // ── 补全解析（对齐 TS autocomplete.ts）─────────────────────────────

    /// slash 命令候选：fuzzy 过滤 + 值 = 命令名。
    #[tokio::test]
    async fn resolve_slash_completion_fuzzy_filters_commands() {
        let sources = CompletionSources {
            commands: vec![
                pi_tui::CompletionCommand::new("/model <provider>/<id>", "Switch model", "model"),
                pi_tui::CompletionCommand::new("/new", "Start a new session", "new"),
            ],
            cwd: ".".into(),
        };
        let req = pi_tui::CompletionRequest {
            seq: 1,
            trigger: pi_tui::components::CompletionTrigger::Slash,
            prefix: "/mod".into(),
            query: "mod".into(),
            command: None,
            debounce_ms: 0,
            force: false,
        };
        let items = resolve_completion(&sources, &req).await;
        assert_eq!(items.len(), 1, "只有 /model 匹配 mod: {items:?}");
        assert_eq!(items[0].value, "model");
        assert_eq!(items[0].label, "/model <provider>/<id>");
    }

    /// 命令参数补全：走命令注册的回调（对齐 TS getArgumentCompletions）。
    #[tokio::test]
    async fn resolve_argument_completion_calls_callback() {
        let f: pi_tui::ArgumentCompletionsFn = std::sync::Arc::new(|prefix: String| {
            Box::pin(async move {
                Some(vec![pi_tui::CompletionItem::new(
                    format!("openai/{prefix}"),
                    prefix.clone(),
                    "openai",
                )])
            })
        });
        let sources = CompletionSources {
            commands: vec![pi_tui::CompletionCommand::new("/model <provider>/<id>", "Switch model", "model")
                .with_argument_completions(f)],
            cwd: ".".into(),
        };
        let req = pi_tui::CompletionRequest {
            seq: 1,
            trigger: pi_tui::components::CompletionTrigger::Argument,
            prefix: "g".into(),
            query: "g".into(),
            command: Some("model".into()),
            debounce_ms: 0,
            force: false,
        };
        let items = resolve_completion(&sources, &req).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].value, "openai/g");
    }

    /// 未注册参数补全的命令：返回空（对齐 TS getSuggestions 返回 null）。
    #[tokio::test]
    async fn resolve_argument_completion_missing_callback_returns_empty() {
        let sources = CompletionSources {
            commands: vec![pi_tui::CompletionCommand::new("/name <name>", "Set the session name", "name")],
            cwd: ".".into(),
        };
        let req = pi_tui::CompletionRequest {
            seq: 1,
            trigger: pi_tui::components::CompletionTrigger::Argument,
            prefix: "x".into(),
            query: "x".into(),
            command: Some("name".into()),
            debounce_ms: 0,
            force: false,
        };
        assert!(resolve_completion(&sources, &req).await.is_empty());
    }

    /// `/model` 参数补全：fuzzy 过滤可用模型（对齐 TS createFuzzyAutocompleteItems）。
    #[tokio::test]
    async fn model_argument_completions_fuzzy_filter_models() {
        let models = vec![
            pi_agent_core::pi_ai_types::Model {
                id: "gpt-4o".into(),
                name: "GPT-4o".into(),
                api: "openai".into(),
                provider: "openai".into(),
                base_url: "http://localhost".into(),
                reasoning: false,
                thinking_level_map: None,
                input: vec!["text".into()],
                cost: pi_agent_core::pi_ai_types::ModelCost::default(),
                context_window: 128000,
                max_tokens: 4096,
                sampling_params: None,
                headers: None,
                compat: None,
            },
            pi_agent_core::pi_ai_types::Model {
                id: "gpt-4o-mini".into(),
                name: "GPT-4o mini".into(),
                api: "openai".into(),
                provider: "openai".into(),
                base_url: "http://localhost".into(),
                reasoning: false,
                thinking_level_map: None,
                input: vec!["text".into()],
                cost: pi_agent_core::pi_ai_types::ModelCost::default(),
                context_window: 128000,
                max_tokens: 4096,
                sampling_params: None,
                headers: None,
                compat: None,
            },
        ];
        let f = model_argument_completions(models);
        let items = f("gpt-4o-m".to_string()).await.expect("模型补全");
        assert_eq!(items.len(), 1, "gpt-4o-m 只匹配 gpt-4o-mini: {items:?}");
        assert_eq!(items[0].value, "openai/gpt-4o-mini");
    }

    #[test]
    fn theme_defaults_to_dark() {
        assert_eq!(state().model.theme.name, "dark");
    }

    #[test]
    fn theme_command_toggles_between_dark_and_light() {
        let mut s = state();
        // No arg toggles dark -> light.
        let effects = slash_command(&mut s, "/theme");
        assert_eq!(s.model.theme.name, "light", "toggle: dark -> light");
        assert_eq!(effects, vec![]);
        // Toggle again: light -> dark.
        slash_command(&mut s, "/theme");
        assert_eq!(s.model.theme.name, "dark", "toggle: light -> dark");
        // A system notice is pushed so the switch is visible.
        assert!(
            s.model.messages.iter().any(|m| m.role == "system"),
            "theme switch reports a system notice"
        );
    }

    #[test]
    fn theme_command_sets_named_theme_explicitly() {
        let mut s = state();
        slash_command(&mut s, "/theme light");
        assert_eq!(s.model.theme.name, "light");
        slash_command(&mut s, "/theme dark");
        assert_eq!(s.model.theme.name, "dark");
        // Prefix matching also works: `/theme lig` matches "light".
        slash_command(&mut s, "/theme lig");
        assert_eq!(s.model.theme.name, "light");
        // Case-insensitive: args are lowercased before matching.
        slash_command(&mut s, "/theme D");
        assert_eq!(s.model.theme.name, "dark");
        // A full name + junk does not match (prefix must be a real prefix).
        slash_command(&mut s, "/theme lightx");
        assert_eq!(s.model.theme.name, "dark", "non-prefix stays dark");
        assert!(
            s.model
                .messages
                .iter()
                .any(|m| m.role == "system" && m.text.contains("Unknown theme")),
            "lightx reports a usage notice"
        );
    }

    #[test]
    fn theme_command_reports_unknown_theme() {
        let mut s = state();
        slash_command(&mut s, "/theme solarized");
        // Theme is untouched and a usage notice is pushed.
        assert_eq!(s.model.theme.name, "dark");
        assert!(
            s.model
                .messages
                .iter()
                .any(|m| m.role == "system" && m.text.contains("Unknown theme")),
            "unknown theme reports a usage notice"
        );
    }

    // ============================================================
    // Theme setting resolution (TS parseAutoThemeSetting / resolveThemeSetting)
    // ============================================================

    #[test]
    fn auto_theme_setting_requires_exactly_one_slash() {
        assert_eq!(parse_auto_theme_setting("light/dark"), Some(("light", "dark")));
        // Spaces around names are trimmed.
        assert_eq!(parse_auto_theme_setting(" light / dark "), Some(("light", "dark")));
        // Empty side, two slashes, no slash → not an auto setting.
        assert_eq!(parse_auto_theme_setting("light/"), None);
        assert_eq!(parse_auto_theme_setting("/dark"), None);
        assert_eq!(parse_auto_theme_setting("a/b/c"), None);
        assert_eq!(parse_auto_theme_setting("dark"), None);
    }

    #[test]
    fn resolve_theme_setting_auto_follows_terminal_theme() {
        use pi_tui::detect::TerminalTheme;
        let auto = Some("solarized-light/solarized-dark");
        assert_eq!(
            resolve_theme_setting(auto, TerminalTheme::Light).as_deref(),
            Some("solarized-light")
        );
        assert_eq!(
            resolve_theme_setting(auto, TerminalTheme::Dark).as_deref(),
            Some("solarized-dark")
        );
        // Explicit name is used verbatim, independent of the terminal theme.
        assert_eq!(
            resolve_theme_setting(Some("light"), TerminalTheme::Dark).as_deref(),
            Some("light")
        );
        // "dark/light.json" is a *valid* auto pair in TS (lightTheme=dark,
        // darkTheme=light.json) — resolved through the terminal theme.
        assert_eq!(
            resolve_theme_setting(Some("dark/light.json"), TerminalTheme::Dark).as_deref(),
            Some("light.json")
        );
        // Slash values that fail parseAutoThemeSetting (two slashes) mean
        // "no theme" (TS resolveThemeSetting returns undefined).
        assert_eq!(resolve_theme_setting(Some("a/b/c"), TerminalTheme::Dark), None);
        // No setting → no resolution.
        assert_eq!(resolve_theme_setting(None, TerminalTheme::Dark), None);
    }
}
