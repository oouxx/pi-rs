//! Translate pi `AgentSessionEvent`s into ACP `SessionNotification`s.
//!
//! Mirrors the mapping done by the TS `pi-acp` adapter
//! (github.com/svkozak/pi-acp, `src/acp/session.ts`), but natively in Rust:
//! pi's streaming events become ACP `agent_message_chunk` /
//! `agent_thought_chunk` / `tool_call` / `tool_call_update` notifications.

use agent_client_protocol as acp;
use pi_agent_core::pi_ai_types::AssistantMessageEvent;

use crate::core::agent_session::AgentSessionEvent;

/// Map a pi tool name to an ACP `ToolKind` so clients can pick icons/UI.
pub fn tool_kind(tool_name: &str) -> acp::ToolKind {
    match tool_name {
        "read" | "ls" | "find" | "grep" => acp::ToolKind::Read,
        "edit" | "write" => acp::ToolKind::Edit,
        "bash" => acp::ToolKind::Execute,
        "web_fetch" | "web_search" => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}

/// Translate a pi session event into an ACP session notification, if any.
///
/// Returns `None` for events that have no ACP wire equivalent (turn
/// lifecycle, compaction, queue updates, etc.).
pub fn translate_event(
    session_id: &acp::SessionId,
    event: &AgentSessionEvent,
) -> Option<acp::SessionNotification> {
    let update = match event {
        // ── Assistant streaming ──────────────────────────────────────────
        AgentSessionEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => match assistant_message_event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(delta.clone())),
                ))
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(delta.clone())),
                ))
            }
            // Tool call generation is reported via tool_execution_* events below.
            _ => return None,
        },

        // ── Tool execution ───────────────────────────────────────────────
        AgentSessionEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
            ..
        } => {
            let tool_call = acp::ToolCall::new(tool_call_id.clone(), tool_name.clone())
                .kind(tool_kind(tool_name))
                .status(acp::ToolCallStatus::InProgress)
                .raw_input(args.clone());
            acp::SessionUpdate::ToolCall(tool_call)
        }
        AgentSessionEvent::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => {
            let text = partial_text(partial_result);
            let fields = acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::InProgress)
                .content(vec![acp::ToolCallContent::Content(acp::Content::new(
                    acp::ContentBlock::Text(acp::TextContent::new(text)),
                ))]);
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                tool_call_id.clone(),
                fields,
            ))
        }
        AgentSessionEvent::ToolExecutionEnd {
            tool_call_id,
            result,
            is_error,
            ..
        } => {
            let status = if *is_error {
                acp::ToolCallStatus::Failed
            } else {
                acp::ToolCallStatus::Completed
            };
            let text = partial_text(result);
            let fields = acp::ToolCallUpdateFields::new()
                .status(status)
                .content(vec![acp::ToolCallContent::Content(acp::Content::new(
                    acp::ContentBlock::Text(acp::TextContent::new(text)),
                ))])
                .raw_output(result.clone());
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                tool_call_id.clone(),
                fields,
            ))
        }

        // ── Session metadata ────────────────────────────────────────────
        AgentSessionEvent::SessionInfoChanged { name } => {
            let update = acp::SessionInfoUpdate::new().title(name.clone().unwrap_or_default());
            acp::SessionUpdate::SessionInfoUpdate(update)
        }

        // ── Assistant errors ─────────────────────────────────────────────
        // ACP has no error stop reason, so surface the failure as a message
        // chunk — otherwise the client UI shows an empty turn with no
        // explanation (e.g. an LLM 402/insufficient-balance error).
        AgentSessionEvent::MessageEnd { message } => {
            if let pi_agent_core::types::AgentMessage::Assistant {
                error_message: Some(err),
                ..
            } = message
            {
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(format!("⚠️ {err}"))),
                ))
            } else {
                return None;
            }
        }

        // Turn lifecycle / compaction / queue events have no ACP wire
        // equivalent — the prompt response itself signals turn completion.
        _ => return None,
    };

    Some(acp::SessionNotification::new(session_id.clone(), update))
}

