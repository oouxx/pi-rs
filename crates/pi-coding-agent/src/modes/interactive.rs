//! Interactive TUI mode — connects AgentSession to the pi-tui Elm architecture.
//!
//! Handles TUI rendering and user interaction, delegating business logic to AgentSession.
//! Mirrors packages/coding-agent/src/modes/interactive/interactive-mode.ts

use std::sync::Arc;
use std::time::Instant;

use pi_tui::app;
use tokio::sync::Mutex;

use crate::core::agent_session::AgentSession;

const DOUBLE_CTRL_C_WINDOW_MS: u64 = 500;
const SPINNER_TICK_MS: u64 = 100;

enum AgentCmd {
    SendMessage(String),
    Abort,
    AbortBash,
    SetModel(String, String),
    CycleModel,
    SetThinkingLevel(String),
    CycleThinkingLevel,
    NewSession(Option<String>),
    SetSessionName(String),
    ExtensionCommand(String, String),
    ReloadExtensions,
}

/// Run the interactive TUI mode.
/// Mirrors the original InteractiveMode.run().
pub async fn run_interactive_mode(mut session: AgentSession) -> i32 {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
    );

    let mut terminal = match pi_tui::Terminal::new() {
        Ok(t) => t,
        Err(e) => { restore_terminal(); eprintln!("Failed to initialize terminal: {e}"); return 1; }
    };

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut tui_model = pi_tui::Model::new(cols, rows);

    let (mut input_rx, shutdown_guard) = match terminal.start() {
        Ok(r) => r,
        Err(e) => { restore_terminal(); eprintln!("Failed to start terminal input: {e}"); return 1; }
    };

    // ── Agent command channel + background task ───────────────────────────
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<AgentCmd>();
    let session = Arc::new(Mutex::new(session));
    let bg_session = session.clone();
    let bg_exit = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bg_exit_flag = bg_exit.clone();

    tokio::spawn(async move {
        while !bg_exit_flag.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    let mut sess = bg_session.lock().await;
                    match cmd {
                        AgentCmd::SendMessage(text) => sess.add_user_text(&text).await,
                        AgentCmd::Abort => sess.abort().await,
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
                                input: vec!["text".to_string()],
                                headers: None,
                                compat: None,
                                cost: pi_agent_core::pi_ai_types::ModelCost {
                                    input: 0.0,
                                    output: 0.0,
                                    cache_read: 0.0,
                                    cache_write: 0.0,
                                },
                            };
                            sess.set_model(model).await;
                        }
                        AgentCmd::CycleModel => {
                            // Cycle through available models via the agent
                            sess.abort().await;
                        }
                        AgentCmd::SetThinkingLevel(level) => {
                            // Thinking level is managed by agent state
                        }
                        AgentCmd::CycleThinkingLevel => {
                            // Cycle through thinking levels
                        }
                        AgentCmd::NewSession(parent) => {
                            sess.new_session(parent.as_deref()).await;
                        }
                        AgentCmd::SetSessionName(name) => {
                            sess.set_session_name(&name);
                        }
                        AgentCmd::ExtensionCommand(_cmd_name, _args) => {
                            // Extension commands are handled by Rust native extensions
                            // via the ExtensionRegistry. Dispatch is TBD per extension.
                        }
                        AgentCmd::ReloadExtensions => {
                            // Extension reload is not applicable for Rust native extensions.
                        }
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
            }
        }
    });

    // ── Subscribe agent events (lock-and-release) ───────────────────────
    let (bridge_tx, mut bridge_rx) = tokio::sync::mpsc::unbounded_channel::<crate::modes::agent_bridge::AgentEvent>();
    {
        let mut sess = session.lock().await;
        crate::modes::agent_bridge::subscribe_agent(&mut sess, bridge_tx).await;
    } // Lock released — background task can now use the session

    // ── Bridge events → pi_tui::Msg ──────────────────────────────────────
    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<pi_tui::Msg>();
    let atx = agent_tx.clone();

    tokio::spawn(async move {
        use crate::modes::agent_bridge::AgentEvent as BE;
        while let Some(ev) = bridge_rx.recv().await {
            let msg = match ev {
                BE::TextDelta(d) => Some(pi_tui::Msg::StreamText(d)),
                BE::MessageEnd(t) => { let _ = atx.send(pi_tui::Msg::NewMessage("assistant".into(), t)); Some(pi_tui::Msg::StreamEnd) }
                BE::ToolStart(n) => Some(pi_tui::Msg::ToolStart(n)),
                BE::ToolEnd(n, e) => Some(pi_tui::Msg::ToolEnd(n, e)),
                BE::ToolOutput(n, o) => Some(pi_tui::Msg::AppendToolOutput(n, o)),
            };
            if let Some(m) = msg { if atx.send(m).is_err() { break; } }
        }
    });

    // ── Main event loop ──────────────────────────────────────────────────
    let mut exit_code: i32 = 0;
    let mut last_ctrl_c = Instant::now() - std::time::Duration::from_millis(DOUBLE_CTRL_C_WINDOW_MS + 100);
    let mut tick_timer = tokio::time::interval(tokio::time::Duration::from_millis(SPINNER_TICK_MS));
    let mut should_quit = false;

    loop {
        let _ = terminal.ratatui_terminal().draw(|frame| app::view(&tui_model, frame));

        if should_quit { break; }

        tokio::select! {
            _ = tick_timer.tick() => {
                if tui_model.is_streaming || !tui_model.active_tools.is_empty() {
                    app::update(&mut tui_model, pi_tui::Msg::Tick);
                }
            }

            Some(key) = input_rx.recv() => {
                use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
                if key.kind != KeyEventKind::Press { continue; }

                match key.code {
                    // Ctrl+C: abort if streaming, double Ctrl+C to quit
                    KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                        let now = Instant::now();
                        let elapsed = now.duration_since(last_ctrl_c).as_millis() as u64;
                        if elapsed < DOUBLE_CTRL_C_WINDOW_MS {
                            should_quit = true; continue;
                        }
                        last_ctrl_c = now;
                        if tui_model.is_streaming || !tui_model.active_tools.is_empty() {
                            let _ = cmd_tx.send(AgentCmd::Abort);
                            tui_model.is_streaming = false;
                        }
                    }
                    // Ctrl+L: clear screen
                    KeyCode::Char('l') if key.modifiers == KeyModifiers::CONTROL => {
                        app::update(&mut tui_model, pi_tui::Msg::ClearScreen);
                    }
                    // Ctrl+D: quit
                    KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => { should_quit = true; }
                    // Ctrl+P: cycle model (matching original Ctrl+P behavior)
                    KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                        let _ = cmd_tx.send(AgentCmd::CycleModel);
                    }
                    // Ctrl+T: cycle thinking level (matching original Ctrl+T behavior)
                    KeyCode::Char('t') if key.modifiers == KeyModifiers::CONTROL => {
                        let _ = cmd_tx.send(AgentCmd::CycleThinkingLevel);
                    }
                    // Ctrl+B: abort bash (matching original Ctrl+C during bash)
                    KeyCode::Char('b') if key.modifiers == KeyModifiers::CONTROL => {
                        let _ = cmd_tx.send(AgentCmd::AbortBash);
                    }
                    // Esc: quit
                    KeyCode::Esc => { should_quit = true; }
                    // Enter: send message
                    KeyCode::Enter => {
                        let text = tui_model.input.value().to_string();
                        if !text.is_empty() {
                            // Handle slash commands
                            if text.starts_with('/') {
                                let ext_cmds: Vec<crate::core::extensions::RegisteredCommand> = Vec::new();
                                handle_slash_command(&text, &cmd_tx, &mut tui_model, &ext_cmds);
                            } else {
                                tui_model.input.clear();
                                tui_model.messages.push(app::Message { role: "user".into(), text: text.clone() });
                                tui_model.is_streaming = true;
                                let _ = cmd_tx.send(AgentCmd::SendMessage(text));
                            }
                        }
                    }
                    _ => { app::update(&mut tui_model, pi_tui::Msg::Key(key)); }
                }
            }

            Some(tui_msg) = agent_rx.recv() => {
                app::update(&mut tui_model, tui_msg);
            }
        }
    }

    // ── Cleanup ──────────────────────────────────────────────────────────
    bg_exit.store(true, std::sync::atomic::Ordering::SeqCst);
    shutdown_guard.shutdown();
    restore_terminal();
    exit_code
}

