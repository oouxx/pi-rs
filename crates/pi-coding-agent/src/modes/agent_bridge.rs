//! Agent bridge — typed events from AgentSession to TUI.
//!
//! Mirrors `agent_bridge/` from plans.md Section 5.1.
//!
//! Tool events carry the **tool call id** (not just the tool name): the TUI
//! keys tool rows by call id, so two calls of the same tool (two `bash`
//! runs, two `read`s) never corrupt each other's state/output.
//!
//! Tool output events carry the **full accumulated snapshot** of the
//! partial result (the bash tool throttles snapshots of the whole output;
//! the ACP translator's delta logic confirms this by computing deltas
//! downstream). The TUI **replaces** the row's output on every update —
//! appending snapshots would duplicate the text.

use std::sync::Arc;

use pi_agent_core::pi_ai_types::AssistantMessageEvent;
use pi_agent_core::types::AgentEvent as CoreAgentEvent;
use tokio::sync::mpsc;

use crate::core::agent_session::AgentSession;

/// Typed agent events for the TUI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta(String),
    /// Final assistant message: text, thinking content, terminal stop
    /// reason and provider error message (TS `Done`/`Error` events).
    MessageEnd {
        text: String,
        thinking: String,
        stop_reason: Option<pi_agent_core::pi_ai_types::StopReason>,
        error_message: Option<String>,
    },
    /// `(tool_call_id, tool_name, args_json)` — args serialized like the TS
    /// fallback tool component (`JSON.stringify(args, null, 2)`).
    ToolStart(String, String, String),
    /// `(tool_call_id, tool_name, is_error)`.
    ToolEnd(String, String, bool),
    /// `(tool_call_id, tool_name, text)` — a full output snapshot.
    ToolOutput(String, String, String),
    /// `(tool_call_id, truncation)` — truncation metadata from the tool
    /// result details (bash `details.truncation` + `fullOutputPath`).
    ToolTruncation(String, Option<pi_tui::app::ToolTruncation>),
}

