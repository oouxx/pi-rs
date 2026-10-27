//! Print mode (single-shot): Send prompts, output result, exit.
//!
//! Used for:
//! - `pi -p "prompt"` — text output
//! - `pi --mode json "prompt"` — JSON event stream
//!
//! Mirrors packages/coding-agent/src/modes/print-mode.ts

use std::sync::Arc;

use crate::core::agent_session::AgentSession;
use pi_agent_core::pi_ai_types::{AssistantMessageEvent, ContentBlock, StopReason};
use pi_agent_core::types::{AgentEvent, AgentMessage};

/// Options for print mode, matching the original PrintModeOptions interface.
pub struct PrintModeOptions<'a> {
    /// Output mode: "text" for final response only, "json" for all events.
    pub mode: &'a str,
    /// Array of additional prompts to send after the first message.
    pub messages: &'a [String],
    /// First message to send.
    pub message: &'a str,
    /// Optional images to attach to the first message.
    pub images: Option<&'a [ContentBlock]>,
    /// Agent session to use.
    pub session: AgentSession,
    /// Whether to show verbose tool execution output on stderr.
    pub verbose: bool,
}

/// Agent event listener used by print-mode output modes.
type PrintModeListener = Arc<
    dyn Fn(
            AgentEvent,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Run in print mode (single-shot).
/// Sends prompts to the agent and outputs the result.
///
/// Registers signal handlers (SIGTERM/SIGHUP) to clean up child processes
/// on early termination, matching the original print-mode.ts behavior.
pub async fn run_print_mode(options: PrintModeOptions<'_>) -> i32 {
    // Register signal handlers for cleanup on early termination
    let session_for_signal = Arc::new(tokio::sync::Mutex::new(Some(options.session)));
    let signal_session = session_for_signal.clone();

    // Set up SIGTERM/SIGHUP handlers (Unix). Windows has no Unix signals;
    // Ctrl+C is handled by the terminal / tokio's ctrl_c below.
    #[cfg(unix)]
    let term_handler = {
        let mut term_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .ok();
        let mut hang_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .ok();

        tokio::spawn(async move {
            tokio::select! {
                _ = async {
                    if let Some(ref mut sig) = term_signal {
                        sig.recv().await;
                    }
                } => {}
                _ = async {
                    if let Some(ref mut sig) = hang_signal {
                        sig.recv().await;
                    }
                } => {}
            }
            if let Some(session) = signal_session.lock().await.take() {
                crate::utils::shell::kill_tracked_detached_children();
                session.dispose_inner().await;
            }
            std::process::exit(1);
        })
    };
    #[cfg(not(unix))]
    let term_handler = {
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            if let Some(session) = signal_session.lock().await.take() {
                session.dispose_inner().await;
            }
            std::process::exit(1);
        })
    };

    let session = session_for_signal
        .lock()
        .await
        .take()
        .unwrap_or_else(|| panic!("session already disposed by signal handler"));
    let images = options.images.unwrap_or(&[]);

    let result = match options.mode {
        "json" => run_json_mode(session, options.message, options.messages, images).await,
        _ => run_text_mode(session, options.message, options.messages, images, options.verbose).await,
    };

    term_handler.abort();
    result
}

/// Run in text mode: stream response to stdout.
async fn run_text_mode(
    session: AgentSession,
    message: &str,
    messages: &[String],
    images: &[ContentBlock],
    verbose: bool,
) -> i32 {
    let listener: PrintModeListener = Arc::new(move |event: AgentEvent, _signal| {
        Box::pin(async move {
            match event {
                AgentEvent::MessageUpdate {
                    assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
                    ..
                } => {
                    print!("{delta}");
                    std::io::Write::flush(&mut std::io::stdout()).ok();
                }
                AgentEvent::MessageEnd { .. } => {
                    // Final text is already streamed via TextDelta
                    // Just add a trailing newline for clean exit
                    println!();
                    std::io::Write::flush(&mut std::io::stdout()).ok();
                }
                AgentEvent::ToolExecutionStart {
                    tool_name, args, ..
                } => {
                    if verbose {
                        let args_str =
                            serde_json::to_string(&args).unwrap_or_default();
                        let clipped = if args_str.len() > 150 {
                            format!("{}...", &args_str[..150])
                        } else {
                            args_str
                        };
                        eprintln!("  \u{26a1} {tool_name}");
                        eprintln!("    {clipped}");
                    }
                }
                AgentEvent::ToolExecutionEnd {
                    tool_name,
                    is_error,
                    ..
                } if verbose => {
                    if is_error {
                        eprintln!("  \u{2717} {tool_name} failed");
                    } else {
                        eprintln!("  \u{2713} {tool_name} done");
                    }
                }
                _ => {}
            }
        })
    });

    session.get_agent().subscribe(listener).await;

    // Send the first message (with images if provided).
    // 用 prompt()（内部等 agent 完成并清理 is_agent_run_active 状态），
    // 因此无需额外的 wait_for_idle；流式错误时也不会永久卡住。
    let opts = if images.is_empty() {
        None
    } else {
        Some(crate::core::agent_session::PromptOptions {
            images: Some(images.to_vec()),
            ..Default::default()
        })
    };
    if let Err(e) = session.prompt(message, opts).await {
        eprintln!("Error: {e}");
        return 1;
    }

    // Send additional messages
    for msg in messages {
        if let Err(e) = session.prompt(msg, None).await {
            eprintln!("Error: {e}");
            return 1;
        }
    }

    // Give a brief moment for final events to flush
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    text_exit_code(&session.get_messages().await)
}

