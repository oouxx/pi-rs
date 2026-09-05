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

use crate::core::agent_session::{AgentSession, AgentSessionEvent, CompactionReason};
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
    /// 排队消息在无活动 run 时入队（按键到 effect 执行之间的窗口）：触发
    /// settled 续跑消费队列（TS 由 run 自己的 settled 循环消费）。
    RunSettledContinuations,
    /// `/compact [instructions]`（TS `handleCompactCommand` →
    /// `session.compact()`）：压缩会发 CompactionStart/End 事件（错误也经
    /// 事件回显，TS 忽略返回值）。经 Effect 先 abort 在飞 run（TS
    /// compact() 同款）再由任务排队执行。
    Compact { instructions: Option<String> },
    /// 压缩结束后按序投递压缩期间排队的消息（TS `flushCompactionQueue`）。
    FlushCompactionQueue { messages: Vec<(String, bool)>, will_retry: bool },
    /// Alt+Up（TS `app.message.dequeue`）取回队列文本后：同步清空会话层
    /// 队列镜像并发出 QueueUpdate（把 pending 显示归零）。
    SyncQueueMirrors,
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
    /// the lock is held for the whole run by `prompt()`).
    Abort,
    /// Esc 中断（参数为按下时编辑器里已有的文本）：先清掉 agent 队列再中止
    /// run，随后把排队 steering/follow-up 文本还原回编辑器（对齐 TS
    /// `restoreQueuedMessagesToEditor({ abort: true })`）。
    AbortAndClearQueues(String),
    /// Alt+Up 取回排队消息（TS `handleDequeue` →
    /// `restoreQueuedMessagesToEditor` 无 abort 变体）：清空 agent 队列 +
    /// 会话队列镜像，pending 显示同步归零。
    ClearAgentQueues,
    /// Esc 在压缩进行中（TS compaction_start 把 onEscape 换成
    /// `abortCompaction`）：无锁中止压缩请求本身，不碰 run。
    AbortCompaction,
    /// Esc 在自动重试退避中（TS auto_retry_start 把 onEscape 换成
    /// `abortRetry`）：无锁中止退避，`AutoRetryEnd(success=false)` 清 UI。
    AbortRetry,
    /// `/compact [instructions]`（TS `handleCompactCommand`）：先无锁 abort
    /// 在飞 run（TS compact() 同款）再由任务执行压缩。
    Compact { instructions: Option<String> },
    /// 按键时刻的 steering 入队（TS Enter-streaming →
    /// `prompt(text, { streamingBehavior: "steer" })` → `_queueSteer`）。
    /// 必须绕过 session 锁：命令任务在整个 run 期间持有锁，经通道排队会
    /// 排到 run 结束之后，is_streaming 判定与 pending 显示都会失效。
    QueueSteer(String),
    /// 按键时刻的 follow-up 入队（TS Alt+Enter →
    /// `prompt(text, { streamingBehavior: "followUp" })` →
    /// `_queueFollowUp`，等当前 run 结束后再作为新 prompt 运行）。
    QueueFollowUp(String),
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
    /// Names of `/skill:<name>` commands (snapshot taken at startup; used by
    /// `/help` to list available skills).
    skill_commands: Vec<String>,
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
    fn new(width: u16, height: u16, ext_commands: Vec<String>, skill_commands: Vec<String>) -> Self {
        let model = pi_tui::Model::new(width, height);
        Self {
            model,
            last_ctrl_c: Instant::now() - std::time::Duration::from_millis(DOUBLE_CTRL_C_WINDOW_MS + 100),
            last_esc: Instant::now() - std::time::Duration::from_millis(DOUBLE_CTRL_C_WINDOW_MS + 100),
            pending_cmds: Vec::new(),
            quit: false,
            ext_commands,
            skill_commands,
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
                || state.model.is_compacting
                || state.model.retry_status.is_some()
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
            // TS compaction_end → flushCompactionQueue / aborted 提示、
            // queueCompactionMessage → 状态提示，都在事件进入 pi-tui 前
            // 由宿主层处理（可能产生 effect / 额外系统消息）。
            let mut effects: Vec<Effect> = Vec::new();
            match msg {
                pi_tui::Msg::QueueCompactionMessage { text, follow_up } => {
                    app::update(
                        &mut state.model,
                        pi_tui::Msg::QueueCompactionMessage { text: text.clone(), follow_up },
                    );
                    // TS queueCompactionMessage：showStatus("Queued message
                    // for after compaction")（编辑器文本已由提交路径清空）。
                    state.model.push_message("system", "Queued message for after compaction");
                }
                pi_tui::Msg::CompactionEnd { will_retry, aborted, manual, error_message } => {
                    let error_display = error_message.clone();
                    app::update(
                        &mut state.model,
                        pi_tui::Msg::CompactionEnd { will_retry, aborted, manual, error_message },
                    );
                    // TS compaction_end：aborted → "Compaction cancelled" /
                    // "Auto-compaction cancelled"；错误 → showError / 错误
                    // 文本进聊天区。全部以系统消息呈现（TS showStatus/
                    // showError 均为转录区 dim/muted 行）。
                    if aborted {
                        state.model.push_message(
                            "system",
                            if manual { "Compaction cancelled" } else { "Auto-compaction cancelled" },
                        );
                    } else if let Some(err) = error_display {
                        state.model.push_message("system", err);
                    }
                    // 压缩结束后 flush 排队消息（TS flushCompactionQueue；
                    // aborted 时同样 flush）。
                    if !state.model.compaction_queue.is_empty() {
                        let messages = std::mem::take(&mut state.model.compaction_queue);
                        effects.push(Effect::AgentCommand(AgentCmd::FlushCompactionQueue {
                            messages,
                            will_retry,
                        }));
                    }
                }
                pi_tui::Msg::RetryCountdown { attempt, max_attempts, delay_ms, error_message, esc_aborts } => {
                    // TS summarization_retry_scheduled：先 showError 再显示
                    // 倒计时（auto_retry_start 无 error_message）。
                    if let Some(err) = &error_message {
                        state.model.push_message("system", err.clone());
                    }
                    app::update(
                        &mut state.model,
                        pi_tui::Msg::RetryCountdown { attempt, max_attempts, delay_ms, error_message, esc_aborts },
                    );
                }
                pi_tui::Msg::RetryEnd { success, attempt, final_error } => {
                    let error_display = final_error.clone();
                    app::update(
                        &mut state.model,
                        pi_tui::Msg::RetryEnd { success, attempt, final_error },
                    );
                    // TS auto_retry_end：仅最终失败时 showError（成功则正常
                    // 显示响应）。
                    if !success {
                        state.model.push_message(
                            "system",
                            format!(
                                "Retry failed after {attempt} attempts: {}",
                                error_display.unwrap_or_else(|| "Unknown error".to_string())
                            ),
                        );
                    }
                }
                pi_tui::Msg::RetryAttemptStart | pi_tui::Msg::RetryLoopEnd => {
                    app::update(&mut state.model, msg);
                }
                msg => {
                    app::update(&mut state.model, msg);
                }
            }
            UpdateOutcome { effects, redraw: true }
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
        // 1) 压缩进行中 → 中止压缩（TS compaction_start 把 onEscape 换成
        //    abortCompaction，优先于其他分支）
        // 2) 流式/运行中 → 中断 + 清排队 follow-up（TS restoreQueuedMessagesToEditor）
        // 3) bash 运行中 → abort bash
        // 4) 空编辑器双击 Esc → TS 弹 tree/fork 选择器（本 port 未实现，见
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
            // 自动重试退避中：TS auto_retry_start 把 onEscape 换成
            // abortRetry（优先于其他分支）。注意压缩/分支摘要的重试倒计时
            // （esc_aborts=false）不替换 onEscape —— Esc 仍走 abortCompaction。
            if state
                .model
                .retry_status
                .as_ref()
                .is_some_and(|r| r.esc_aborts)
            {
                return vec![Effect::AbortRetry];
            }
            if state.model.is_compacting {
                // 无锁中止压缩（压缩运行在持有 session 锁的任务里，命令通
                // 道中止会排在锁后面）。
                return vec![Effect::AbortCompaction];
            }
            if state.model.is_streaming || !state.model.active_tools.is_empty() {
                // Immediate UI feedback; the agent event stream then closes
                // via Done → MessageEnd → StreamEnd.
                state.model.is_streaming = false;
                // pending 区排队项随即从显示中移除（权威清空由
                // RestoreQueuedToEditor 里的 clear_all_queues 发出的
                // QueueUpdate 事件确认）。
                state.model.pending_steering.clear();
                state.model.pending_follow_up.clear();
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
        // Enter: submit message or slash command (Shift+Enter inserts a
        // newline — TS `tui.input.newLine`; Alt+Enter is the follow-up queue
        // keybinding, TS `app.message.followUp`). While the slash-command
        // menu is open, Enter picks the highlighted completion and runs it
        // (TS dropdown behavior).
        KeyCode::Enter => {
            if key.modifiers == KeyModifiers::SHIFT {
                app::update(&mut state.model, pi_tui::Msg::InputNewline);
                return vec![];
            }
            if key.modifiers == KeyModifiers::ALT {
                return handle_follow_up_key(state);
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
            let text = state.model.input.expanded_value();
            if text.is_empty() {
                return vec![];
            }
            submit_input(state, text, false)
        }
        // Alt+Up：TS `app.message.dequeue`（handleDequeue →
        // restoreQueuedMessagesToEditor）——取回全部排队消息合并进编辑器。
        KeyCode::Up if key.modifiers == KeyModifiers::ALT && !state.model.completer.visible => {
            // TS getAllQueuedMessages 顺序：session steering + compaction
            // steer，然后 session follow-up + compaction follow-up。
            let mut queued: Vec<String> = state.model.pending_steering.clone();
            queued.extend(
                state
                    .model
                    .compaction_queue
                    .iter()
                    .filter(|(_, follow_up)| !*follow_up)
                    .map(|(text, _)| text.clone()),
            );
            queued.extend(state.model.pending_follow_up.clone());
            queued.extend(
                state
                    .model
                    .compaction_queue
                    .iter()
                    .filter(|(_, follow_up)| *follow_up)
                    .map(|(text, _)| text.clone()),
            );
            let count = queued.len();
            let current = state.model.input.value().to_string();
            // TS clearAllQueues：清掉 TUI 层队列镜像；会话层队列由
            // ClearAgentQueues 同步清空（随后的 QueueUpdate 事件把 pending
            // 显示归零）。
            state.model.pending_steering.clear();
            state.model.pending_follow_up.clear();
            state.model.compaction_queue.clear();
            if count == 0 {
                // TS handleDequeue：showStatus("No queued messages to restore")。
                state.model.push_message("system", "No queued messages to restore");
                return vec![];
            }
            // TS restoreQueuedMessagesToEditor：`[queuedText,
            // currentText].filter(trim).join("\n\n")`。
            let mut parts: Vec<String> =
                queued.into_iter().filter(|t| !t.trim().is_empty()).collect();
            if !current.trim().is_empty() {
                parts.push(current);
            }
            let combined = parts.join("\n\n");
            app::update(&mut state.model, pi_tui::Msg::SetEditorText(combined));
            state.model.push_message(
                "system",
                format!(
                    "Restored {count} queued message{plural} to editor",
                    plural = if count > 1 { "s" } else { "" }
                ),
            );
            vec![Effect::ClearAgentQueues]
        }
        _ => {
            let cmds = app::update(&mut state.model, pi_tui::Msg::Key(key));
            state.pending_cmds.extend(cmds);
            vec![]
        }
    }
}

/// 提交路径（TS `handleSubmit` 尾部 + `handleFollowUp` 的 isStreaming/
/// isCompacting 分支）：
/// - 压缩中 → 入 TUI 层 compaction 队列（压缩结束 flush，TS
///   `queueCompactionMessage`；扩展命令已由 slash_command 提前路由立即
///   执行）；
/// - streaming → 按键时刻直接入队：steering（Enter）或 follow-up
///   （Alt+Enter），对齐 TS `prompt(text, { streamingBehavior })` ——
///   经 `Effect::QueueSteer/QueueFollowUp` 绕过 session 锁（命令任务在
///   整个 run 期间持有锁，经通道会排到 run 结束后，判定与 pending 显示
///   都会失效）；
/// - 空闲 → 新 run。
///
/// 输入框总是清空；用户气泡不由提交路径直接绘制——bridge 把 agent 的
/// `MessageStart(User)` 转成聊天区消息（TS `message_start` 语义：排队中
/// 的消息只出现在 pending 区，被消费后才进转录）。
fn submit_message(state: &mut AppState, text: String, follow_up: bool) -> Vec<Effect> {
    state.model.input.clear();
    if state.model.is_compacting {
        // TS queueCompactionMessage：入 compaction 队列 + 状态提示
        // （压缩结束后 CompactionEnd → flushCompactionQueue 投递）。
        state.model.compaction_queue.push((text, follow_up));
        state.model.push_message("system", "Queued message for after compaction");
        return vec![];
    }
    if state.model.is_streaming {
        // TS：排队中的消息只进 pending 区（TS updatePendingMessagesDisplay），
        // 镜像与 agent 队列入队由 effect 完成。
        if follow_up {
            state.model.pending_follow_up.push(text.clone());
            return vec![Effect::QueueFollowUp(text)];
        }
        state.model.pending_steering.push(text.clone());
        return vec![Effect::QueueSteer(text)];
    }
    state.model.is_streaming = true;
    vec![Effect::AgentCommand(AgentCmd::SendMessage(text))]
}

/// TS `isExtensionCommand`（按键时刻判定，使用启动时的命令快照）：压缩/
/// streaming 中扩展命令必须立即执行，而命令任务在 run 期间不可达。
fn is_ext_command_snapshot(state: &AppState, text: &str) -> bool {
    if !text.starts_with('/') {
        return false;
    }
    let name = match text.find(' ') {
        Some(idx) => &text[1..idx],
        None => &text[1..],
    };
    state.ext_commands.iter().any(|c| c == name)
}

/// Enter 尾部提交路径：slash 命令路由（builtins/扩展命令在 streaming/
/// 压缩中同样立即执行，对齐 TS onSubmit 顺序）+ 普通消息。
fn submit_input(state: &mut AppState, text: String, from_follow_up: bool) -> Vec<Effect> {
    if text.starts_with('/') {
        return slash_command(state, &text);
    }
    submit_message(state, text, from_follow_up)
}

/// Alt+Enter（TS `handleFollowUp` / `app.message.followUp`）：
/// - 压缩中：入 compaction 队列（followUp 模式）；扩展命令立即执行
/// - streaming：排 follow-up 队列（等当前 run 结束后再作为新 prompt）
/// - 空闲：等同 Enter（完整提交路径，含命令路由）。
fn handle_follow_up_key(state: &mut AppState) -> Vec<Effect> {
    // TS getExpandedText：粘贴 marker 展开（不做补全应用）。
    let text = state.model.input.expanded_value();
    let text = text.trim().to_string();
    if text.is_empty() {
        return vec![];
    }
    if state.model.is_compacting || state.model.is_streaming {
        // TS handleFollowUp → session.prompt(followUp)：prompt 先执行扩展
        // 命令再判 streaming —— 扩展命令立即执行；其余（含内建/未知命令
        // 文本）排队。
        if is_ext_command_snapshot(state, &text) {
            return slash_command(state, &text);
        }
        return submit_message(state, text, true);
    }
    // TS：空闲时 `this.editor.onSubmit(text)`。
    submit_input(state, text, true)
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
            // TS renderCurrentSessionState：会话重建/切换时清空 pending
            // 区与 compaction 队列。
            state.model.pending_steering.clear();
            state.model.pending_follow_up.clear();
            state.model.compaction_queue.clear();
            state.model.is_compacting = false;
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
        "/compact" => {
            // TS handleCompactCommand：错误/结果经 CompactionEnd 事件回显
            //（TUI 显示 + flush 队列）；先 abort 在飞 run（TS compact() 同
            // 款），压缩任务在 run 结束后执行。
            let instructions = if args.is_empty() { None } else { Some(args.to_string()) };
            vec![Effect::Compact { instructions }]
        }
        "/help" => {
            let mut help = "Commands: /new, /name <name>, /model <provider>/<id>, /compact [instructions], /theme [dark|light], /help, /quit".to_string();
            if !state.ext_commands.is_empty() {
                help.push_str(&format!("\nExtension: /{}", state.ext_commands.join(", /")));
            }
            if !state.skill_commands.is_empty() {
                help.push_str(&format!("\nSkills: /{}", state.skill_commands.join(", /")));
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
                // Unknown command → sent as a regular message (TS
                // onSubmit tail). Queued per streaming/compaction state,
                // matching the TS prompt(steer) routing.
                submit_message(state, text.to_string(), false)
            }
        }
    }
}

// ============================================================================
// effects executor (grok `app/effects.rs`)
// ============================================================================

/// Lock-free queue mirror handles（与 session 镜像共享同一 Arc）：按键时刻
/// 的排队必须绕过 session 锁（命令任务整个 run 期间持有锁），同时保持
/// 消费匹配与 QueueUpdate 事件准确。
#[derive(Clone)]
struct QueueMirrors {
    steering: Arc<std::sync::Mutex<Vec<String>>>,
    follow_up: Arc<std::sync::Mutex<Vec<String>>>,
}

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
/// 内建命令（`/model` 带模型参数补全）+ prompt template + 扩展命令
/// + skill 命令（受 `enableSkillCommands` 控制）。
fn build_completion_commands(session: &AgentSession) -> Vec<pi_tui::CompletionCommand> {
    let mut commands = vec![
        pi_tui::CompletionCommand::new("/help", "Show commands", "help"),
        pi_tui::CompletionCommand::new("/new", "Start a new session", "new"),
        pi_tui::CompletionCommand::new("/name <name>", "Set the session name", "name"),
        pi_tui::CompletionCommand::new("/compact [instructions]", "Manually compact the session context", "compact"),
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
    // Prompt template 命令（对齐 TS `templateCommands`）。
    commands.extend(template_completion_commands(&session.prompt_templates()));
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
    // Skill 命令（对齐 TS `skillCommandList`，受 `enableSkillCommands` 控制，默认 true）。
    if session.get_enable_skill_commands() {
        if let Some(resources) = session.resource_loader() {
            commands.extend(skill_completion_commands(&resources.skills));
        }
    }
    commands
}

/// Prompt template → 补全命令（对齐 TS `templateCommands`）。
fn template_completion_commands(templates: &[crate::core::prompt_templates::PromptTemplate]) -> Vec<pi_tui::CompletionCommand> {
    templates
        .iter()
        .map(|t| {
            pi_tui::CompletionCommand::new(
                format!("/{}", t.name),
                t.description.clone(),
                t.name.clone(),
            )
        })
        .collect()
}

/// Skill → `/skill:<name>` 补全命令（对齐 TS `skillCommandList`）。
fn skill_completion_commands(skills: &[crate::core::skills::Skill]) -> Vec<pi_tui::CompletionCommand> {
    skills
        .iter()
        .map(|s| {
            let name = format!("skill:{}", s.name);
            pi_tui::CompletionCommand::new(format!("/{name}"), s.description.clone(), name)
        })
        .collect()
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
    compaction_abort: &crate::core::agent_session::CompactionAbortHandle,
    retry_abort: &crate::core::agent_session::RetryAbortHandle,
    mirrors: &QueueMirrors,
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
        Effect::ClearAgentQueues => {
            // TS handleDequeue → clearAllQueues 的会话层部分：清空 agent
            // 队列（编辑器还原已在按键处理里用镜像文本完成），镜像由
            // SyncQueueMirrors 清空并发 QueueUpdate（pending 显示归零）。
            agent_handle.clear_all_queues().await;
            let _ = cmd_tx.send(AgentCmd::SyncQueueMirrors);
        }
        Effect::AbortCompaction => {
            // TS compaction_start 把 onEscape 换成 abortCompaction：仅中止
            // 压缩请求本身，不碰当前 run；压缩结束的 CompactionEnd 事件随
            // 后触发状态提示与队列 flush。
            compaction_abort.abort();
        }
        Effect::AbortRetry => {
            // TS auto_retry_start 把 onEscape 换成 abortRetry：中止���避，
            // `_prepare_retry` 的 watch 分支发 AutoRetryEnd(success=false)
            // 清倒计时并提示 Retry cancelled。
            retry_abort.abort();
        }
        Effect::Compact { instructions } => {
            // TS handleCompactCommand → session.compact()：compact() 先
            // abort 在飞 run（TS _disconnectFromAgent + abort），这里同样
            // 先无锁中止再排队压缩任务。错误经 CompactionEnd 事件回显
            //（TS 忽略返回值）。
            agent_handle.abort().await;
            let _ = cmd_tx.send(AgentCmd::Compact { instructions });
        }
        Effect::QueueSteer(text) => {
            enqueue_queued(agent_handle, mirrors, cmd_tx, text, true).await;
        }
        Effect::QueueFollowUp(text) => {
            enqueue_queued(agent_handle, mirrors, cmd_tx, text, false).await;
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

/// TS `_queueSteer`/`_queueFollowUp`：镜像推入 + agent 队列入队。全部绕过
/// session 锁（命令任务整个 run 期间持锁，按键时刻的排队必须即时生效，
/// 否则 is_streaming 判定与 pending 显示都会失效）。按键到 effect 执行之
/// 间 run 可能已结束：此时无人消费队列，触发 settled 续跑。
async fn enqueue_queued(
    agent_handle: &Arc<Agent>,
    mirrors: &QueueMirrors,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AgentCmd>,
    text: String,
    steer: bool,
) {
    let timestamp = chrono::Utc::now().timestamp_millis();
    let message = pi_agent_core::types::AgentMessage::User {
        content: vec![pi_agent_core::pi_ai_types::ContentBlock::text(text.clone())],
        timestamp,
    };
    {
        let mut mirror = if steer {
            mirrors.steering.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
        } else {
            mirrors.follow_up.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        mirror.push(text);
    }
    if steer {
        agent_handle.steer(message).await;
    } else {
        agent_handle.follow_up(message).await;
    }
    if !agent_handle.state().await.is_streaming {
        let _ = cmd_tx.send(AgentCmd::RunSettledContinuations);
    }
}

// ============================================================================
// Agent command background task (session-locked commands)
// ============================================================================

/// TS `isExtensionCommand`：`/name`（可带参数）命中扩展注册的命令。
fn is_extension_command(sess: &AgentSession, text: &str) -> bool {
    if !text.starts_with('/') {
        return false;
    }
    let command_name = match text.find(' ') {
        Some(idx) => &text[1..idx],
        None => &text[1..],
    };
    sess.get_extension_registry()
        .map(|r| r.commands().iter().any(|c| c.name == command_name))
        .unwrap_or(false)
}

/// 以默认选项执行一次提交（扩展命令立即执行路径；错误回显到 UI）。
async fn run_prompt_default(sess: &AgentSession, result_tx: &tokio::sync::mpsc::UnboundedSender<pi_tui::Msg>, text: &str) {
    let result = sess
        .prompt(
            text,
            Some(crate::core::agent_session::PromptOptions {
                expand_prompt_templates: Some(true),
                source: Some("interactive".into()),
                ..Default::default()
            }),
        )
        .await;
    if let Err(e) = result {
        let _ = result_tx.send(pi_tui::Msg::NewMessage("system".into(), e));
    }
}

/// TS `flushCompactionQueue`：压缩结束后按序投递压缩期间排队的消息。
///
/// - willRetry 且仍在 streaming：全部按模式入 steer/followUp 队列（重试
///   回合消费；扩展命令立即执行）。
/// - 否则：扩展命令先立即执行；第一条非命令消息作为新 prompt（带其模
///   式）；其余按模式入 steer/followUp。
///
/// Rust `prompt()` 会阻塞整个 run，所以空闲时先排队剩余消息（run 的首个
/// turn 边界消费 steering 队列；follow-up 由 run 结束后的 settled 循环消
/// 费）再启动第一条 prompt —— 与 TS "prompt 启动 + 其余排队" 等价。任一
/// 步失败：清会话队列，全部消息还原回 TUI compaction 队列并提示（TS
/// `restoreQueue`）。
async fn flush_compaction_queue(
    sess: &AgentSession,
    result_tx: &tokio::sync::mpsc::UnboundedSender<pi_tui::Msg>,
    messages: Vec<(String, bool)>,
    will_retry: bool,
) {
    if messages.is_empty() {
        return;
    }

    let streaming = sess.is_streaming().await;

    let restore_queue = |error: String| {
        // TS restoreQueue：清会话队列，全部消息还原回 compaction 队列
        // （逆序回填保持原顺序），并提示失败原因。
        for (text, follow_up) in messages.iter().rev() {
            let _ = result_tx.send(pi_tui::Msg::QueueCompactionMessage {
                text: text.clone(),
                follow_up: *follow_up,
            });
        }
        let _ = result_tx.send(pi_tui::Msg::NewMessage(
            "system".into(),
            format!(
                "Failed to send queued message{}: {error}",
                if messages.len() > 1 { "s" } else { "" }
            ),
        ));
    };

    if will_retry && streaming {
        for (text, follow_up) in &messages {
            let outcome = if is_extension_command(sess, text) {
                Some(sess
                    .prompt(
                        text,
                        Some(crate::core::agent_session::PromptOptions {
                            expand_prompt_templates: Some(true),
                            source: Some("interactive".into()),
                            ..Default::default()
                        }),
                    )
                    .await)
            } else if *follow_up {
                let _ = sess.follow_up(text, None).await;
                None
            } else {
                let _ = sess.steer(text, None).await;
                None
            };
            if let Some(Err(e)) = outcome {
                let _ = sess.clear_all_queues().await;
                restore_queue(e);
                return;
            }
        }
        return;
    }

    // 第一条非扩展命令消息作为新 prompt（TS firstPromptIndex）。
    let first_prompt_index = messages
        .iter()
        .position(|(text, _)| !is_extension_command(sess, text));
    let Some(first_prompt_index) = first_prompt_index else {
        // 全部是扩展命令：逐个立即执行（TS：execute them all）。
        for (text, _) in &messages {
            if sess
                .prompt(
                    text,
                    Some(crate::core::agent_session::PromptOptions {
                        expand_prompt_templates: Some(true),
                        source: Some("interactive".into()),
                        ..Default::default()
                    }),
                )
                .await
                .is_err()
            {
                let _ = sess.clear_all_queues().await;
                restore_queue("prompt failed".to_string());
                return;
            }
        }
        return;
    };

    for (text, _) in &messages[..first_prompt_index] {
        if sess
            .prompt(
                text,
                Some(crate::core::agent_session::PromptOptions {
                    expand_prompt_templates: Some(true),
                    source: Some("interactive".into()),
                    ..Default::default()
                }),
            )
            .await
            .is_err()
        {
            let _ = sess.clear_all_queues().await;
            restore_queue("prompt failed".to_string());
            return;
        }
    }

    let (first_text, first_mode) = &messages[first_prompt_index];
    let rest = &messages[first_prompt_index + 1..];
    if !streaming {
        // 空闲：先处理剩余消息，再启动第一条 prompt（见上方说明）。
        // 扩展命令立即执行（TS flushCompactionQueue 对 rest 的 ext 分支）。
        for (text, follow_up) in rest {
            if is_extension_command(sess, text) {
                run_prompt_default(sess, result_tx, text).await;
            } else if *follow_up {
                let _ = sess.follow_up(text, None).await;
            } else {
                let _ = sess.steer(text, None).await;
            }
        }
    }
    let behavior = if *first_mode { "followUp" } else { "steer" };
    let prompt_result = sess
        .prompt(
            first_text,
            Some(crate::core::agent_session::PromptOptions {
                expand_prompt_templates: Some(true),
                source: Some("interactive".into()),
                streaming_behavior: Some(behavior.into()),
                ..Default::default()
            }),
        )
        .await;
    if let Err(e) = prompt_result {
        let _ = sess.clear_all_queues().await;
        restore_queue(e);
        return;
    }
    if streaming {
        // 已在 streaming：prompt(first) 实际按模式入队并立即返回，剩余
        // 消息此刻排队（顺序保持）。
        for (text, follow_up) in rest {
            if is_extension_command(sess, text) {
                run_prompt_default(sess, result_tx, text).await;
            } else if *follow_up {
                let _ = sess.follow_up(text, None).await;
            } else {
                let _ = sess.steer(text, None).await;
            }
        }
    }
}

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
                        AgentCmd::SendMessage(text) => {
                            // TUI 也走 session.prompt()（与 ACP/print/RPC 一致），而不是
                            // 一条 Rust 专属的轻量路径。prompt() 内部做扩展命令执行、
                            // skill/template 展开、streaming 排队、compact 检查、
                            // post-agent-run 重试与 /goal 自动延续，并返回 Result
                            //（可把错误回显到 UI），还正确清理 is_agent_run_active。
                            let result = sess
                                .prompt(
                                    &text,
                                    Some(crate::core::agent_session::PromptOptions {
                                        expand_prompt_templates: Some(true),
                                        source: Some("interactive".into()),
                                        ..Default::default()
                                    }),
                                )
                                .await;
                            if let Err(e) = result {
                                let _ = result_tx.send(pi_tui::Msg::NewMessage(
                                    "system".into(),
                                    e,
                                ));
                            }
                        }
                        AgentCmd::RunSettledContinuations => {
                            // 排队消息在无活动 run 时入队（按键到 effect 执行
                            // 之间的窗口）：触发 settled 续跑消费队列（对齐
                            // ExtensionCommand 尾部的同款调用）。
                            sess.run_settled_continuations().await;
                        }
                        AgentCmd::Compact { instructions } => {
                            // TS handleCompactCommand：忽略返回值，错误/结果
                            // 经 CompactionEnd 事件回显（TUI 显示 + flush）。
                            let _ = sess.compact(instructions.as_deref()).await;
                        }
                        AgentCmd::FlushCompactionQueue { messages, will_retry } => {
                            flush_compaction_queue(&sess, &result_tx, messages, will_retry).await;
                        }
                        AgentCmd::SyncQueueMirrors => {
                            // TS clearAllQueues 的会话层部分：清空队列镜像 +
                            // agent 队列并发出 QueueUpdate（pending 显示归零）。
                            let _ = sess.clear_all_queues().await;
                        }
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
                BE::UserMessage(text) => vec![pi_tui::Msg::NewMessage("user".into(), text)],
                BE::AgentRunStart => vec![pi_tui::Msg::AgentStart],
                BE::AgentRunEnd => vec![pi_tui::Msg::AgentEnd],
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
    let (ext_commands, skill_commands, initial_model_name, cwd) = {
        let cmds = session
            .get_extension_registry()
            .map(|r| r.commands().iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();
        let skills = if session.get_enable_skill_commands() {
            session
                .resource_loader()
                .map(|r| r.skills.iter().map(|s| format!("skill:{}", s.name)).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let model_id = session.get_state().await.model.id.clone();
        let cwd = session.get_cwd().to_string();
        (cmds, skills, model_id, cwd)
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

    let mut state = AppState::new(cols, rows, ext_commands, skill_commands);

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
    // ── 会话事件订阅（对齐 TS session.subscribe）：queue_update → pending
    // 显示；compaction_start/end → 压缩状态 + 队列 flush 触发。 ──
    // 无锁压缩中止句柄：压缩运行在持有 session 锁的任务里，命令通道中止
    // 会排在锁后面；句柄只碰 std Mutex，Esc 可即时中止（TS abortCompaction）。
    let compaction_abort = session.lock().await.compaction_abort_handle();
    let retry_abort = session.lock().await.retry_abort_handle();
    let queue_mirrors = {
        let (steering, follow_up) = session.lock().await.queue_mirror_handles();
        QueueMirrors {
            steering,
            follow_up,
        }
    };
    {
        let ev_tx = result_tx.clone();
        session
            .lock()
            .await
            .subscribe_session_events(std::sync::Arc::new(move |event| {
                let msg = match event {
                    AgentSessionEvent::QueueUpdate { steering, follow_up } => {
                        Some(pi_tui::Msg::SetPendingQueues { steering, follow_up })
                    }
                    AgentSessionEvent::CompactionStart { reason } => Some(pi_tui::Msg::CompactionStart {
                        manual: matches!(reason, CompactionReason::Manual),
                        overflow: matches!(reason, CompactionReason::Overflow),
                    }),
                    AgentSessionEvent::CompactionEnd {
                        reason,
                        will_retry,
                        aborted,
                        error_message,
                        ..
                    } => Some(pi_tui::Msg::CompactionEnd {
                        will_retry,
                        aborted,
                        manual: matches!(reason, CompactionReason::Manual),
                        error_message,
                    }),
                    AgentSessionEvent::AutoRetryStart {
                        attempt,
                        max_attempts,
                        delay_ms,
                        ..
                    } => Some(pi_tui::Msg::RetryCountdown {
                        attempt,
                        max_attempts,
                        delay_ms,
                        error_message: None,
                        esc_aborts: true,
                    }),
                    AgentSessionEvent::AutoRetryEnd {
                        success,
                        attempt,
                        final_error,
                        ..
                    } => Some(pi_tui::Msg::RetryEnd {
                        success,
                        attempt,
                        final_error,
                    }),
                    AgentSessionEvent::SummarizationRetryScheduled {
                        attempt,
                        max_attempts,
                        delay_ms,
                        error_message,
                        ..
                    } => Some(pi_tui::Msg::RetryCountdown {
                        attempt,
                        max_attempts,
                        delay_ms,
                        error_message: Some(error_message),
                        // TS summarization_retry_scheduled 不替换 onEscape：
                        // Esc 仍走 abortCompaction。
                        esc_aborts: false,
                    }),
                    AgentSessionEvent::SummarizationRetryAttemptStart { .. }
                    | AgentSessionEvent::SummarizationRetryFinished {} => {
                        Some(pi_tui::Msg::RetryAttemptStart)
                    }
                    _ => None,
                };
                if let Some(m) = msg {
                    let _ = ev_tx.send(m);
                }
            }));
    }
    let bg_session = session.clone();
    let bg_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_exit_flag = bg_exit.clone();
    spawn_agent_command_task(bg_session, cmd_rx, bg_exit_flag, result_tx.clone());

    // ── Subscribe agent events (lock-and-release) ───────────────────────
    // Keep an `Arc<Agent>` handle for abort: `abort()` is `&self` and must
    // NOT be routed through the session mutex — `prompt()` holds the
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
                    // Bracketed paste → editor `handle_paste` (TS `handlePaste`).
                    pi_tui::terminal::InputEvent::Paste(text) => {
                        Action::Agent(pi_tui::Msg::Paste(text))
                    }
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
            execute_effect(
                effect,
                &agent_handle,
                &compaction_abort,
                &retry_abort,
                &queue_mirrors,
                &cmd_tx,
                &completion_sources,
                &result_tx,
            )
            .await;
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
        AppState::new(120, 80, Vec::new(), Vec::new())
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

    /// skill 命令补全：每个 skill 生成 `/skill:<name>` 命令（对齐 TS
    /// `skillCommandList`），insert_text 为 `skill:<name>`（不带 `/`）。
    #[test]
    fn skill_completion_commands_build_skill_slash_commands() {
        use crate::core::skills::{create_skill_source_info, Skill};
        let skills = vec![
            Skill {
                name: "review".into(),
                description: "Review the diff".into(),
                file_path: "/tmp/review/SKILL.md".into(),
                base_dir: "/tmp/review".into(),
                source_info: create_skill_source_info("/tmp/review/SKILL.md", "/tmp/review", "user"),
                disable_model_invocation: false,
            },
            Skill {
                name: "test".into(),
                description: "Run tests".into(),
                file_path: "/tmp/test/SKILL.md".into(),
                base_dir: "/tmp/test".into(),
                source_info: create_skill_source_info("/tmp/test/SKILL.md", "/tmp/test", "user"),
                disable_model_invocation: false,
            },
        ];
        let cmds = skill_completion_commands(&skills);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].insert_text, "skill:review");
        assert_eq!(cmds[0].label, "/skill:review");
        assert_eq!(cmds[0].description, "Review the diff");
        assert_eq!(cmds[1].insert_text, "skill:test");
        assert_eq!(cmds[1].label, "/skill:test");
    }

    /// prompt template 命令补全：每个 template 生成 `/name` 命令（对齐 TS
    /// `templateCommands`）。
    #[test]
    fn template_completion_commands_build_template_slash_commands() {
        use crate::core::prompt_templates::{PromptSource, PromptTemplate};
        use pi_extension_api::source_info::create_source_info;
        let templates = vec![
            PromptTemplate {
                name: "fix".into(),
                description: "Fix the issue".into(),
                file_path: "/tmp/fix.md".into(),
                source: PromptSource::User,
                append: false,
                source_info: create_source_info(
                    "/tmp/fix.md".into(),
                    "local".into(),
                    pi_extension_api::source_info::SourceScope::User,
                    pi_extension_api::source_info::SourceOrigin::TopLevel,
                    None,
                ),
            },
        ];
        let cmds = template_completion_commands(&templates);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].insert_text, "fix");
        assert_eq!(cmds[0].label, "/fix");
        assert_eq!(cmds[0].description, "Fix the issue");
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
    // 消息排队对齐（TS handleSubmit / handleFollowUp / handleDequeue /
    // queueCompactionMessage / flushCompactionQueue）
    // ============================================================

    fn key(code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, mods)
    }

    use crossterm::event::KeyModifiers as KM;

    /// Alt+Enter streaming：按键时刻排 follow-up 队列（TS
    /// prompt(followUp)），不推用户气泡（气泡由 bridge 的
    /// MessageStart(User) 画）。
    #[test]
    fn alt_enter_while_streaming_queues_follow_up() {
        let mut s = state();
        s.model.is_streaming = true;
        s.model.input.set_value("later");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::ALT));
        assert_eq!(effects, vec![Effect::QueueFollowUp("later".into())]);
        assert_eq!(s.model.input.value(), "", "编辑器清空");
        assert!(s.model.is_streaming, "仍处于 streaming");
        assert_eq!(s.model.pending_follow_up, vec!["later"], "pending 区立即显示");
        assert!(s.model.messages.is_empty(), "排队消息不进气泡（TS：被消费后才进转录）");
    }

    /// Alt+Enter 空闲：等同 Enter（完整提交路径）。
    #[test]
    fn alt_enter_when_idle_submits_like_enter() {
        let mut s = state();
        s.model.input.set_value("hello");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::ALT));
        assert_eq!(effects, vec![Effect::AgentCommand(AgentCmd::SendMessage("hello".into()))]);
        assert!(s.model.is_streaming);
        assert_eq!(s.model.input.value(), "");
    }

    /// Alt+Enter 空闲 + 命令文本：走完整提交路径（TS onSubmit 含命令路由）。
    #[test]
    fn alt_enter_when_idle_routes_slash_commands() {
        let mut s = state();
        s.model.input.set_value("/name bot");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::ALT));
        assert_eq!(
            effects,
            vec![Effect::AgentCommand(AgentCmd::SetSessionName("bot".into()))]
        );
    }

    /// Alt+Enter 压缩中：入 compaction 队列（followUp 模式，TS
    /// queueCompactionMessage(text, "followUp")）。
    #[test]
    fn alt_enter_during_compaction_queues_compaction_message() {
        let mut s = state();
        s.model.is_compacting = true;
        s.model.input.set_value("after compaction");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::ALT));
        assert_eq!(effects, vec![]);
        assert_eq!(
            s.model.compaction_queue,
            vec![("after compaction".to_string(), true)]
        );
        assert_eq!(s.model.input.value(), "");
    }

    /// Enter streaming：按键时刻排 steering 队列（TS prompt(steer)），
    /// 不推气泡。
    #[test]
    fn enter_while_streaming_queues_steering_without_bubble() {
        let mut s = state();
        s.model.is_streaming = true;
        s.model.input.set_value("steer me");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::NONE));
        assert_eq!(effects, vec![Effect::QueueSteer("steer me".into())]);
        assert_eq!(s.model.input.value(), "");
        assert!(s.model.is_streaming);
        assert_eq!(s.model.pending_steering, vec!["steer me"]);
        assert!(s.model.messages.is_empty(), "排队消息不进气泡");
    }

    /// Enter 压缩中：入 compaction 队列（steer 模式，TS
    /// queueCompactionMessage(text, "steer")，压缩结束后 flush）。
    #[test]
    fn enter_during_compaction_queues_compaction_message() {
        let mut s = state();
        s.model.is_compacting = true;
        s.model.input.set_value("x");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::NONE));
        assert_eq!(effects, vec![], "压缩中入队是纯 UI 操作，无 effect");
        assert_eq!(s.model.compaction_queue, vec![("x".to_string(), false)]);
        assert!(s
            .model
            .messages
            .iter()
            .any(|m| m.role == "system" && m.text == "Queued message for after compaction"));
    }

    /// Enter 压缩中 + 扩展命令：立即执行（TS isCompacting 分支里
    /// isExtensionCommand → session.prompt 立即执行）。
    #[test]
    fn enter_during_compaction_runs_extension_command_immediately() {
        let mut s = state();
        s.model.is_compacting = true;
        s.ext_commands = vec!["goal".into()];
        s.model.input.set_value("/goal run");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::NONE));
        assert_eq!(
            effects,
            vec![Effect::AgentCommand(AgentCmd::ExtensionCommand("goal".into(), "run".into()))]
        );
    }

    /// Alt+Up：按 TS getAllQueuedMessages 顺序（session steering +
    /// compaction steer，然后 session follow-up + compaction follow-up）
    /// 取回全部排队文本合并进编辑器，并清空队列。
    #[test]
    fn alt_up_restores_all_queued_in_ts_order() {
        let mut s = state();
        s.model.pending_steering = vec!["a".into()];
        s.model.compaction_queue = vec![("c".into(), false), ("f".into(), true)];
        s.model.pending_follow_up = vec!["b".into()];
        s.model.input.set_value("draft");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Up, KM::ALT));
        assert_eq!(effects, vec![Effect::ClearAgentQueues]);
        // queuedText = "a\n\nc\n\nb\n\nf"；combined = [queuedText,
        // current].filter(trim).join("\n\n")。
        assert_eq!(s.model.input.value(), "a\n\nc\n\nb\n\nf\n\ndraft");
        assert!(s.model.pending_steering.is_empty());
        assert!(s.model.pending_follow_up.is_empty());
        assert!(s.model.compaction_queue.is_empty());
        assert!(s
            .model
            .messages
            .iter()
            .any(|m| m.role == "system" && m.text.contains("Restored 4 queued messages")));
    }

    /// Alt+Up 无排队：状态提示，不动编辑器，无 effect。
    #[test]
    fn alt_up_without_queued_shows_status_only() {
        let mut s = state();
        s.model.input.set_value("keep");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Up, KM::ALT));
        assert_eq!(effects, vec![]);
        assert_eq!(s.model.input.value(), "keep", "无排队时编辑器不动（TS 提前 return）");
        assert!(
            s.model
                .messages
                .iter()
                .any(|m| m.role == "system" && m.text == "No queued messages to restore")
        );
    }

    /// Shift+Enter 仍然是换行（TS tui.input.newLine）。
    #[test]
    fn shift_enter_inserts_newline() {
        let mut s = state();
        s.model.input.set_value("one");
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Enter, KM::SHIFT));
        assert_eq!(effects, vec![]);
        assert_eq!(s.model.input.value(), "one\n");
    }

    /// Esc 压缩中：中止压缩（TS compaction_start 把 onEscape 换成
    /// abortCompaction），优先于其他分支。
    #[test]
    fn esc_during_compaction_aborts_compaction() {
        let mut s = state();
        s.model.is_compacting = true;
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Esc, KM::NONE));
        assert_eq!(effects, vec![Effect::AbortCompaction]);
        assert!(!s.quit);
    }

    /// QueueUpdate 事件 → pending 显示数据更新（TS
    /// updatePendingMessagesDisplay）。
    #[test]
    fn queue_update_msg_updates_pending_display() {
        let mut s = state();
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::SetPendingQueues {
                steering: vec!["s1".into()],
                follow_up: vec!["f1".into()],
            }),
        );
        assert_eq!(s.model.pending_steering, vec!["s1"]);
        assert_eq!(s.model.pending_follow_up, vec!["f1"]);
    }

    /// QueueCompactionMessage 消息 → 入 compaction 队列 + 状态提示。
    #[test]
    fn queue_compaction_message_msg_enqueues_and_notifies() {
        let mut s = state();
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::QueueCompactionMessage {
                text: "held".into(),
                follow_up: false,
            }),
        );
        assert_eq!(s.model.compaction_queue, vec![("held".to_string(), false)]);
        assert!(s
            .model
            .messages
            .iter()
            .any(|m| m.role == "system" && m.text == "Queued message for after compaction"));
    }

    /// CompactionEnd → is_compacting 复位 + flush 排队消息（TS
    /// compaction_end → flushCompactionQueue；aborted 时也 flush）。
    #[test]
    fn compaction_end_flushes_compaction_queue() {
        let mut s = state();
        s.model.is_compacting = true;
        s.model.compaction_queue = vec![("x".into(), false)];
        let outcome = update(
            &mut s,
            Action::Agent(pi_tui::Msg::CompactionEnd {
                will_retry: false,
                aborted: false,
                manual: false,
                error_message: None,
            }),
        );
        assert!(!s.model.is_compacting);
        assert!(s.model.compaction_queue.is_empty());
        assert_eq!(
            outcome.effects,
            vec![Effect::AgentCommand(AgentCmd::FlushCompactionQueue {
                messages: vec![("x".to_string(), false)],
                will_retry: false,
            })]
        );
    }

    /// CompactionStart/End → 压缩状态标签数据（TS CompactionStatusIndicator）。
    #[test]
    fn compaction_start_end_toggle_indicator() {
        let mut s = state();
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::CompactionStart { manual: false, overflow: true }),
        );
        assert!(s.model.is_compacting);
        assert!(!s.model.compaction_manual);
        assert!(s.model.compaction_overflow);
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::CompactionEnd {
                will_retry: false,
                aborted: true,
                manual: false,
                error_message: None,
            }),
        );
        assert!(!s.model.is_compacting);
        assert!(s
            .model
            .messages
            .iter()
            .any(|m| m.role == "system" && m.text == "Auto-compaction cancelled"));
    }

    /// /new 清空 pending 显示与 compaction 队列（TS
    /// renderCurrentSessionState）。
    #[test]
    fn new_session_clears_pending_and_compaction_queues() {
        let mut s = state();
        s.model.pending_steering = vec!["a".into()];
        s.model.compaction_queue = vec![("c".into(), true)];
        s.model.is_compacting = true;
        slash_command(&mut s, "/new");
        assert!(s.model.pending_steering.is_empty());
        assert!(s.model.compaction_queue.is_empty());
        assert!(!s.model.is_compacting);
    }

    // ============================================================
    // /compact 与 provider 重试显示（TS handleCompactCommand /
    // RetryStatusIndicator / abortRetry）
    // ============================================================

    /// /compact：Effect::Compact（无参）。
    #[test]
    fn compact_command_routes_effect() {
        let mut s = state();
        let effects = slash_command(&mut s, "/compact");
        assert_eq!(effects, vec![Effect::Compact { instructions: None }]);
    }

    /// /compact 带自定义指令：Effect::Compact(Some(...))。
    #[test]
    fn compact_command_with_instructions() {
        let mut s = state();
        let effects = slash_command(&mut s, "/compact focus on the auth module");
        assert_eq!(
            effects,
            vec![Effect::Compact {
                instructions: Some("focus on the auth module".into()),
            }]
        );
    }

    /// /compact 在压缩中同样路由（TS 内建命令先于 isCompacting 分支）。
    #[test]
    fn compact_command_works_during_compaction() {
        let mut s = state();
        s.model.is_compacting = true;
        let effects = slash_command(&mut s, "/compact");
        assert_eq!(effects, vec![Effect::Compact { instructions: None }]);
    }

    /// 自动重试退避中：Esc = 中止重试（TS auto_retry_start 替换 onEscape）。
    #[test]
    fn esc_during_auto_retry_aborts_retry() {
        let mut s = state();
        s.model.retry_status = Some(pi_tui::app::RetryStatus {
            attempt: 1,
            max_attempts: 3,
            delay_ms: 2000,
            started_at: Instant::now(),
            esc_aborts: true,
        });
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Esc, KM::NONE));
        assert_eq!(effects, vec![Effect::AbortRetry]);
        assert!(!s.quit);
    }

    /// 压缩重试倒计时中：Esc 仍走 abortCompaction（TS
    /// summarization_retry_scheduled 不替换 onEscape）。
    #[test]
    fn esc_during_summarization_retry_aborts_compaction() {
        let mut s = state();
        s.model.is_compacting = true;
        s.model.retry_status = Some(pi_tui::app::RetryStatus {
            attempt: 2,
            max_attempts: 3,
            delay_ms: 4000,
            started_at: Instant::now(),
            esc_aborts: false,
        });
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Esc, KM::NONE));
        assert_eq!(effects, vec![Effect::AbortCompaction]);
    }

    /// AutoRetryStart → 倒计时状态；AutoRetryEnd → 清除；失败时错误提示
    /// （TS auto_retry_end 仅最终失败时 showError）。
    #[test]
    fn retry_events_drive_countdown_and_error() {
        let mut s = state();
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::RetryCountdown {
                attempt: 1,
                max_attempts: 3,
                delay_ms: 2000,
                error_message: None,
                esc_aborts: true,
            }),
        );
        let rs = s.model.retry_status.as_ref().expect("countdown set");
        assert_eq!(rs.attempt, 1);
        assert_eq!(rs.max_attempts, 3);
        assert!(rs.esc_aborts);
        assert!(s.model.messages.is_empty(), "auto_retry_start 不报错");

        // 失败：清除 + 错误提示（TS 文案）。
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::RetryEnd {
                success: false,
                attempt: 3,
                final_error: Some("boom".into()),
            }),
        );
        assert!(s.model.retry_status.is_none());
        assert!(s
            .model
            .messages
            .iter()
            .any(|m| m.role == "system" && m.text == "Retry failed after 3 attempts: boom"));

        // 成功：清除，无错误提示。
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::RetryCountdown {
                attempt: 1,
                max_attempts: 3,
                delay_ms: 2000,
                error_message: None,
                esc_aborts: true,
            }),
        );
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::RetryEnd { success: true, attempt: 1, final_error: None }),
        );
        assert!(s.model.retry_status.is_none());
    }

    /// 压缩重试倒计时：附带的错误先入转录（TS
    /// summarization_retry_scheduled → showError），Esc 不中止重试。
    #[test]
    fn summarization_retry_scheduled_shows_error_and_countdown() {
        let mut s = state();
        update(
            &mut s,
            Action::Agent(pi_tui::Msg::RetryCountdown {
                attempt: 2,
                max_attempts: 3,
                delay_ms: 4000,
                error_message: Some("Compaction failed: 503".into()),
                esc_aborts: false,
            }),
        );
        assert!(s.model.retry_status.is_some());
        assert!(s
            .model
            .messages
            .iter()
            .any(|m| m.role == "system" && m.text == "Compaction failed: 503"));
        // Esc：esc_aborts=false → 走压缩链（is_compacting 时 abortCompaction）。
        s.model.is_compacting = true;
        let effects = handle_key(&mut s, key(crossterm::event::KeyCode::Esc, KM::NONE));
        assert_eq!(effects, vec![Effect::AbortCompaction]);
    }

    /// 摘要重试 attempt_start / finished 清除倒计时（回落压缩指示器）。
    #[test]
    fn summarization_retry_attempt_start_clears_countdown() {
        let mut s = state();
        s.model.is_compacting = true;
        s.model.retry_status = Some(pi_tui::app::RetryStatus {
            attempt: 1,
            max_attempts: 3,
            delay_ms: 2000,
            started_at: Instant::now(),
            esc_aborts: false,
        });
        update(&mut s, Action::Agent(pi_tui::Msg::RetryAttemptStart));
        assert!(s.model.retry_status.is_none(), "倒计时清除，回落压缩标签");
        assert!(s.model.is_compacting);
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
