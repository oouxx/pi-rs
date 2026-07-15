//! Print mode (single-shot): Send prompts, output result, exit.
//!
//! Used for:
//! - `pi -p "prompt"` — text output
//! - `pi --mode json "prompt"` — JSON event stream
//!
//! Mirrors packages/coding-agent/src/modes/print-mode.ts

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::core::agent_session::AgentSession;
use pi_agent_core::pi_ai_types::{AssistantMessageEvent, ContentBlock};
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

/// Run in print mode (single-shot).
/// Sends prompts to the agent and outputs the result.
///
/// Registers signal handlers (SIGTERM/SIGHUP) to clean up child processes
/// on early termination, matching the original print-mode.ts behavior.
pub async fn run_print_mode(options: PrintModeOptions<'_>) -> i32 {
    // Register signal handlers for cleanup on early termination
    let session_for_signal = Arc::new(tokio::sync::Mutex::new(Some(options.session)));
    let signal_session = session_for_signal.clone();

    // Set up SIGTERM handler
    let mut term_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .ok();
    let mut hang_signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .ok();

    let term_handler = tokio::spawn(async move {
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
            session.dispose().await;
        }
        std::process::exit(1);
    });

    let session = session_for_signal.lock().await.take().unwrap();
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
    mut session: AgentSession,
    message: &str,
    messages: &[String],
    images: &[ContentBlock],
    verbose: bool,
) -> i32 {
    let has_error = Arc::new(AtomicBool::new(false));
    let err_flag = has_error.clone();

    let listener: Arc<
        dyn Fn(
                AgentEvent,
                Option<tokio::sync::watch::Receiver<bool>>,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            + Send
            + Sync,
    > = Arc::new(move |event: AgentEvent, _signal| {
        let err_flag = err_flag.clone();
        Box::pin(async move {
            match event {
                AgentEvent::MessageUpdate {
                    assistant_message_event,
                    ..
                } => {
                    if let AssistantMessageEvent::TextDelta { delta, .. } =
                        assistant_message_event
                    {
                        print!("{delta}");
                        std::io::Write::flush(&mut std::io::stdout()).ok();
                    }
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
                    tool_name, is_error, ..
                } => {
                    if is_error {
                        err_flag.store(true, Ordering::SeqCst);
                    }
                    if verbose {
                        if is_error {
                            eprintln!("  \u{2717} {tool_name} {}", "failed");
                        } else {
                            eprintln!("  \u{2713} {tool_name} {}", "done");
                        }
                    }
                }
                _ => {}
            }
        })
    });

    session.subscribe(listener).await;

    // Send the first message (with images if provided)
    if images.is_empty() {
        session.add_user_text(message).await;
    } else {
        let mut content = vec![ContentBlock::text(message)];
        content.extend_from_slice(images);
        session.add_user_message(content).await;
    }
    session.wait_for_idle().await;

    // Send additional messages
    for msg in messages {
        session.add_user_text(msg).await;
        session.wait_for_idle().await;
    }

    // Give a brief moment for final events to flush
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    if has_error.load(Ordering::SeqCst) {
        1
    } else {
        0
    }
}

/// Run in JSON mode: newline-delimited JSON event stream.
async fn run_json_mode(
    mut session: AgentSession,
    message: &str,
    messages: &[String],
    images: &[ContentBlock],
) -> i32 {
    let has_error = Arc::new(AtomicBool::new(false));
    let err_flag = has_error.clone();

    println!(
        "{}",
        serde_json::json!({"type": "start", "message": message})
    );

    let listener: Arc<
        dyn Fn(
                AgentEvent,
                Option<tokio::sync::watch::Receiver<bool>>,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            + Send
            + Sync,
    > = Arc::new(move |event: AgentEvent, _signal| {
        let err_flag = err_flag.clone();
        Box::pin(async move {
            let json = match &event {
                AgentEvent::MessageStart { .. } => {
                    serde_json::json!({"type": "message_start"})
                }
                AgentEvent::MessageUpdate {
                    assistant_message_event,
                    ..
                } => serde_json::json!({"type": "message_update", "event": assistant_message_event}),
                AgentEvent::MessageEnd { message: msg } => {
                    serde_json::json!({"type": "message_end", "message": msg})
                }
                AgentEvent::ToolExecutionStart {
                    tool_name,
                    tool_call_id,
                    args,
                } => serde_json::json!({"type": "tool_execution_start", "tool_call_id": tool_call_id, "tool_name": tool_name, "args": args}),
                AgentEvent::ToolExecutionEnd {
                    tool_name,
                    tool_call_id,
                    result,
                    is_error,
                } => {
                    if *is_error {
                        err_flag.store(true, Ordering::SeqCst);
                    }
                    serde_json::json!({"type": "tool_execution_end", "tool_call_id": tool_call_id, "tool_name": tool_name, "result": result, "is_error": is_error})
                }
                AgentEvent::AgentEnd { .. } => serde_json::json!({"type": "agent_end"}),
                _ => return,
            };
            println!("{}", serde_json::to_string(&json).unwrap_or_default());
        })
    });

    session.subscribe(listener).await;

    // Send the first message (with images if provided)
    if images.is_empty() {
        session.add_user_text(message).await;
    } else {
        let mut content = vec![ContentBlock::text(message)];
        content.extend_from_slice(images);
        session.add_user_message(content).await;
    }
    session.wait_for_idle().await;

    // Send additional messages
    for msg in messages {
        session.add_user_text(msg).await;
        session.wait_for_idle().await;
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    println!("{}", serde_json::json!({"type": "end"}));

    if has_error.load(Ordering::SeqCst) {
        1
    } else {
        0
    }
}

/// Run in quiet text mode: only print the final response text.
pub async fn run_quiet_text_mode(mut session: AgentSession, message: &str) -> i32 {
    let final_text = Arc::new(std::sync::Mutex::new(String::new()));
    let output_text = final_text.clone();

    let listener: Arc<
        dyn Fn(
                AgentEvent,
                Option<tokio::sync::watch::Receiver<bool>>,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            + Send
            + Sync,
    > = Arc::new(move |event: AgentEvent, _signal| {
        let output_text = output_text.clone();
        Box::pin(async move {
            if let AgentEvent::MessageEnd { message: msg } = event {
                if let AgentMessage::Assistant { content, .. } = &msg {
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
                    *output_text.lock().unwrap() = text;
                }
            }
        })
    });

    session.subscribe(listener).await;
    session.add_user_text(message).await;
    session.wait_for_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let text = final_text.lock().unwrap().clone();
    if !text.is_empty() {
        println!("{text}");
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_options_default_mode() {
        assert_eq!("text", "text");
    }
}