/// Text mode fails only when the final assistant message stopped with an error
/// or abort. Tool failures are model-visible results and may be recovered by a
/// later assistant turn, so they do not determine the process exit code.
fn text_exit_code(messages: &[AgentMessage]) -> i32 {
    if matches!(
        messages.last(),
        Some(AgentMessage::Assistant {
            stop_reason: Some(StopReason::Error | StopReason::Aborted),
            ..
        })
    ) {
        1
    } else {
        0
    }
}

/// Run in JSON mode: newline-delimited JSON event stream.
///
/// Wire format mirrors TS `print-mode.ts` (mode "json"): every
/// `AgentSessionEvent` serialized through `toJsonEvent()` (camelCase,
/// `message_update` delta-only per TS 0.84). The pi-rs `start`/`end`
/// sentinel lines are kept for stream framing; the event payloads between
/// them match the TS wire shape exactly.
async fn run_json_mode(
    session: AgentSession,
    message: &str,
    messages: &[String],
    images: &[ContentBlock],
) -> i32 {
    println!(
        "{}",
        serde_json::json!({"type": "start", "message": message})
    );

    let listener: crate::core::agent_session::AgentSessionEventListener =
        Arc::new(|event: crate::core::agent_session::AgentSessionEvent| {
            let json = super::json_event::to_json_event(&event);
            println!("{}", serde_json::to_string(&json).unwrap_or_default());
        });

    session.subscribe_session_events(listener);

    // Send the first message (with images if provided).
    // 用 prompt()（内部等 agent 完成并清理 is_agent_run_active 状态），
    // 因此无需额外的 wait_for_idle；流式错误时也不会永久卡住。
    let opts = if images.is_empty() {
        None
    } else {
        Some(crate::core::agent_session::PromptOptions {
            images: Some(images.to_vec()),
            ..Default::default()
        })
    };
    if let Err(e) = session.prompt(message, opts).await {
        eprintln!("Error: {e}");
        return 1;
    }

    // Send additional messages
    for msg in messages {
        if let Err(e) = session.prompt(msg, None).await {
            eprintln!("Error: {e}");
            return 1;
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    println!("{}", serde_json::json!({"type": "end"}));

    0
}

/// Run in quiet text mode: only print the final response text.
pub async fn run_quiet_text_mode(session: AgentSession, message: &str) -> i32 {
    let final_text = Arc::new(std::sync::Mutex::new(String::new()));
    let output_text = final_text.clone();

    let listener: PrintModeListener = Arc::new(move |event: AgentEvent, _signal| {
        let output_text = output_text.clone();
        Box::pin(async move {
            if let AgentEvent::MessageEnd {
                message: AgentMessage::Assistant { content, .. },
            } = event
            {
                let text: String = content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text { text, .. } = b {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                *output_text.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = text;
            }
        })
    });

    session.get_agent().subscribe(listener).await;
    if let Err(e) = session.prompt(message, None).await {
        eprintln!("Error: {e}");
        return 1;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let text = final_text.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    if !text.is_empty() {
        println!("{text}");
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::text_exit_code;
    use pi_agent_core::pi_ai_types::StopReason;
    use pi_agent_core::types::AgentMessage;

    fn assistant_message(stop_reason: StopReason) -> AgentMessage {
        AgentMessage::Assistant {
            content: Vec::new(),
            api: "test".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
            usage: Default::default(),
            stop_reason: Some(stop_reason),
            error_message: None,
            timestamp: 0,
        }
    }

    fn failed_tool_result() -> AgentMessage {
        AgentMessage::ToolResult {
            tool_call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            content: Vec::new(),
            details: serde_json::json!({}),
            is_error: true,
            usage: None,
            timestamp: 0,
        }
    }

    /// A failed tool is a recoverable model-visible result. If the following
    /// assistant turn stops normally, print mode must still exit successfully.
    #[test]
    fn recoverable_tool_error_does_not_fail_text_mode() {
        let messages = vec![
            assistant_message(StopReason::ToolUse),
            failed_tool_result(),
            assistant_message(StopReason::Stop),
        ];

        assert_eq!(text_exit_code(&messages), 0);
    }

    /// Only terminal assistant errors and aborts indicate a failed text-mode
    /// process; other stop reasons complete normally.
    #[test]
    fn only_terminal_assistant_error_or_abort_fails_text_mode() {
        assert_eq!(text_exit_code(&[assistant_message(StopReason::Error)]), 1);
        assert_eq!(text_exit_code(&[assistant_message(StopReason::Aborted)]), 1);
        assert_eq!(text_exit_code(&[assistant_message(StopReason::Stop)]), 0);
        assert_eq!(text_exit_code(&[assistant_message(StopReason::Length)]), 0);
    }
}
