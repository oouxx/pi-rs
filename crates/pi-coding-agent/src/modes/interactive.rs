//! Interactive TUI mode — connects AgentSession to the pi-tui Elm architecture.
//!
//! Key bindings:
//! - Ctrl+C (while streaming) → abort current agent turn
//! - Ctrl+C twice quickly → exit
//! - Ctrl+D → exit
//! - Esc → exit

use std::sync::Arc;
use std::time::Instant;

use pi_agent_core::pi_ai_types::AssistantMessageEvent;
use pi_agent_core::types::AgentEvent;
use pi_tui::app;

use crate::core::agent_session::AgentSession;

/// Time window (ms) for double-press Ctrl+C to exit.
const DOUBLE_CTRL_C_WINDOW_MS: u64 = 500;
const SPINNER_TICK_MS: u64 = 100;

/// Run the interactive TUI mode.
pub async fn run_interactive_mode(mut session: AgentSession) -> i32 {
    // ── Enter alternate screen ──────────────────────────────────────────
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
    );

    // ── Setup TUI ──────────────────────────────────────────────────────
    let mut terminal = match pi_tui::Terminal::new() {
        Ok(t) => t,
        Err(e) => {
            restore_terminal();
            eprintln!("Failed to initialize terminal: {e}");
            return 1;
        }
    };

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut tui_model = pi_tui::Model::new(cols, rows);

    let (mut input_rx, shutdown_guard) = match terminal.start() {
        Ok(r) => r,
        Err(e) => {
            restore_terminal();
            eprintln!("Failed to start terminal input: {e}");
            return 1;
        }
    };

    // ── Channel for agent events → TUI messages ────────────────────────
    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<pi_tui::Msg>();

    let agent_tx_clone = agent_tx.clone();
    let listener: Arc<dyn Fn(AgentEvent, Option<tokio::sync::watch::Receiver<bool>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync> =
        Arc::new(move |event, _signal| {
            let tx = agent_tx_clone.clone();
            Box::pin(async move {
                match &event {
                    AgentEvent::MessageUpdate { assistant_message_event, .. } => {
                        if let AssistantMessageEvent::TextDelta { delta, .. } = assistant_message_event {
                            let _ = tx.send(pi_tui::Msg::StreamText(delta.clone()));
                        }
                    }
                    AgentEvent::MessageEnd { message: msg } => {
                        if let pi_agent_core::types::AgentMessage::Assistant { content, .. } = msg {
                            let text: String = content.iter()
                                .filter_map(|b| if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = b { Some(text.clone()) } else { None })
                                .collect();
                            if !text.is_empty() {
                                // Create a new message via NewMessage
                                let _ = tx.send(pi_tui::Msg::NewMessage("assistant".into(), text));
                            }
                            let _ = tx.send(pi_tui::Msg::StreamEnd);
                        }
                    }
                    AgentEvent::ToolExecutionStart { tool_name, .. } => {
                        let _ = tx.send(pi_tui::Msg::ToolStart(tool_name.clone()));
                    }
                    AgentEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
                        let _ = tx.send(pi_tui::Msg::ToolEnd(tool_name.clone(), *is_error));
                    }
                    _ => {}
                }
            })
        });

    session.subscribe(listener).await;

    // ── Main event loop ────────────────────────────────────────────────
    let mut pending_message = String::new();
    let mut exit_code: i32 = 0;
    let mut last_ctrl_c = Instant::now() - std::time::Duration::from_millis(DOUBLE_CTRL_C_WINDOW_MS + 100);
    let mut tick_timer = tokio::time::interval(tokio::time::Duration::from_millis(SPINNER_TICK_MS));

    loop {
        // Render current TUI state
        let draw_result = terminal.ratatui_terminal().draw(|frame| {
            app::view(&tui_model, frame);
        });

        if let Err(e) = draw_result {
            eprintln!("Render error: {e}");
            exit_code = 1;
            break;
        }

        // Process events
        tokio::select! {
            // Tick timer for spinner animation
            _ = tick_timer.tick() => {
                if tui_model.is_streaming || !tui_model.active_tools.is_empty() {
                    app::update(&mut tui_model, pi_tui::Msg::Tick);
                }
                if !pending_message.is_empty() {
                    let msg = std::mem::take(&mut pending_message);
                    session.add_user_text(&msg).await;
                }
            }

            // TUI keyboard events
            Some(key) = input_rx.recv() => {
                use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                        let now = Instant::now();
                        let since_last = now.duration_since(last_ctrl_c).as_millis() as u64;

                        if since_last < DOUBLE_CTRL_C_WINDOW_MS {
                            exit_code = 0;
                            break;
                        }
                        last_ctrl_c = now;

                        if tui_model.is_streaming || !tui_model.active_tools.is_empty() {
                            session.abort().await;
                            tui_model.is_streaming = false;
                        }
                    }
                    KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
                        exit_code = 0;
                        break;
                    }
                    KeyCode::Esc => {
                        exit_code = 0;
                        break;
                    }
                    KeyCode::Enter => {
                        let text = tui_model.input.value().to_string();
                        if !text.is_empty() {
                            tui_model.input.clear();
                            tui_model.messages.push(app::Message {
                                role: "user".into(),
                                text: text.clone(),
                            });
                            tui_model.is_streaming = true;
                            pending_message = text;
                        }
                    }
                    _ => {
                        app::update(&mut tui_model, pi_tui::Msg::Key(key));
                    }
                }
            }

            // Agent response events (as pi_tui::Msg)
            Some(tui_msg) = agent_rx.recv() => {
                app::update(&mut tui_model, tui_msg);
            }
        }
    }

    // ── Cleanup ────────────────────────────────────────────────────────
    shutdown_guard.shutdown();
    restore_terminal();
    exit_code
}

/// Restore terminal to normal mode and leave alternate screen.
fn restore_terminal() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    let _ = crossterm::terminal::disable_raw_mode();
}
