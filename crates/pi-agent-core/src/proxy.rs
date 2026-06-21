use std::collections::HashMap;

use futures::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::pi_ai_types::{AssistantMessage, AssistantMessageEvent, ContentBlock, StopReason, Usage};

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone)]
pub struct ProxyStreamOptions {
    /// Proxy server URL (e.g., "https://genai.example.com")
    pub proxy_url: String,
    /// Auth token for the proxy server
    pub auth_token: String,
    /// Local abort signal for the proxy request
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// Extra headers to include in the proxy request
    pub headers: Option<HashMap<String, String>>,
}

pub type StreamResponse = Box<dyn Stream<Item = AssistantMessageEvent> + Send + Unpin>;

/// Stream function that proxies through a server instead of calling LLM providers directly.
/// The server strips the `partial` field from delta events to reduce bandwidth.
/// We reconstruct the partial message client-side.
pub fn stream_proxy(
    model: crate::pi_ai_types::Model,
    context: crate::pi_ai_types::Context,
    options: ProxyStreamOptions,
) -> StreamResponse {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AssistantMessageEvent>();

    tokio::spawn(async move {
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

        let client = reqwest::Client::new();
        let mut headers_map = reqwest::header::HeaderMap::new();
        headers_map.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", options.auth_token))
                .expect("valid auth header"),
        );
        headers_map.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        // Build proxy request payload matching TS buildProxyRequestOptions
        let request_body = serde_json::json!({
            "model": model,
            "context": context,
            "options": {
                "transport": serde_json::Value::Null,
                "headers": options.headers,
            },
        });

        let signal = options.signal;
        let is_aborted = || -> bool {
            signal.as_ref().map(|rx| *rx.borrow()).unwrap_or(false)
        };

        let response_result = client
            .post(&format!("{}/api/stream", options.proxy_url))
            .headers(headers_map)
            .json(&request_body)
            .send()
            .await;

        let response = match response_result {
            Ok(r) => r,
            Err(e) => {
                let error_message = format!("Proxy error: {}", e);
                partial.stop_reason = StopReason::Error;
                partial.error_message = Some(error_message.clone());
                let _ = tx.send(AssistantMessageEvent::Error {
                    reason: StopReason::Error,
                    error: partial.clone(),
                });
                return;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_message = if let Ok(body) = response.text().await {
                if let Ok(err_data) =
                    serde_json::from_str::<serde_json::Value>(&body)
                {
                    err_data
                        .get("error")
                        .and_then(|v| v.as_str())
                        .map(|s| format!("Proxy error: {}", s))
                        .unwrap_or_else(|| format!("Proxy error: {} {}", status.as_u16(), status.as_str()))
                } else {
                    format!("Proxy error: {} {}", status.as_u16(), status.as_str())
                }
            } else {
                format!("Proxy error: {} {}", status.as_u16(), status.as_str())
            };
            partial.stop_reason = StopReason::Error;
            partial.error_message = Some(error_message.clone());
            let _ = tx.send(AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error: partial.clone(),
            });
            return;
        }

        let mut reader = response.bytes_stream();
        use futures::StreamExt;
        let mut buffer = String::new();

        while let Some(chunk_result) = reader.next().await {
            if is_aborted() {
                partial.stop_reason = StopReason::Aborted;
                partial.error_message = Some("Request aborted by user".into());
                let _ = tx.send(AssistantMessageEvent::Error {
                    reason: StopReason::Aborted,
                    error: partial.clone(),
                });
                return;
            }

            match chunk_result {
                Ok(chunk) => {
                    let chunk_str = String::from_utf8_lossy(&chunk);
                    buffer.push_str(&chunk_str);

                    let lines: Vec<String> = buffer.split('\n').map(|s| s.to_string()).collect();
                    buffer = String::new();

                    for line in &lines {
                        if line.starts_with("data: ") {
                            let data = line[6..].trim().to_string();
                            if data.is_empty() {
                                continue;
                            }
                            if let Ok(proxy_event) =
                                serde_json::from_str::<ProxyAssistantMessageEvent>(&data)
                            {
                                if let Some(event) =
                                    process_proxy_event(proxy_event, &mut partial)
                                {
                                    let _ = tx.send(event);
                                }
                            }
                        } else if !line.is_empty() && !line.starts_with(":") {
                            // Keep non-empty, non-comment lines in buffer for next chunk
                            buffer.push_str(line);
                            buffer.push('\n');
                        }
                    }
                }
                Err(e) => {
                    let error_message = format!("Proxy stream error: {}", e);
                    partial.stop_reason = StopReason::Error;
                    partial.error_message = Some(error_message.clone());
                    let _ = tx.send(AssistantMessageEvent::Error {
                        reason: StopReason::Error,
                        error: partial.clone(),
                    });
                    return;
                }
            }
        }

        // Process any remaining buffer content
        if buffer.starts_with("data: ") {
            let data = buffer[6..].trim().to_string();
            if !data.is_empty() {
                if let Ok(proxy_event) =
                    serde_json::from_str::<ProxyAssistantMessageEvent>(&data)
                {
                    if let Some(event) = process_proxy_event(proxy_event, &mut partial) {
                        let _ = tx.send(event);
                    }
                }
            }
        }
    });

    Box::new(UnboundedReceiverStream::new(rx))
}

