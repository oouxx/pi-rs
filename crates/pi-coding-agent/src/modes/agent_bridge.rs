//! Agent bridge — typed events from AgentSession to TUI (plans.md §5.1).
//!
//! Tool events carry the tool call id, so two calls of the same tool never
//! corrupt each other's rows. Tool output events carry the full accumulated
//! snapshot; the TUI replaces the row's output on each update.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use pi_agent_core::pi_ai_types::{AssistantMessageEvent, ContentBlock, StopReason};
use pi_agent_core::types::AgentEvent as CoreAgentEvent;
use tokio::sync::mpsc;

use crate::core::agent_session::{AgentSession, AgentSessionEvent};

/// Typed agent events for the TUI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta(String),
    /// 用户消息进入 transcript（TS `message_start` role==user →
    /// `addMessageToChat`）：排队中的 steering/follow-up 被消费、或新
    /// prompt 启动时发出。TUI 不在提交时自己画用户气泡（对齐 TS：排队
    /// 消息只在 pending 区显示，被消费后才进聊天区）。
    UserMessage(String),
    /// Assistant message started (TS `message_start` role==assistant →
    /// creating `AssistantMessageComponent`). Sent before any content delta so
    /// terminal errors with empty content (for example a connection timeout
    /// before the first token) still have a transcript row to attach to.
    AssistantMessageStart,
    /// Run 生命周期（TS isStreaming = _isAgentRunActive）：排队消息消费
    /// 触发的续跑也走这两个事件，TUI spinner 据此启停。
    AgentRunStart,
    AgentRunEnd,
    /// Final assistant message: text, thinking content, terminal stop
    /// reason, provider error message, and any tool calls that failed with
    /// the message (TS `message_end` → `updateContent` + failed pending-tool
    /// components).
    MessageEnd {
        text: String,
        thinking: String,
        stop_reason: Option<pi_agent_core::pi_ai_types::StopReason>,
        error_message: Option<String>,
        /// `(tool_call_id, tool_name, args_json)` for assistant tool calls
        /// that were never executed because the turn ended in error/abort.
        failed_tool_calls: Vec<(String, String, String)>,
        /// TS `session.retryAttempt` when the terminal message arrived.
        retry_attempt: u32,
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
    /// `(tool_call_id, details)` — 工具结果的结构化 `details`
    /// （扩展自定义渲染 `renderResult` 用；对齐 TS `result.details`）。
    ToolDetails(String, Option<serde_json::Value>),
}

