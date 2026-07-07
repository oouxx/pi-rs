//! Agent bridge — typed events from AgentSession to TUI.
//!
//! Mirrors `agent_bridge/` from plans.md Section 5.1.

use std::sync::Arc;

use pi_agent_core::pi_ai_types::AssistantMessageEvent;
use pi_agent_core::types::AgentEvent as CoreAgentEvent;
use tokio::sync::mpsc;

use crate::core::agent_session::AgentSession;

/// Typed agent events for the TUI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta(String),
    MessageEnd(String),
    ToolStart(String),
    ToolEnd(String, bool),
    ToolOutput(String, String),
}

/// Subscribe to an AgentSession and forward typed events to the sender.
/// Call this before starting agent processing.
pub async fn subscribe_agent(
    session: &mut AgentSession,
    tx: mpsc::UnboundedSender<AgentEvent>,
) {
    let tx_clone = tx.clone();
    let listener: Arc<dyn Fn(CoreAgentEvent, Option<tokio::sync::watch::Receiver<bool>>) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync> =
        Arc::new(move |event, _signal| {
            let tx = tx_clone.clone();
            Box::pin(async move {
                match &event {
                    CoreAgentEvent::MessageUpdate { assistant_message_event, .. } => {
                        if let AssistantMessageEvent::TextDelta { delta, .. } = assistant_message_event {
                            let _ = tx.send(AgentEvent::TextDelta(delta.clone()));
                        }
                    }
                    CoreAgentEvent::MessageEnd { message: msg } => {
                        if let pi_agent_core::types::AgentMessage::Assistant { content, .. } = msg {
                            let text: String = content.iter()
                                .filter_map(|b| if let pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. } = b { Some(text.clone()) } else { None })
                                .collect();
                            if !text.is_empty() {
                                let _ = tx.send(AgentEvent::MessageEnd(text));
                            }
                        }
                    }
                    CoreAgentEvent::ToolExecutionStart { tool_name, .. } => {
                        let _ = tx.send(AgentEvent::ToolStart(tool_name.clone()));
                    }
                    CoreAgentEvent::ToolExecutionEnd { tool_name, is_error, .. } => {
                        let _ = tx.send(AgentEvent::ToolEnd(tool_name.clone(), *is_error));
                    }
                    CoreAgentEvent::ToolExecutionUpdate { tool_name, partial_result, .. } => {
                        if let Some(text) = partial_result.as_str() {
                            let _ = tx.send(AgentEvent::ToolOutput(tool_name.clone(), text.to_string()));
                        }
                    }
                    _ => {}
                }
            })
        });

    session.subscribe(listener).await;
}
