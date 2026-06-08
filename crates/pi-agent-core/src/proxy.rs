use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::pi_ai_types::{AssistantMessage, ContentBlock, StopReason, Usage};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProxyAssistantMessageEvent {
    Start {
        content_index: usize,
    },
    TextStart {
        content_index: usize,
    },
    TextDelta {
        content_index: usize,
        delta: String,
    },
    TextEnd {
        content_index: usize,
        content_signature: Option<String>,
    },
    ThinkingStart {
        content_index: usize,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        content_index: usize,
        content_signature: Option<String>,
    },
    ToolCallStart {
        content_index: usize,
        id: String,
        tool_name: String,
    },
    ToolCallDelta {
        content_index: usize,
        delta: String,
    },
    ToolCallEnd {
        content_index: usize,
    },
    Done {
        reason: StopReason,
        usage: Option<Usage>,
    },
    Error {
        reason: StopReason,
        error_message: String,
        usage: Option<Usage>,
    },
}

pub fn process_proxy_event(
    proxy_event: ProxyAssistantMessageEvent,
    partial: &mut AssistantMessage,
) -> Option<crate::pi_ai_types::AssistantMessageEvent> {
    match proxy_event {
        ProxyAssistantMessageEvent::Start { content_index: _ } => {
            Some(crate::pi_ai_types::AssistantMessageEvent::Start {
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::TextStart { content_index } => {
            while partial.content.len() <= content_index {
                partial.content.push(ContentBlock::Text {
                    text: String::new(),
                    text_signature: None,
                });
            }
            partial.content[content_index] = ContentBlock::Text {
                text: String::new(),
                text_signature: None,
            };
            Some(crate::pi_ai_types::AssistantMessageEvent::TextStart {
                content_index,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::TextDelta {
            content_index,
            delta,
        } => {
            if let Some(ContentBlock::Text { text, .. }) = partial.content.get_mut(content_index) {
                text.push_str(&delta);
            }
            Some(crate::pi_ai_types::AssistantMessageEvent::TextDelta {
                content_index,
                delta,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::TextEnd {
            content_index,
            content_signature,
        } => {
            if let Some(ContentBlock::Text {
                text_signature: sig,
                ..
            }) = partial.content.get_mut(content_index)
            {
                *sig = content_signature;
            }
            let content = match &partial.content.get(content_index) {
                Some(ContentBlock::Text { text, .. }) => text.clone(),
                _ => String::new(),
            };
            Some(crate::pi_ai_types::AssistantMessageEvent::TextEnd {
                content_index,
                content,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::ThinkingStart { content_index } => {
            while partial.content.len() <= content_index {
                partial.content.push(ContentBlock::Thinking {
                    thinking: String::new(),
                    thinking_signature: None,
                    redacted: None,
                });
            }
            partial.content[content_index] = ContentBlock::Thinking {
                thinking: String::new(),
                thinking_signature: None,
                redacted: None,
            };
            Some(crate::pi_ai_types::AssistantMessageEvent::ThinkingStart {
                content_index,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::ThinkingDelta {
            content_index,
            delta,
        } => {
            if let Some(ContentBlock::Thinking { thinking, .. }) =
                partial.content.get_mut(content_index)
            {
                thinking.push_str(&delta);
            }
            Some(crate::pi_ai_types::AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::ThinkingEnd {
            content_index,
            content_signature,
        } => {
            if let Some(ContentBlock::Thinking {
                thinking_signature: sig,
                ..
            }) = partial.content.get_mut(content_index)
            {
                *sig = content_signature;
            }
            let content = match &partial.content.get(content_index) {
                Some(ContentBlock::Thinking { thinking, .. }) => thinking.clone(),
                _ => String::new(),
            };
            Some(crate::pi_ai_types::AssistantMessageEvent::ThinkingEnd {
                content_index,
                content,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::ToolCallStart {
            content_index,
            id,
            tool_name,
        } => {
            while partial.content.len() <= content_index {
                partial.content.push(ContentBlock::ToolCall {
                    id: String::new(),
                    name: String::new(),
                    arguments: serde_json::Value::Object(serde_json::Map::new()),
                    thought_signature: None,
                });
            }
            partial.content[content_index] = ContentBlock::ToolCall {
                id,
                name: tool_name,
                arguments: serde_json::Value::Object(serde_json::Map::new()),
                thought_signature: None,
            };
            Some(crate::pi_ai_types::AssistantMessageEvent::ToolCallStart {
                content_index,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::ToolCallDelta {
            content_index,
            delta,
        } => {
            if let Some(ContentBlock::ToolCall { arguments, .. }) =
                partial.content.get_mut(content_index)
            {
                // Merge delta into arguments (simplified)
                if let serde_json::Value::Object(map) = arguments {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&delta) {
                        if let serde_json::Value::Object(delta_map) = parsed {
                            for (k, v) in delta_map {
                                map.insert(k, v);
                            }
                        }
                    }
                }
            }
            Some(crate::pi_ai_types::AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::ToolCallEnd { content_index } => {
            let tool_call = match &partial.content.get(content_index) {
                Some(ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                }) => crate::pi_ai_types::ToolCall {
                    type_field: "toolCall".to_string(),
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    thought_signature: None,
                },
                _ => return None,
            };
            Some(crate::pi_ai_types::AssistantMessageEvent::ToolCallEnd {
                content_index,
                tool_call,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::Done { reason, usage } => {
            partial.stop_reason = reason.clone();
            partial.usage = usage.unwrap_or_default();
            Some(crate::pi_ai_types::AssistantMessageEvent::Done {
                reason,
                message: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::Error {
            reason,
            error_message,
            usage,
        } => {
            partial.stop_reason = reason.clone();
            partial.error_message = Some(error_message);
            partial.usage = usage.unwrap_or_default();
            Some(crate::pi_ai_types::AssistantMessageEvent::Error {
                reason,
                error: partial.clone(),
            })
        }
    }
}

pub async fn stream_proxy(
    url: &str,
    api_key: &str,
    model: &crate::pi_ai_types::Model,
    context: crate::pi_ai_types::Context,
    options: Option<ProxyStreamOptions>,
) -> Result<AssistantMessage, Box<dyn std::error::Error + Send + Sync>> {
    let _opts = options.unwrap_or_default();

    let client = reqwest::Client::new();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", api_key))?,
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );

    let mut partial = AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    };

    let response = client
        .post(url)
        .headers(headers)
        .json(&serde_json::json!({
            "model": model.id,
            "messages": context.messages,
            "system": context.system_prompt,
        }))
        .send()
        .await?;

    let body = response.text().await?;
    let _buffer = String::new();

    for line in body.lines() {
        if line.starts_with("data: ") {
            let data = &line[6..];
            if data.trim().is_empty() {
                continue;
            }
            if let Ok(proxy_event) = serde_json::from_str::<ProxyAssistantMessageEvent>(data) {
                process_proxy_event(proxy_event, &mut partial);
            }
        }
    }

    Ok(partial)
}

#[derive(Debug, Clone, Default)]
pub struct ProxyStreamOptions {
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub headers: Option<HashMap<String, String>>,
}