/// 从工具结果值里取 `details`（对齐 TS `result.details`）。
pub(crate) fn tool_details(value: &serde_json::Value) -> Option<serde_json::Value> {
    let details = value.get("details")?;
    (!details.is_null()).then(|| details.clone())
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
pub(crate) fn tool_truncation(value: &serde_json::Value) -> Option<pi_tui::app::ToolTruncation> {
    let obj = value.as_object()?;
    let details = obj.get("details")?.as_object()?;
    let trunc = details.get("truncation")?.as_object()?;
    let truncated = trunc
        .get("truncated")
        .and_then(|t| t.as_bool())
        .unwrap_or(false);
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
        output_lines: trunc
            .get("output_lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        total_lines: trunc
            .get("total_lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
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

pub(crate) fn tool_result_text(value: &serde_json::Value) -> String {
    if value.is_null() {
        return String::new();
    }
    let obj = value.as_object();
    let details = obj
        .and_then(|o| o.get("details"))
        .and_then(|d| d.as_object());

    // `details.diff` — pi's edit tool returns the unified diff there.
    if let Some(diff) = details.and_then(|d| d.get("diff")).and_then(|d| d.as_str()) {
        if !diff.trim().is_empty() {
            return diff.to_string();
        }
    }

    // `content: [{ type: "text", text: "..." }, ...]`
    if let Some(content) = obj
        .and_then(|o| o.get("content"))
        .and_then(|c| c.as_array())
    {
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

/// Build the terminal assistant event forwarded to the TUI.
///
/// TS always runs `message_end` through the existing assistant component,
/// including when both text and thinking are empty (provider errors before the
/// first content block). Do not use content emptiness as a forwarding gate.
fn assistant_message_end_event(
    content: &[ContentBlock],
    stop_reason: Option<&StopReason>,
    error_message: Option<&str>,
    retry_attempt: u32,
) -> AgentEvent {
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
    let thinking: String = content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Thinking { thinking, .. } = b {
                Some(thinking.clone())
            } else {
                None
            }
        })
        .collect();
    let failed = matches!(stop_reason, Some(StopReason::Error | StopReason::Aborted));
    let failed_tool_calls = if failed {
        content
            .iter()
            .filter_map(|block| {
                let ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } = block
                else {
                    return None;
                };
                let args = if arguments.is_null() {
                    String::new()
                } else {
                    serde_json::to_string_pretty(arguments).unwrap_or_default()
                };
                Some((id.clone(), name.clone(), args))
            })
            .collect()
    } else {
        Vec::new()
    };
    let stop_reason = stop_reason.and_then(|r| match r {
        StopReason::Length => Some(StopReason::Length),
        StopReason::Aborted => Some(StopReason::Aborted),
        StopReason::Error => Some(StopReason::Error),
        _ => None,
    });
    AgentEvent::MessageEnd {
        text,
        thinking,
        stop_reason,
        error_message: error_message
            .filter(|message| !message.is_empty())
            .map(str::to_string),
        failed_tool_calls,
        retry_attempt,
    }
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

pub async fn subscribe_agent(session: &mut AgentSession, tx: mpsc::UnboundedSender<AgentEvent>) {
    let tx_clone = tx.clone();
    // The session's internal agent listener resets `retry_attempt` after a
    // non-Error terminal message. Snapshot it from the session event, which is
    // emitted before that reset, so the bridge can reproduce TS's
    // `Aborted after N retry attempt(s)` wording.
    let retry_snapshot = Arc::new(AtomicU32::new(0));
    {
        let retry_attempt = session.retry_attempt_handle();
        let snapshot = retry_snapshot.clone();
        session.subscribe_session_events(Arc::new(move |event| {
            if matches!(event, AgentSessionEvent::MessageEnd { .. }) {
                let attempt = *retry_attempt
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                snapshot.store(attempt, Ordering::SeqCst);
            }
        }));
    }
    let listener: CoreAgentEventListener = Arc::new(move |event, _signal| {
        let tx = tx_clone.clone();
        let retry_snapshot = retry_snapshot.clone();
        Box::pin(async move {
            match &event {
                CoreAgentEvent::MessageStart {
                    message: pi_agent_core::types::AgentMessage::User { content, .. },
                    ..
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
                    if !text.is_empty() {
                        let _ = tx.send(AgentEvent::UserMessage(text));
                    }
                }
                CoreAgentEvent::MessageStart {
                    message: pi_agent_core::types::AgentMessage::Assistant { .. },
                    ..
                } => {
                    let _ = tx.send(AgentEvent::AssistantMessageStart);
                }
                CoreAgentEvent::AgentStart => {
                    let _ = tx.send(AgentEvent::AgentRunStart);
                }
                CoreAgentEvent::AgentEnd { .. } => {
                    let _ = tx.send(AgentEvent::AgentRunEnd);
                }
                CoreAgentEvent::MessageUpdate {
                    assistant_message_event: AssistantMessageEvent::TextDelta { delta, .. },
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
                    let attempt = retry_snapshot.load(Ordering::SeqCst);
                    let _ = tx.send(assistant_message_end_event(
                        content,
                        stop_reason.as_ref(),
                        error_message.as_deref(),
                        attempt,
                    ));
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
                        let _ =
                            tx.send(AgentEvent::ToolTruncation(tool_call_id.clone(), truncation));
                    }
                    if let Some(details) = tool_details(result) {
                        let _ =
                            tx.send(AgentEvent::ToolDetails(tool_call_id.clone(), Some(details)));
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
                    // 中间结果的 details 也要更新：扩展的 `onUpdate` 会带
                    // 新的 details（如 subagent 的并行进度快照）。
                    if let Some(details) = tool_details(partial_result) {
                        let _ =
                            tx.send(AgentEvent::ToolDetails(tool_call_id.clone(), Some(details)));
                    }
                }
                _ => {}
            }
        })
    });

    session.get_agent().subscribe(listener).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider can fail before emitting any content (connection refused,
    /// DNS failure, auth error). TS still runs `message_end` on the assistant
    /// component; the bridge must therefore forward an empty terminal event
    /// instead of dropping the provider error.
    #[test]
    fn empty_assistant_error_still_forwards_message_end() {
        let event = assistant_message_end_event(
            &[],
            Some(&StopReason::Error),
            Some("connection refused"),
            0,
        );

        match event {
            AgentEvent::MessageEnd {
                text,
                thinking,
                stop_reason,
                error_message,
                failed_tool_calls,
                ..
            } => {
                assert!(text.is_empty());
                assert!(thinking.is_empty());
                assert_eq!(stop_reason, Some(StopReason::Error));
                assert_eq!(error_message.as_deref(), Some("connection refused"));
                assert!(failed_tool_calls.is_empty());
            }
            other => panic!("expected MessageEnd, got {other:?}"),
        }
    }

    /// TS uses `errorMessage || fallback`, so an empty provider string is
    /// treated as absent rather than rendering an empty error notice.
    #[test]
    fn empty_error_message_is_normalized_to_none() {
        let event = assistant_message_end_event(&[], Some(&StopReason::Error), Some(""), 0);
        match event {
            AgentEvent::MessageEnd { error_message, .. } => {
                assert_eq!(error_message, None);
            }
            other => panic!("expected MessageEnd, got {other:?}"),
        }
    }

    /// A provider can emit a partial tool call and then fail. TS marks the
    /// pending tool component failed instead of showing an assistant-level
    /// error, so the bridge must preserve those tool calls for the TUI.
    #[test]
    fn terminal_error_collects_unexecuted_tool_calls() {
        let content = vec![ContentBlock::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
            thought_signature: None,
            namespace: None,
        }];
        let event = assistant_message_end_event(
            &content,
            Some(&StopReason::Error),
            Some("stream reset"),
            2,
        );

        match event {
            AgentEvent::MessageEnd {
                failed_tool_calls,
                retry_attempt,
                ..
            } => {
                assert_eq!(failed_tool_calls.len(), 1);
                assert_eq!(retry_attempt, 2);
                assert_eq!(failed_tool_calls[0].0, "call-1");
                assert_eq!(failed_tool_calls[0].1, "bash");
                assert!(failed_tool_calls[0].2.contains("echo hi"));
            }
            other => panic!("expected MessageEnd, got {other:?}"),
        }
    }
}
