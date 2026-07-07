//! Interactive TUI mode — connects AgentSession to the pi-tui Elm architecture.
//!
//! Uses two concurrent event loops:
//! - TUI events (keyboard input) → `pi_tui::Msg::Key`
//! - Agent events (streaming response) → `pi_tui::Msg::StreamText`

use std::sync::Arc;

use pi_agent_core::pi_ai_types::AssistantMessageEvent;
use pi_agent_core::types::AgentEvent;

use crate::core::agent_session::AgentSession;

/// Run the interactive TUI mode.
///
/// Creates an agent session and a TUI, then runs both event loops
/// concurrently, bridging user input to the agent and agent responses
/// to the TUI display.
pub async fn run_interactive_mode(mut session: AgentSession) -> i32 {
    // ── Setup TUI ──────────────────────────────────────────────────────
    let mut terminal = match pi_tui::Terminal::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to initialize terminal: {e}");
            return 1;
        }
    };

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut tui_model = pi_tui::Model::new(cols, rows);

    let (mut input_rx, shutdown_guard) = match terminal.start() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to start terminal input: {e}");
            return 1;
        }
    };

    // ── Channel for agent events → TUI ─────────────────────────────────
    let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel::<AgentOutput>();

    enum AgentOutput {
        TextDelta(String),
        MessageEnd(String),
        ToolStart(String),
        ToolEnd(String, bool),
    }

    // ── Subscribe to agent events ──────────────────────────────────────
    let agent_tx_clone = agent_tx.clone();
    let listener: Arc<dyn Fn(AgentEvent, Option<tokio::sync::watch::Receiver<bool>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync> =
        Arc::new(move |event, _signal| {
            let tx = agent_tx_clone.clone();
            Box::pin(async move {
                match event {
                    AgentEvent::MessageUpdate { assistant_message_event, .. } => {
                        if let AssistantMessageEvent::TextDelta { delta, .. } = assistant_message_event {
                            let _ = tx.send(AgentOutput::TextDelta(delta));
                        }
                    }
                    AgentEvent::MessageEnd { message: msg } => {
                        if let pi_agent_core::types::AgentMessage::Assistant { content, .. } = &msg {
                            let text: String = content.iter()
                                .filter_map(|b| if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = b { Some(text.clone()) } else { None })
                                .collect();
                            if !text.is_empty() {
                                let _ = tx.send(AgentOutput::MessageEnd(text));
                            }
                        }
                    }
                    AgentEvent::ToolExecutionStart { tool_name, .. } => {
                        let _ = tx.send(AgentOutput::ToolStart(tool_name));
                    }
                    AgentEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
                        let _ = tx.send(AgentOutput::ToolEnd(tool_name, is_error));
                    }
                    _ => {}
                }
            })
        });

    session.subscribe(listener).await;

    // ── Main event loop ────────────────────────────────────────────────
    let mut pending_message = String::new();
    let mut exit_code: i32 = 0;

    loop {
        // Render current TUI state
        let draw_result = terminal.ratatui_terminal().draw(|frame| {
            pi_tui::app::view(&tui_model, frame);
        });

        if let Err(e) = draw_result {
            eprintln!("Render error: {e}");
            exit_code = 1;
            break;
        }

        // Process events
        tokio::select! {
            // TUI keyboard events
            Some(key) = input_rx.recv() => {
                use crossterm::event::{KeyCode, KeyEventKind};

                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Handle quit (Ctrl+C, Ctrl+D, Esc)
                match key.code {
                    KeyCode::Char('c') if key.modifiers == crossterm::event::KeyModifiers::CONTROL => {
                        exit_code = 0;
                        break;
                    }
                    KeyCode::Char('d') if key.modifiers == crossterm::event::KeyModifiers::CONTROL => {
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
                            // Send to agent
                            tui_model.input.clear();
                            tui_model.messages.push(pi_tui::app::Message {
                                role: "user".into(),
                                text: text.clone(),
                            });
                            tui_model.is_streaming = true;
                            pending_message = text;
                        }
                    }
                    _ => {
                        pi_tui::app::update(&mut tui_model, pi_tui::Msg::Key(key));
                    }
                }
            }

            // Agent response events
            Some(output) = agent_rx.recv() => {
                match output {
                    AgentOutput::TextDelta(delta) => {
                        // Append to last assistant message
                        if tui_model.messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                            if let Some(m) = tui_model.messages.last_mut() {
                                m.text.push_str(&delta);
                            }
                        } else {
                            tui_model.messages.push(pi_tui::app::Message {
                                role: "assistant".into(),
                                text: delta,
                            });
                        }
                    }
                    AgentOutput::MessageEnd(text) => {
                        if tui_model.messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                            if let Some(m) = tui_model.messages.last_mut() {
                                m.text = text;
                            }
                        } else {
                            tui_model.messages.push(pi_tui::app::Message {
                                role: "assistant".into(),
                                text,
                            });
                        }
                        tui_model.is_streaming = false;
                    }
                    AgentOutput::ToolStart(tool_name) => {
                        tui_model.messages.push(pi_tui::app::Message {
                            role: "tool".into(),
                            text: format!("Running {tool_name}..."),
                        });
                    }
                    AgentOutput::ToolEnd(tool_name, is_error) => {
                        if is_error {
                            tui_model.messages.push(pi_tui::app::Message {
                                role: "tool".into(),
                                text: format!("✗ {tool_name} failed"),
                            });
                        }
                    }
                }
            }

            // Periodic refresh
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(16)) => {
                // Send pending message to agent if any
                if !pending_message.is_empty() {
                    let msg = std::mem::take(&mut pending_message);
                    session.add_user_text(&msg).await;
                }
            }
        }
    }

    // ── Cleanup ────────────────────────────────────────────────────────
    shutdown_guard.shutdown();
    let _ = terminal.clear_screen();
    exit_code
}