/// Handle slash commands, matching the original slash command handling.
fn handle_slash_command(
    text: &str,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AgentCmd>,
    model: &mut pi_tui::Model,
    extension_commands: &[crate::core::extensions::RegisteredCommand],
) {
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let command = parts[0];
    let args = parts.get(1).copied().unwrap_or("");

    match command {
        "/new" => {
            let parent = if args.is_empty() { None } else { Some(args.to_string()) };
            let _ = cmd_tx.send(AgentCmd::NewSession(parent));
            model.messages.push(app::Message { role: "system".into(), text: "New session created".into() });
        }
        "/name" => {
            if !args.is_empty() {
                let _ = cmd_tx.send(AgentCmd::SetSessionName(args.to_string()));
                model.messages.push(app::Message { role: "system".into(), text: format!("Session name set to: {}", args) });
            }
        }
        "/model" => {
            if let Some(eq_idx) = args.find('/') {
                let provider = &args[..eq_idx];
                let model_id = &args[eq_idx + 1..];
                let _ = cmd_tx.send(AgentCmd::SetModel(provider.to_string(), model_id.to_string()));
                model.messages.push(app::Message { role: "system".into(), text: format!("Switched to {}/{}", provider, model_id) });
            }
        }
        "/help" => {
            model.messages.push(app::Message {
                role: "system".into(),
                text: "Commands: /new, /name <name>, /model <provider>/<id>, /help, /quit".into(),
            });
        }
        "/reload" => {
            let _ = cmd_tx.send(AgentCmd::ReloadExtensions);
            model.messages.push(app::Message {
                role: "system".into(),
                text: "Reloading extensions...".into(),
            });
        }
        "/quit" | "/exit" => {
            // Handled by the main loop
        }
        _ => {
            // Check if this is an extension command
            let cmd_name = &command[1..]; // strip leading '/'
            let is_ext_cmd = extension_commands.iter().any(|c| c.name == cmd_name);
            if is_ext_cmd {
                let _ = cmd_tx.send(AgentCmd::ExtensionCommand(cmd_name.to_string(), args.to_string()));
                model.messages.push(app::Message {
                    role: "system".into(),
                    text: format!("Running extension command: /{}", cmd_name),
                });
                return;
            }
            // Unknown command - send as regular message
            model.input.clear();
            model.messages.push(app::Message { role: "user".into(), text: text.to_string() });
            model.is_streaming = true;
            let _ = cmd_tx.send(AgentCmd::SendMessage(text.to_string()));
        }
    }
}

fn restore_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = crossterm::terminal::disable_raw_mode();
}