pub fn process_proxy_event(
    proxy_event: ProxyAssistantMessageEvent,
    partial: &mut AssistantMessage,
) -> Option<AssistantMessageEvent> {
    match proxy_event {
        ProxyAssistantMessageEvent::Start { content_index: _ } => {
            Some(AssistantMessageEvent::Start {
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
            Some(AssistantMessageEvent::TextStart {
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
            Some(AssistantMessageEvent::TextDelta {
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
            Some(AssistantMessageEvent::TextEnd {
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
            Some(AssistantMessageEvent::ThinkingStart {
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
            Some(AssistantMessageEvent::ThinkingDelta {
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
            Some(AssistantMessageEvent::ThinkingEnd {
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
            Some(AssistantMessageEvent::ToolCallStart {
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
            Some(AssistantMessageEvent::ToolCallDelta {
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
            Some(AssistantMessageEvent::ToolCallEnd {
                content_index,
                tool_call,
                partial: partial.clone(),
            })
        }
        ProxyAssistantMessageEvent::Done { reason, usage } => {
            partial.stop_reason = reason.clone();
            partial.usage = usage.unwrap_or_default();
            Some(AssistantMessageEvent::Done {
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
            Some(AssistantMessageEvent::Error {
                reason,
                error: partial.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_partial() -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            api: "test".into(),
            provider: "test".into(),
            model: "test".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    #[test]
    fn test_process_proxy_start() {
        let mut partial = make_partial();
        let event = process_proxy_event(
            ProxyAssistantMessageEvent::Start { content_index: 0 },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::Start { .. })));
    }

    #[test]
    fn test_process_proxy_text_flow() {
        let mut partial = make_partial();

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::TextStart { content_index: 0 },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::TextStart { .. })));
        assert_eq!(partial.content.len(), 1);

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hello".into(),
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::TextDelta { .. })));
        if let Some(ContentBlock::Text { text, .. }) = partial.content.get(0) {
            assert_eq!(text, "Hello");
        } else {
            panic!("expected Text block");
        }

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::TextEnd {
                content_index: 0,
                content_signature: None,
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::TextEnd { .. })));
    }

    #[test]
    fn test_process_proxy_thinking_flow() {
        let mut partial = make_partial();

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::ThinkingStart { content_index: 0 },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::ThinkingStart { .. })));

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "I'm thinking".into(),
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::ThinkingDelta { .. })));
        if let Some(ContentBlock::Thinking { thinking, .. }) = partial.content.get(0) {
            assert_eq!(thinking, "I'm thinking");
        }

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content_signature: Some("sig123".into()),
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::ThinkingEnd { .. })));
    }

    #[test]
    fn test_process_proxy_tool_call_flow() {
        let mut partial = make_partial();

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "call-1".into(),
                tool_name: "my_tool".into(),
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::ToolCallStart { .. })));

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: r#"{"key": "value"}"#.into(),
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::ToolCallDelta { .. })));

        let event = process_proxy_event(
            ProxyAssistantMessageEvent::ToolCallEnd { content_index: 0 },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::ToolCallEnd { .. })));
    }

    #[test]
    fn test_process_proxy_done() {
        let mut partial = make_partial();
        let event = process_proxy_event(
            ProxyAssistantMessageEvent::Done {
                reason: StopReason::Stop,
                usage: None,
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::Done { .. })));
        assert_eq!(partial.stop_reason, StopReason::Stop);
    }

    #[test]
    fn test_process_proxy_error() {
        let mut partial = make_partial();
        let event = process_proxy_event(
            ProxyAssistantMessageEvent::Error {
                reason: StopReason::Error,
                error_message: "something went wrong".into(),
                usage: None,
            },
            &mut partial,
        );
        assert!(matches!(event, Some(AssistantMessageEvent::Error { .. })));
        assert_eq!(partial.stop_reason, StopReason::Error);
        assert_eq!(partial.error_message, Some("something went wrong".into()));
    }
}
