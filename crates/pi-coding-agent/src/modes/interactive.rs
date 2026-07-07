//! Interactive TUI mode — connects AgentSession to the pi-tui Elm architecture.
//!
//! Uses a background task to own the AgentSession and process commands,
//! keeping the TUI event loop responsive to agent events.

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
}

/// Run the interactive TUI mode.
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
                    KeyCode::Char('l') if key.modifiers == KeyModifiers::CONTROL => {
                        app::update(&mut tui_model, pi_tui::Msg::ClearScreen);
                    }
                    KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => { should_quit = true; }
                    KeyCode::Esc => { should_quit = true; }
                    KeyCode::Enter => {
                        let text = tui_model.input.value().to_string();
                        if !text.is_empty() {
                            tui_model.input.clear();
                            tui_model.messages.push(app::Message { role: "user".into(), text: text.clone() });
                            tui_model.is_streaming = true;
                            let _ = cmd_tx.send(AgentCmd::SendMessage(text));
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

    bg_exit.store(true, std::sync::atomic::Ordering::SeqCst);
    shutdown_guard.shutdown();
    restore_terminal();
    exit_code
}

fn restore_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = crossterm::terminal::disable_raw_mode();
}
