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

const DOUBLE_CTRL_C_WINDOW_MS: u64 = 500;
const SPINNER_TICK_MS: u64 = 100;

// ============================================================================
// AgentCmd — commands that need the session mutex (executed by the
// background task, which holds the lock across an agent run)
// ============================================================================

#[derive(Debug, Clone)]
enum AgentCmd {
    SendMessage(String),
    AbortBash,
    SetModel(String, String),
    CycleModel,
    CycleThinkingLevel,
    NewSession(Option<String>),
    SetSessionName(String),
    ExtensionCommand(String, String),
    ReloadExtensions,
}

// ============================================================================
// Action — everything the event loop observes, normalized to one enum
// ============================================================================

enum Action {
    /// A pressed key (release events are filtered before dispatch).
    Key(crossterm::event::KeyEvent),
    /// Agent events already converted to pi-tui messages by the bridge.
    Agent(pi_tui::Msg),
    /// Spinner/streaming tick.
    Tick,
}

// ============================================================================
// Effect — side effects requested by `update`, executed by the event loop
// ============================================================================

enum Effect {
    /// Send a command to the agent background task (session-locked).
    AgentCommand(AgentCmd),
    /// Abort the running agent directly (never through the session mutex —
    /// the lock is held for the whole run by `add_user_text`).
    Abort,
}

// ============================================================================
// AppState — coordination state (owns the pi-tui model)
// ============================================================================

struct AppState {
    model: pi_tui::Model,
    last_ctrl_c: Instant,
    quit: bool,
}

impl AppState {
    fn new(width: u16, height: u16) -> Self {
        Self {
            model: pi_tui::Model::new(width, height),
            last_ctrl_c: Instant::now() - std::time::Duration::from_millis(DOUBLE_CTRL_C_WINDOW_MS + 100),
            quit: false,
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
            let active = state.model.is_streaming || !state.model.active_tools.is_empty();
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
            let effects = handle_key(state, key);
            UpdateOutcome { effects, redraw: true }
        }
    }
}

/// Key dispatch — pure: mutates model/state and returns effects.
/// Mirrors the key handling from TS `interactive-mode.ts` (Ctrl+C abort with
/// double-press quit, Ctrl+L clear, Ctrl+D/Esc quit, Ctrl+P/T/B commands,
/// Enter submits, slash commands).
fn handle_key(state: &mut AppState, key: crossterm::event::KeyEvent) -> Vec<Effect> {
    use crossterm::event::{KeyCode, KeyModifiers};

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
        // Ctrl+P: cycle model
        KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
            vec![Effect::AgentCommand(AgentCmd::CycleModel)]
        }
        // Ctrl+T: cycle thinking level
        KeyCode::Char('t') if key.modifiers == KeyModifiers::CONTROL => {
            vec![Effect::AgentCommand(AgentCmd::CycleThinkingLevel)]
        }
        // Ctrl+B: abort bash
        KeyCode::Char('b') if key.modifiers == KeyModifiers::CONTROL => {
            vec![Effect::AgentCommand(AgentCmd::AbortBash)]
        }
        // Esc: quit
        KeyCode::Esc => {
            state.quit = true;
            vec![]
        }
        // Enter: submit message or slash command
        KeyCode::Enter => {
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
            state.model.messages.push(app::Message { role: "user".into(), text: text.clone() });
            state.model.is_streaming = true;
            vec![Effect::AgentCommand(AgentCmd::SendMessage(text))]
        }
        _ => {
            app::update(&mut state.model, pi_tui::Msg::Key(key));
            vec![]
        }
    }
}

/// Slash-command handling (pure; mirrors `interactive-mode.ts`).
fn slash_command(state: &mut AppState, text: &str) -> Vec<Effect> {
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let command = parts[0];
    let args = parts.get(1).copied().unwrap_or("");

    let system = |state: &mut AppState, msg: String| {
        state.model.messages.push(app::Message { role: "system".into(), text: msg });
    };

    match command {
        "/new" => {
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
                system(state, format!("Switched to {provider}/{model_id}"));
                vec![Effect::AgentCommand(AgentCmd::SetModel(provider, model_id))]
            } else {
                vec![]
            }
        }
        "/help" => {
            system(state, "Commands: /new, /name <name>, /model <provider>/<id>, /help, /quit".into());
            vec![]
        }
        "/reload" => {
            system(state, "Reloading extensions...".into());
            vec![Effect::AgentCommand(AgentCmd::ReloadExtensions)]
        }
        "/quit" | "/exit" => {
            state.quit = true;
            vec![]
        }
        _ => {
            // Extension commands are registered via the extension registry;
            // with no extensions loaded, unknown commands fall through to a
            // regular message (matching the original behavior).
            let cmd_name = &command[1..];
            if is_extension_command(cmd_name) {
                system(state, format!("Running extension command: /{cmd_name}"));
                vec![Effect::AgentCommand(AgentCmd::ExtensionCommand(cmd_name.to_string(), args.to_string()))]
            } else {
                state.model.input.clear();
                state.model.messages.push(app::Message { role: "user".into(), text: text.to_string() });
                state.model.is_streaming = true;
                vec![Effect::AgentCommand(AgentCmd::SendMessage(text.to_string()))]
            }
        }
    }
}

