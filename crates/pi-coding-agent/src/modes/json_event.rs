//! Mirror of TS `modes/json-event.ts` — the session-event wire shape shared
//! by the JSON (`--mode json`) and RPC stdout protocols.
//!
//! Matches TS 0.84 `toJsonEvent()`:
//! - non-`message_update` events pass through unchanged (camelCase, complete);
//! - `message_update` carries only the delta: `{ type, usage,
//!   assistantMessageEvent }` with the cumulative `message` snapshot and the
//!   `assistantMessageEvent.partial` snapshot removed (`message_start`
//!   provides the initial message, deltas build it, `message_end` provides
//!   the final authoritative message). Cumulative `usage` is kept because its
//!   size is constant.

use serde_json::Value;

use pi_agent_core::types::AgentMessage;

use crate::core::agent_session::AgentSessionEvent;

/// Convert a session event to its JSON wire shape (TS `toJsonEvent()`).
pub fn to_json_event(event: &AgentSessionEvent) -> Value {
    let AgentSessionEvent::MessageUpdate {
        message,
        assistant_message_event,
    } = event
    else {
        return serde_json::to_value(event).unwrap_or(Value::Null);
    };

    // TS throws "message_update message is not an assistant message" here; we
    // skip defensively so a broken producer can never crash the stream.
    let AgentMessage::Assistant { usage, .. } = message else {
        return Value::Null;
    };

    let mut ame = serde_json::to_value(assistant_message_event).unwrap_or(Value::Null);
    if let Some(obj) = ame.as_object_mut() {
        obj.remove("partial");
    }

    serde_json::json!({
        "type": "message_update",
        "usage": usage,
        "assistantMessageEvent": ame,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use pi_agent_core::pi_ai::types::AssistantMessageEvent;

    use super::*;
    use crate::core::agent_session::AgentSessionEvent;

    fn sample_assistant_message() -> pi_agent_core::pi_ai::types::AssistantMessage {
        serde_json::from_value(serde_json::json!({
            "content": [],
            "api": "openai-completions",
            "provider": "openai",
            "model": "gpt-5.5",
            "usage": {"input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 0,
                      "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0,
                               "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": "stop",
            "timestamp": 0
        }))
        .unwrap()
    }

    fn sample_agent_message() -> pi_agent_core::types::AgentMessage {
        serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": [],
            "api": "openai-completions",
            "provider": "openai",
            "model": "gpt-5.5",
            "usage": {"input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 0,
                      "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0,
                               "cacheWrite": 0.0, "total": 0.0}},
            "stopReason": "stop",
            "timestamp": 0
        }))
        .unwrap()
    }

    fn sid() -> String {
        "s1".to_string()
    }

    #[test]
    fn non_message_update_passes_through() {
        let e = AgentSessionEvent::MessageEnd {
            message: sample_agent_message(),
        };
        let v = to_json_event(&e);
        assert_eq!(v["type"], "message_end");
        // Non-message_update events keep their full payload (matching TS
        // `toJsonEvent` passthrough).
        assert!(v.get("message").is_some(), "message_end keeps message");
    }

    #[test]
    fn message_update_strips_message_and_partial_keeps_usage() {
        let e = AgentSessionEvent::MessageUpdate {
            message: sample_agent_message(),
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
                partial: sample_assistant_message(),
            },
        };
        let v = to_json_event(&e);
        assert_eq!(v["type"], "message_update");
        // Cumulative `message` snapshot removed (TS 0.84 delta-only wire).
        assert!(
            v.get("message").is_none(),
            "cumulative message must be stripped on the wire"
        );
        // Cumulative usage preserved (constant size).
        assert_eq!(v["usage"]["input"], 10);
        assert_eq!(v["usage"]["output"], 5);
        // Delta event keeps its fields, minus the cumulative partial snapshot.
        assert_eq!(v["assistantMessageEvent"]["type"], "text_delta");
        assert_eq!(v["assistantMessageEvent"]["contentIndex"], 0);
        assert_eq!(v["assistantMessageEvent"]["delta"], "hi");
        assert!(
            v["assistantMessageEvent"].get("partial").is_none(),
            "partial must be stripped on the wire (TS toJsonEvent)"
        );
    }

    #[test]
    fn message_update_without_partial_keeps_event_unchanged() {
        // (Our Rust type always carries `partial`, so the no-partial branch is
        // unreachable; assert the wire shape is still correct.)
        let e = AgentSessionEvent::MessageUpdate {
            message: sample_agent_message(),
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "hi".into(),
                partial: sample_assistant_message(),
            },
        };
        let v = to_json_event(&e);
        assert_eq!(v["type"], "message_update");
        assert!(v.get("message").is_none());
        assert!(v["assistantMessageEvent"].get("partial").is_none());
        assert_eq!(v["assistantMessageEvent"]["delta"], "hi");
    }
}