/// Extract a plain-text representation from a tool result / partial result.
fn partial_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            // Common shapes: { "text": "..." } or { "output": "..." }
            for key in ["text", "output", "content"] {
                if let Some(serde_json::Value::String(s)) = map.get(key) {
                    return s.clone();
                }
            }
            value.to_string()
        }
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use pi_agent_core::pi_ai_types::{AssistantMessage, StopReason, Usage};

    fn sample_assistant_message() -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        }
    }

    #[test]
    fn text_delta_becomes_agent_message_chunk() {
        let sid = acp::SessionId::new("s1");
        let event = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hello".into(),
                partial: sample_assistant_message(),
            },
        };
        let notif = translate_event(&sid, &event).expect("should translate");
        assert_eq!(notif.session_id, sid);
        match notif.update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                match chunk.content {
                    acp::ContentBlock::Text(t) => assert_eq!(t.text, "hello"),
                    _ => panic!("expected text content"),
                }
            }
            other => panic!("expected agent_message_chunk, got {other:?}"),
        }
    }

    #[test]
    fn thinking_delta_becomes_agent_thought_chunk() {
        let sid = acp::SessionId::new("s1");
        let event = AgentSessionEvent::MessageUpdate {
            message: serde_json::from_value(serde_json::json!({
                "role":"assistant","content":[],"api":"openai-completions","provider":"openai",
                "model":"gpt-5.5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,
                "cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0}},
                "stopReason":"stop","timestamp":0
            })).unwrap(),
            assistant_message_event: AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "hmm".into(),
                partial: sample_assistant_message(),
            },
        };
        let notif = translate_event(&sid, &event).expect("should translate");
        assert!(matches!(notif.update, acp::SessionUpdate::AgentThoughtChunk(_)));
    }

    #[test]
    fn tool_execution_maps_to_tool_call_and_updates() {
        let sid = acp::SessionId::new("s1");

        let start = AgentSessionEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            args: serde_json::json!({"path": "a.txt"}),
        };
        let notif = translate_event(&sid, &start).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCall(tc) => {
                assert_eq!(tc.tool_call_id.0.as_ref(), "t1");
                assert_eq!(tc.title, "read");
                assert_eq!(tc.kind, acp::ToolKind::Read);
                assert_eq!(tc.status, acp::ToolCallStatus::InProgress);
                assert_eq!(tc.raw_input, Some(serde_json::json!({"path": "a.txt"})));
            }
            other => panic!("expected tool_call, got {other:?}"),
        }

        let end = AgentSessionEvent::ToolExecutionEnd {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            result: serde_json::json!({"text": "file contents"}),
            is_error: false,
        };
        let notif = translate_event(&sid, &end).expect("should translate");
        match notif.update {
            acp::SessionUpdate::ToolCallUpdate(tcu) => {
                assert_eq!(tcu.tool_call_id.0.as_ref(), "t1");
                assert_eq!(tcu.fields.status, Some(acp::ToolCallStatus::Completed));
                assert_eq!(tcu.fields.raw_output, Some(serde_json::json!({"text": "file contents"})));
            }
            other => panic!("expected tool_call_update, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_events_are_not_translated() {
        let sid = acp::SessionId::new("s1");
        for event in [
            AgentSessionEvent::AgentStart,
            AgentSessionEvent::TurnStart,
            AgentSessionEvent::AgentSettled,
        ] {
            assert!(translate_event(&sid, &event).is_none(), "unexpected translation");
        }
    }

    /// An assistant message that ended with an error must be surfaced to the
    /// client as a message chunk — otherwise the UI shows an empty turn with
    /// no explanation (e.g. LLM 402 insufficient-balance).
    #[test]
    fn assistant_error_message_is_translated() {
        let sid = acp::SessionId::new("s1");
        let event = AgentSessionEvent::MessageEnd {
            message: pi_agent_core::types::AgentMessage::Assistant {
                content: vec![],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: pi_agent_core::pi_ai_types::Usage::default(),
                stop_reason: Some(pi_agent_core::pi_ai_types::StopReason::Error),
                error_message: Some("OpenAI API error 402 Payment Required".into()),
                timestamp: 0,
            },
        };
        let notif = translate_event(&sid, &event).expect("error must be translated");
        match notif.update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                acp::ContentBlock::Text(t) => {
                    assert!(t.text.contains("402"), "error text must be surfaced");
                }
                _ => panic!("expected text content"),
            },
            other => panic!("expected agent_message_chunk, got {other:?}"),
        }
    }

    /// A normal (non-error) assistant message end has no ACP wire equivalent.
    #[test]
    fn normal_message_end_is_not_translated() {
        let sid = acp::SessionId::new("s1");
        let event = AgentSessionEvent::MessageEnd {
            message: pi_agent_core::types::AgentMessage::Assistant {
                content: vec![],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: pi_agent_core::pi_ai_types::Usage::default(),
                stop_reason: Some(pi_agent_core::pi_ai_types::StopReason::Stop),
                error_message: None,
                timestamp: 0,
            },
        };
        assert!(translate_event(&sid, &event).is_none());
    }
}