/// Extension commands are discovered from the extension registry; with the
/// minimal TUI the registry is empty, so this is always false (the unknown
/// command falls back to a plain message).
fn is_extension_command(_cmd_name: &str) -> bool {
    false
}

// ============================================================================
// effects executor (grok `app/effects.rs`)
// ============================================================================

/// Run one effect. This is the only place that performs I/O / touches the
/// agent. `Abort` bypasses the session mutex deliberately (see [`Effect`]).
async fn execute_effect(
    effect: Effect,
    agent_handle: &Arc<Agent>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AgentCmd>,
) {
    match effect {
        Effect::Abort => {
            agent_handle.abort().await;
        }
        Effect::AgentCommand(cmd) => {
            let _ = cmd_tx.send(cmd);
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
                            // Model will be resolved by the session
                            let model = pi_agent_core::pi_ai_types::Model {
                                provider,
                                id: model_id,
                                name: String::new(),
                                api: String::new(),
                                base_url: String::new(),
                                context_window: 128000,
                                max_tokens: 16384,
                                reasoning: false,
                                thinking_level_map: None,
                                sampling_params: None,
                                input: vec!["text".to_string()],
                                headers: None,
                                compat: None,
                                cost: pi_agent_core::pi_ai_types::ModelCost {
                                    input: 0.0,
                                    output: 0.0,
                                    cache_read: 0.0,
                                    cache_write: 0.0,
                                    tiers: vec![],
                                },
                            };
                            let _ = sess.set_model(model).await;
                        }
                        AgentCmd::CycleModel => {
                            // Cycle through available models via the agent
                            sess.abort().await;
                        }
                        AgentCmd::CycleThinkingLevel => {
                            // Cycle through thinking levels
                        }
                        AgentCmd::NewSession(parent) => {
                            sess.session_mgr_new(parent.as_deref()).await;
                        }
                        AgentCmd::SetSessionName(name) => {
                            sess.set_session_name(&name);
                        }
                        AgentCmd::ExtensionCommand(_cmd_name, _args) => {
                            // Extension commands are handled by Rust native
                            // extensions via the ExtensionRegistry. Dispatch
                            // is TBD per extension.
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
                BE::MessageEnd(t) => {
                    let mut v = Vec::new();
                    if !assistant_stream_open && !t.is_empty() {
                        v.push(pi_tui::Msg::NewMessage("assistant".into(), t));
                    }
                    v.push(pi_tui::Msg::StreamEnd);
                    assistant_stream_open = false;
                    v
                }
                BE::ToolStart(n) => vec![pi_tui::Msg::ToolStart(n)],
                BE::ToolEnd(n, e) => vec![pi_tui::Msg::ToolEnd(n, e)],
                BE::ToolOutput(n, o) => vec![pi_tui::Msg::AppendToolOutput(n, o)],
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
pub async fn run_interactive_mode(session: AgentSession) -> i32 {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
    );

    let mut terminal = match pi_tui::Terminal::new() {
        Ok(t) => t,
        Err(e) => { restore_terminal(); eprintln!("Failed to initialize terminal: {e}"); return 1; }
    };

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut state = AppState::new(cols, rows);

    let (mut input_rx, shutdown_guard) = match terminal.start() {
        Ok(r) => r,
        Err(e) => { restore_terminal(); eprintln!("Failed to start terminal input: {e}"); return 1; }
    };

    // ── Agent command channel + background task ───────────────────────────
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<AgentCmd>();
    let session = Arc::new(Mutex::new(session));
    let bg_session = session.clone();
    let bg_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_exit_flag = bg_exit.clone();
    spawn_agent_command_task(bg_session, cmd_rx, bg_exit_flag);

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

    let mut agent_rx = spawn_agent_bridge_task(bridge_rx);

    // ── Event loop: Action → update → effects → execute ──────────────────
    // Event-driven rendering (Elm style): draw only when state changed.
    let _ = terminal.ratatui_terminal().draw(|frame| app::view(&state.model, frame));

    let mut tick_timer = tokio::time::interval(tokio::time::Duration::from_millis(SPINNER_TICK_MS));
    loop {
        if state.quit {
            break;
        }

        let action = tokio::select! {
            _ = tick_timer.tick() => Action::Tick,
            Some(key) = input_rx.recv() => {
                use crossterm::event::KeyEventKind;
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                Action::Key(key)
            }
            Some(msg) = agent_rx.recv() => Action::Agent(msg),
        };

        let outcome = update(&mut state, action);
        for effect in outcome.effects {
            execute_effect(effect, &agent_handle, &cmd_tx).await;
        }
        if state.quit {
            break;
        }
        if outcome.redraw {
            let _ = terminal.ratatui_terminal().draw(|frame| app::view(&state.model, frame));
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

fn restore_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = crossterm::terminal::disable_raw_mode();
}