/// Extract the displayable text from a tool result/partial-result value
/// (same shape the ACP translator reads: `content: [{type:"text",text}]`
/// blocks, falling back to `details.stdout`/`stderr`/`output` and
/// `details.diff`). Returns the full accumulated text for snapshot
/// semantics.
/// Extract truncation metadata from a tool result value: bash puts
/// `details.truncation` + `details.fullOutputPath` on the result, read puts
/// `details.truncation`, and grep puts `details.matchLimitReached` /
/// `details.linesTruncated` next to `details.truncation`.
fn tool_truncation(value: &serde_json::Value) -> Option<pi_tui::app::ToolTruncation> {
    let obj = value.as_object()?;
    let details = obj.get("details")?.as_object()?;
    let trunc = details.get("truncation")?.as_object()?;
    let truncated = trunc.get("truncated").and_then(|t| t.as_bool()).unwrap_or(false);
    let full_output_path = details
        .get("fullOutputPath")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let match_limit_reached = details.get("match_limit_reached").and_then(|v| v.as_u64());
    let lines_truncated = details.get("lines_truncated").and_then(|v| v.as_bool());
    // Nothing to warn about (TS renderers only add a warning when one of
    // these is present).
    if !truncated
        && full_output_path.is_none()
        && match_limit_reached.is_none()
        && lines_truncated != Some(true)
    {
        return None;
    }
    Some(pi_tui::app::ToolTruncation {
        truncated,
        truncated_by: trunc
            .get("truncated_by")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        output_lines: trunc.get("output_lines").and_then(|v| v.as_u64()).unwrap_or(0),
        total_lines: trunc.get("total_lines").and_then(|v| v.as_u64()).unwrap_or(0),
        max_lines: trunc.get("max_lines").and_then(|v| v.as_u64()).unwrap_or(0),
        max_bytes: trunc.get("max_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
        full_output_path,
        first_line_exceeds_limit: trunc
            .get("first_line_exceeds_limit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        match_limit_reached,
        lines_truncated,
    })
}

fn tool_result_text(value: &serde_json::Value) -> String {
    if value.is_null() {
        return String::new();
    }
    let obj = value.as_object();
    let details = obj.and_then(|o| o.get("details")).and_then(|d| d.as_object());

    // `details.diff` — pi's edit tool returns the unified diff there.
    if let Some(diff) = details.and_then(|d| d.get("diff")).and_then(|d| d.as_str()) {
        if !diff.trim().is_empty() {
            return diff.to_string();
        }
    }

    // `content: [{ type: "text", text: "..." }, ...]`
    if let Some(content) = obj.and_then(|o| o.get("content")).and_then(|c| c.as_array()) {
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|c| {
                let c = c.as_object()?;
                if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                    c.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !texts.is_empty() {
            return texts.join("");
        }
    }

    let get_str = |key: &str| -> Option<String> {
        details
            .and_then(|d| d.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                obj.and_then(|o| o.get(key))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
    };
    let stdout = get_str("stdout").or_else(|| get_str("output"));
    let stderr = get_str("stderr");
    let mut parts: Vec<String> = [stdout, stderr].into_iter().flatten().collect();
    parts.retain(|s| !s.is_empty());
    parts.join("\n")
}

/// Subscribe to an AgentSession and forward typed events to the sender.
/// Call this before starting agent processing.
/// Agent event listener bridging core events to the mode's event channel.
type CoreAgentEventListener = Arc<
    dyn Fn(
            CoreAgentEvent,
            Option<tokio::sync::watch::Receiver<bool>>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

pub async fn subscribe_agent(
    session: &mut AgentSession,
    tx: mpsc::UnboundedSender<AgentEvent>,
) {
    let tx_clone = tx.clone();
    let listener: CoreAgentEventListener =
        Arc::new(move |event, _signal| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                match &event {
                    CoreAgentEvent::MessageUpdate {
                        assistant_message_event:
                            AssistantMessageEvent::TextDelta { delta, .. },
                        ..
                    } => {
                        let _ = tx.send(AgentEvent::TextDelta(delta.clone()));
                    }
                    CoreAgentEvent::MessageEnd {
                        message:
                            pi_agent_core::types::AgentMessage::Assistant {
                                content,
                                stop_reason,
                                error_message,
                                ..
                            },
                    } => {
                        let text: String = content
                            .iter()
                            .filter_map(|b| {
                                if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = b {
                                    Some(text.clone())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        let thinking: String = content
                            .iter()
                            .filter_map(|b| {
                                if let pi_agent_core::pi_ai_types::ContentBlock::Thinking { thinking, .. } = b {
                                    Some(thinking.clone())
                                } else {
                                    None
                                }
                            })
                            .collect();
                        let stop_reason = stop_reason.clone().and_then(|r| match r {
                            pi_agent_core::pi_ai_types::StopReason::Length => {
                                Some(pi_agent_core::pi_ai_types::StopReason::Length)
                            }
                            pi_agent_core::pi_ai_types::StopReason::Aborted => {
                                Some(pi_agent_core::pi_ai_types::StopReason::Aborted)
                            }
                            pi_agent_core::pi_ai_types::StopReason::Error => {
                                Some(pi_agent_core::pi_ai_types::StopReason::Error)
                            }
                            _ => None,
                        });
                        if !text.is_empty() || !thinking.is_empty() {
                            let _ = tx.send(AgentEvent::MessageEnd {
                                text,
                                thinking,
                                stop_reason,
                                error_message: error_message.clone(),
                            });
                        }
                    }
                    CoreAgentEvent::ToolExecutionStart {
                        tool_call_id,
                        tool_name,
                        args,
                        ..
                    } => {
                        let args_json = if args.is_null() {
                            String::new()
                        } else {
                            serde_json::to_string_pretty(args).unwrap_or_default()
                        };
                        let _ = tx.send(AgentEvent::ToolStart(
                            tool_call_id.clone(),
                            tool_name.clone(),
                            args_json,
                        ));
                    }
                    CoreAgentEvent::ToolExecutionEnd {
                        tool_call_id,
                        tool_name,
                        result,
                        is_error,
                        ..
                    } => {
                        let _ = tx.send(AgentEvent::ToolEnd(
                            tool_call_id.clone(),
                            tool_name.clone(),
                            *is_error,
                        ));
                        // Final output snapshot: the tool's complete result
                        // (replace semantics — the TUI overwrites the
                        // streamed output with this text).
                        let text = tool_result_text(result);
                        let truncation = tool_truncation(result);
                        if !text.is_empty() {
                            let _ = tx.send(AgentEvent::ToolOutput(
                                tool_call_id.clone(),
                                tool_name.clone(),
                                text,
                            ));
                        }
                        if truncation.is_some() {
                            let _ = tx.send(AgentEvent::ToolTruncation(
                                tool_call_id.clone(),
                                truncation,
                            ));
                        }
                    }
                    CoreAgentEvent::ToolExecutionUpdate {
                        tool_call_id,
                        tool_name,
                        partial_result,
                        ..
                    } => {
                        let text = tool_result_text(partial_result);
                        if !text.is_empty() {
                            let _ = tx.send(AgentEvent::ToolOutput(
                                tool_call_id.clone(),
                                tool_name.clone(),
                                text,
                            ));
                        }
                    }
                    _ => {}
                }
            })
        });

    session.get_agent().subscribe(listener).await;
}
