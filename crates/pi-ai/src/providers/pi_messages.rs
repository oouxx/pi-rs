//! pi-messages API implementation (match TS `api/pi-messages.ts`).
//!
//! Streams pi's own message protocol directly to a backend: the request is a
//! single POST of `{ model, context, options }` to `<baseUrl>/messages`; the
//! response is an SSE stream of serialized assistant-message events plus a
//! terminal `done`/`error` event. This is the wire protocol spoken by the
//! Radius gateway, but any backend implementing it can be used, e.g. via a
//! models.json custom provider with `"api": "pi-messages"`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;

use crate::types::{
    AssistantMessage, AssistantMessageDiagnostic, AssistantMessageEvent, CacheRetention,
    ContentBlock, Context, DiagnosticSeverity, Model, SimpleStreamOptions, StopReason,
    StreamOptions, ToolCall, Usage,
};
use crate::utils::event_stream::AssistantMessageEventStream;

// ============================================================================
// Event types (match TS `PiMessagesEvent`)
// ============================================================================

/// Impact summary of a server-side message rewrite (e.g. a gateway policy).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PiMessagesRewriteImpact {
    pub policy_id: String,
    pub policy_version: u32,
    pub changed: bool,
    pub token_count_change: i64,
    pub message_count_change: i64,
    pub system_prompt_changed: bool,
}

/// Serialized assistant-message event as sent by a pi-messages backend.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PiMessagesEvent {
    Start,
    TextStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
    },
    TextDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
    },
    TextEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
        #[serde(rename = "contentSignature")]
        content_signature: Option<String>,
    },
    ThinkingStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
    },
    ThinkingDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
        #[serde(rename = "contentSignature")]
        content_signature: Option<String>,
        redacted: Option<bool>,
    },
    ToolcallStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
    },
    ToolcallDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
    },
    ToolcallEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        #[serde(rename = "toolCall")]
        tool_call: ToolCall,
    },
    Done {
        reason: String,
        usage: Usage,
        #[serde(rename = "responseId")]
        response_id: Option<String>,
        rewrite: Option<PiMessagesRewriteImpact>,
    },
    Error {
        reason: String,
        usage: Usage,
        #[serde(rename = "errorMessage")]
        error_message: Option<String>,
        #[serde(rename = "responseId")]
        response_id: Option<String>,
        rewrite: Option<PiMessagesRewriteImpact>,
    },
}

fn reason_to_stop_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::Stop,
        "length" => StopReason::Length,
        "toolUse" => StopReason::ToolUse,
        "aborted" => StopReason::Aborted,
        _ => StopReason::Error,
    }
}

// ============================================================================
// Event conversion (match TS `createEventConverter`)
// ============================================================================

/// Convert pi-messages events into pi-ai `AssistantMessageEvent`s, maintaining
/// the assembled `AssistantMessage` (partial) and per-tool JSON scratch buffers.
struct PiMessagesConverter {
    partial: AssistantMessage,
    tool_json: HashMap<usize, String>,
}

impl PiMessagesConverter {
    fn new(model: &Model) -> Self {
        Self {
            partial: AssistantMessage {
                content: vec![],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                raw_stop_reason: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
            tool_json: HashMap::new(),
        }
    }

    fn append_rewrite_diagnostic(&mut self, rewrite: Option<&PiMessagesRewriteImpact>) {
        if let Some(rewrite) = rewrite {
            // pi-rs uses the simplified diagnostic shape (contentIndex /
            // diagnostic / severity); the rewrite impact is serialized into
            // the diagnostic text (DEVIATIONS: TS stores
            // { type: "pi_messages_rewrite", timestamp, details }).
            let text = serde_json::to_string(rewrite).unwrap_or_default();
            let diagnostics = self.partial.diagnostics.get_or_insert_with(Vec::new);
            diagnostics.push(AssistantMessageDiagnostic {
                content_index: 0,
                diagnostic: format!("pi_messages_rewrite: {text}"),
                severity: DiagnosticSeverity::Warning,
            });
        }
    }

    /// Assign a content block at `content_index`, extending the vec as needed
    /// (TS arrays auto-extend on index assignment).
    fn set_content_slot(&mut self, content_index: usize, block: ContentBlock) {
        if self.partial.content.len() <= content_index {
            self.partial
                .content
                .resize(content_index + 1, ContentBlock::text(""));
        }
        self.partial.content[content_index] = block;
    }

    fn convert(&mut self, event: PiMessagesEvent) -> AssistantMessageEvent {
        match event {
            PiMessagesEvent::Start => AssistantMessageEvent::Start {
                partial: self.partial.clone(),
            },
            PiMessagesEvent::TextStart { content_index } => {
                self.set_content_slot(content_index, ContentBlock::text(""));
                AssistantMessageEvent::TextStart {
                    content_index,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::TextDelta { content_index, delta } => {
                if let Some(ContentBlock::Text { text, .. }) = self.partial.content.get_mut(content_index) {
                    text.push_str(&delta);
                }
                AssistantMessageEvent::TextDelta {
                    content_index,
                    delta,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::TextEnd {
                content_index,
                content,
                content_signature,
            } => {
                if let Some(ContentBlock::Text { text, text_signature, .. }) =
                    self.partial.content.get_mut(content_index)
                {
                    *text = content.clone();
                    *text_signature = content_signature.clone();
                }
                AssistantMessageEvent::TextEnd {
                    content_index,
                    content,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::ThinkingStart { content_index } => {
                self.set_content_slot(
                    content_index,
                    ContentBlock::Thinking {
                    thinking: String::new(),
                    thinking_signature: None,
                        redacted: None,
                    },
                );
                AssistantMessageEvent::ThinkingStart {
                    content_index,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::ThinkingDelta { content_index, delta } => {
                if let Some(ContentBlock::Thinking { thinking, .. }) =
                    self.partial.content.get_mut(content_index)
                {
                    thinking.push_str(&delta);
                }
                AssistantMessageEvent::ThinkingDelta {
                    content_index,
                    delta,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::ThinkingEnd {
                content_index,
                content,
                content_signature,
                redacted,
            } => {
                if let Some(ContentBlock::Thinking {
                    thinking,
                    thinking_signature,
                    redacted: r,
                    ..
                }) = self.partial.content.get_mut(content_index)
                {
                    *thinking = content.clone();
                    *thinking_signature = content_signature.clone();
                    *r = redacted;
                }
                AssistantMessageEvent::ThinkingEnd {
                    content_index,
                    content,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::ToolcallStart {
                content_index,
                id,
                tool_name,
            } => {
                self.set_content_slot(
                    content_index,
                    ContentBlock::ToolCall {
                    id: id.clone(),
                    name: tool_name.clone(),
                        arguments: serde_json::json!({}),
                        thought_signature: None,
                    },
                );
                self.tool_json.insert(content_index, String::new());
                AssistantMessageEvent::ToolCallStart {
                    content_index,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::ToolcallDelta { content_index, delta } => {
                let json = format!(
                    "{}{}",
                    self.tool_json.get(&content_index).cloned().unwrap_or_default(),
                    delta
                );
                self.tool_json.insert(content_index, json.clone());
                if let Some(ContentBlock::ToolCall { arguments, .. }) =
                    self.partial.content.get_mut(content_index)
                {
                    *arguments = crate::utils::json_parse::parse_streaming_json(Some(&json));
                }
                AssistantMessageEvent::ToolCallDelta {
                    content_index,
                    delta,
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::ToolcallEnd {
                content_index,
                tool_call,
            } => {
                self.partial.content[content_index] = ContentBlock::ToolCall {
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: tool_call.arguments.clone(),
                    thought_signature: None,
                };
                self.tool_json.remove(&content_index);
                AssistantMessageEvent::ToolCallEnd {
                    content_index,
                    tool_call: ToolCall {
                        type_field: "toolCall".to_string(),
                        id: tool_call.id,
                        name: tool_call.name,
                        arguments: tool_call.arguments,
                        thought_signature: None,
                    },
                    partial: self.partial.clone(),
                }
            }
            PiMessagesEvent::Done {
                reason,
                usage,
                response_id,
                rewrite,
            } => {
                self.partial.stop_reason = reason_to_stop_reason(&reason);
                self.partial.usage = usage;
                self.partial.response_id = response_id;
                self.append_rewrite_diagnostic(rewrite.as_ref());
                AssistantMessageEvent::Done {
                    reason: self.partial.stop_reason.clone(),
                    message: self.partial.clone(),
                }
            }
            PiMessagesEvent::Error {
                reason,
                usage,
                error_message,
                response_id,
                rewrite,
            } => {
                let stop_reason = reason_to_stop_reason(&reason);
                self.partial.stop_reason = stop_reason.clone();
                self.partial.usage = usage;
                self.partial.error_message = error_message;
                self.partial.response_id = response_id;
                self.append_rewrite_diagnostic(rewrite.as_ref());
                AssistantMessageEvent::Error {
                    reason: stop_reason,
                    error: self.partial.clone(),
                }
            }
        }
    }
}

// ============================================================================
// SSE parsing (match TS `readPiMessagesEvents` / `parsePiMessagesEvent`)
// ============================================================================

fn parse_pi_messages_event(raw: &str) -> Option<PiMessagesEvent> {
    let data = raw
        .lines()
        .find(|line| line.starts_with("data:"))
        .map(|line| line[5..].trim())?;
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    serde_json::from_str(data).ok()
}

fn read_pi_messages_events(body: &[u8]) -> Vec<PiMessagesEvent> {
    let text = String::from_utf8_lossy(body).replace("\r\n", "\n");
    let mut events = Vec::new();
    for chunk in text.split("\n\n") {
        if let Some(event) = parse_pi_messages_event(chunk) {
            events.push(event);
        }
    }
    events
}

// ============================================================================
// Error handling (match TS `PiMessagesResponseError`)
// ============================================================================

fn parse_pi_messages_error_body(body: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    let error = parsed.get("error")?;
    if error.is_object() {
        Some(parsed)
    } else {
        None
    }
}

fn format_pi_messages_response_error(
    status: u16,
    status_text: &str,
    body: &str,
    error_body: Option<&Value>,
) -> String {
    let message = error_body
        .and_then(|b| b.get("error"))
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str);
    let code = error_body
        .and_then(|b| b.get("error"))
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str);
    let suffix = message.unwrap_or(body);
    let code_suffix = code.map(|c| format!(" ({c})")).unwrap_or_default();
    format!("{status} {status_text}: {suffix}{code_suffix}")
}


// ============================================================================
// Options
// ============================================================================

fn resolve_cache_retention(options: Option<&StreamOptions>) -> Option<String> {
    if let Some(CacheRetention::Long) = options.and_then(|o| o.cache_retention.as_ref()) {
        return Some("long".to_string());
    }
    if std::env::var("PI_CACHE_RETENTION").as_deref() == Ok("long") {
        return Some("long".to_string());
    }
    None
}

// ============================================================================
// Main streaming function
// ============================================================================

/// Stream a completion from a pi-messages backend.
#[must_use]
pub fn stream_pi_messages(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> AssistantMessageEventStream {
    let model = model.clone();
    let context = context.clone();
    let owned_options = options.cloned();
    let api_key = owned_options
        .as_ref()
        .and_then(|o| o.api_key.clone())
        .or_else(|| crate::env_api_keys::get_env_api_key(&model.provider));

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        let result = stream_pi_messages_inner(
            &model,
            &context,
            owned_options.as_ref(),
            api_key.as_deref(),
            &tx,
        )
        .await;
        if let Err(e) = result {
            let reason = if owned_options
                .as_ref()
                .and_then(|o| o.signal.as_ref())
                .map(|s| *s.borrow())
                .unwrap_or(false)
            {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            let _ = tx.send(AssistantMessageEvent::Error {
                reason: reason.clone(),
                error: AssistantMessage {
                    content: vec![],
                    api: model.api.clone(),
                    provider: model.provider.clone(),
                    model: model.id.clone(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: Usage::default(),
                    stop_reason: reason,
                    error_message: Some(e.to_string()),
                    raw_stop_reason: None,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                },
            });
        }
    });

    AssistantMessageEventStream::from_receiver(rx)
}

async fn stream_pi_messages_inner(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
    api_key: Option<&str>,
    tx: &tokio::sync::mpsc::UnboundedSender<AssistantMessageEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // The pi-messages protocol requires an api key (match TS).
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Err(format!("No API key provided for provider \"{}\"", model.provider).into());
    };
    let signal = options.and_then(|o| o.signal.clone());

    let url = format!(
        "{}/messages",
        model.base_url.trim_end_matches('/')
    );
    let payload = json!({
        "model": model.id,
        "context": context,
        "options": {
            "temperature": options.and_then(|o| o.temperature),
            "maxTokens": options.and_then(|o| o.max_tokens),
            "reasoning": options.and_then(|o| o.reasoning_effort.clone()),
            "cacheRetention": resolve_cache_retention(options),
            "sessionId": options.and_then(|o| o.session_id.clone()),
            "toolChoice": options.and_then(|o| o.tool_choice.clone()),
        },
    });

    // Check abort signal before sending.
    if let Some(ref rx) = signal {
        if *rx.borrow() {
            return Err("Request was aborted".into());
        }
    }

    let client = reqwest::Client::new();
    let mut request = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json")
        .json(&payload);
    if let Some(headers) = options.and_then(|o| o.headers.clone()) {
        for (k, v) in headers {
            request = request.header(k, v);
        }
    }
    let response = request.send().await?;
    let status = response.status();
    let status_text = status
        .canonical_reason()
        .unwrap_or("")
        .to_string();
    let response_bytes = response.bytes().await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&response_bytes).to_string();
        let error_body = parse_pi_messages_error_body(&body);
        let message =
            format_pi_messages_response_error(status.as_u16(), &status_text, &body, error_body.as_ref());
        return Err(message.into());
    }
    if response_bytes.is_empty() {
        return Err(format!("{} response has no body", model.provider).into());
    }

    let events = read_pi_messages_events(&response_bytes);
    let mut converter = PiMessagesConverter::new(model);
    let mut terminal = false;
    for event in events {
        let converted = converter.convert(event);
        let is_terminal = matches!(
            converted,
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
        );
        let _ = tx.send(converted);
        if is_terminal {
            terminal = true;
            break;
        }
    }
    if !terminal {
        return Err(format!("{} stream ended without a terminal event", model.provider).into());
    }

    Ok(())
}

/// Stream a completion from a pi-messages backend with simplified options.
#[must_use]
pub fn stream_simple_pi_messages(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let mut full_opts = StreamOptions::default();
    if let Some(opts) = options {
        full_opts.temperature = opts.base.temperature;
        full_opts.max_tokens = opts.base.max_tokens;
        full_opts.signal.clone_from(&opts.base.signal);
        full_opts.api_key.clone_from(&opts.base.api_key);
        full_opts.transport.clone_from(&opts.base.transport);
        full_opts.cache_retention.clone_from(&opts.base.cache_retention);
        full_opts.session_id.clone_from(&opts.base.session_id);
        full_opts.headers.clone_from(&opts.base.headers);
        full_opts.timeout_ms = opts.base.timeout_ms;
        full_opts.max_retries = opts.base.max_retries;
        full_opts.max_retry_delay_ms = opts.base.max_retry_delay_ms;
        full_opts.metadata.clone_from(&opts.base.metadata);
        full_opts.reasoning_effort.clone_from(&opts.reasoning);
        full_opts.thinking_budgets.clone_from(&opts.thinking_budgets);
    }
    stream_pi_messages(model, context, Some(&full_opts))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    fn make_model() -> Model {
        Model {
            id: "radius-model".into(),
            name: "Radius Model".into(),
            api: "pi-messages".into(),
            provider: "radius".into(),
            base_url: "https://gateway.example".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: crate::types::ModelCost {
                input: 1.0,
                output: 2.0,
                cache_read: 0.1,
                cache_write: 0.2,
                tiers: vec![],
            },
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn test_parse_pi_messages_event() {
        let raw = "data: {\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"Hello\"}\n";
        let event = parse_pi_messages_event(raw).unwrap();
        match event {
            PiMessagesEvent::TextDelta { content_index, delta } => {
                assert_eq!(content_index, 0);
                assert_eq!(delta, "Hello");
            }
            _ => panic!("expected text_delta"),
        }
        assert!(parse_pi_messages_event("data: [DONE]").is_none());
        assert!(parse_pi_messages_event("").is_none());
    }

    #[test]
    fn test_read_pi_messages_events() {
        let body = b"data: {\"type\":\"start\"}\n\ndata: {\"type\":\"text_start\",\"contentIndex\":0}\n\ndata: {\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"Hi\"}\n\ndata: {\"type\":\"done\",\"reason\":\"stop\",\"usage\":{\"input\":1,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":3,\"cost\":{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0,\"total\":0}}}\n\n";
        let events = read_pi_messages_events(body);
        assert_eq!(events.len(), 4);
        assert!(matches!(events[3], PiMessagesEvent::Done { .. }));
    }

    #[test]
    fn test_converter_text_sequence() {
        let model = make_model();
        let mut converter = PiMessagesConverter::new(&model);
        let events = vec![
            PiMessagesEvent::Start,
            PiMessagesEvent::TextStart { content_index: 0 },
            PiMessagesEvent::TextDelta {
                content_index: 0,
                delta: "Hello".into(),
            },
            PiMessagesEvent::TextDelta {
                content_index: 0,
                delta: " world".into(),
            },
            PiMessagesEvent::TextEnd {
                content_index: 0,
                content: "Hello world".into(),
                content_signature: Some("sig-1".into()),
            },
            PiMessagesEvent::Done {
                reason: "stop".into(),
                usage: Usage::default(),
                response_id: Some("resp_1".into()),
                rewrite: None,
            },
        ];
        let mut converted = Vec::new();
        for e in events {
            converted.push(converter.convert(e));
        }
        // Final Done carries the assembled message.
        match converted.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(message.content.len(), 1);
                match &message.content[0] {
                    ContentBlock::Text { text, text_signature, .. } => {
                        assert_eq!(text, "Hello world");
                        assert_eq!(text_signature.as_deref(), Some("sig-1"));
                    }
                    _ => panic!("expected text block"),
                }
                assert_eq!(message.stop_reason, StopReason::Stop);
                assert_eq!(message.response_id.as_deref(), Some("resp_1"));
            }
            _ => panic!("expected done"),
        }
    }

    #[test]
    fn test_converter_toolcall_sequence() {
        let model = make_model();
        let mut converter = PiMessagesConverter::new(&model);
        let events = vec![
            PiMessagesEvent::ToolcallStart {
                content_index: 0,
                id: "call_1".into(),
                tool_name: "get_weather".into(),
            },
            PiMessagesEvent::ToolcallDelta {
                content_index: 0,
                delta: "{\"city\":".into(),
            },
            PiMessagesEvent::ToolcallDelta {
                content_index: 0,
                delta: "\"NYC\"}".into(),
            },
            PiMessagesEvent::ToolcallEnd {
                content_index: 0,
                tool_call: ToolCall {
                    type_field: "toolCall".into(),
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: json!({"city": "NYC"}),
                    thought_signature: None,
                },
            },
            PiMessagesEvent::Done {
                reason: "toolUse".into(),
                usage: Usage::default(),
                response_id: None,
                rewrite: None,
            },
        ];
        let mut converted = Vec::new();
        for e in events {
            converted.push(converter.convert(e));
        }
        // toolcall_delta parses the accumulated JSON incrementally.
        match &converted[2] {
            AssistantMessageEvent::ToolCallDelta { partial, .. } => {
                match &partial.content[0] {
                    ContentBlock::ToolCall { arguments, .. } => {
                        assert_eq!(arguments, &json!({"city": "NYC"}));
                    }
                    _ => panic!("expected tool call block"),
                }
            }
            _ => panic!("expected toolcall_delta"),
        }
        match converted.last().unwrap() {
            AssistantMessageEvent::Done { message, .. } => {
                assert_eq!(message.stop_reason, StopReason::ToolUse);
                match &message.content[0] {
                    ContentBlock::ToolCall { id, name, arguments, .. } => {
                        assert_eq!(id, "call_1");
                        assert_eq!(name, "get_weather");
                        assert_eq!(arguments, &json!({"city": "NYC"}));
                    }
                    _ => panic!("expected tool call block"),
                }
            }
            _ => panic!("expected done"),
        }
    }

    #[test]
    fn test_converter_error_and_rewrite_diagnostic() {
        let model = make_model();
        let mut converter = PiMessagesConverter::new(&model);
        let rewrite = PiMessagesRewriteImpact {
            policy_id: "policy-1".into(),
            policy_version: 3,
            changed: true,
            token_count_change: -12,
            message_count_change: -1,
            system_prompt_changed: true,
        };
        let event = PiMessagesEvent::Error {
            reason: "error".into(),
            usage: Usage::default(),
            error_message: Some("upstream failed".into()),
            response_id: Some("resp_2".into()),
            rewrite: Some(rewrite),
        };
        match converter.convert(event) {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Error);
                assert_eq!(error.error_message.as_deref(), Some("upstream failed"));
                assert_eq!(error.response_id.as_deref(), Some("resp_2"));
                let diags = error.diagnostics.unwrap();
                assert!(diags[0].diagnostic.starts_with("pi_messages_rewrite:"));
                assert!(diags[0].diagnostic.contains("policy-1"));
            }
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn test_error_body_parsing() {
        let body = r#"{"error":{"message":"bad request","code":"E123"}}"#;
        let error_body = parse_pi_messages_error_body(body).unwrap();
        let message = format_pi_messages_response_error(400, "Bad Request", body, Some(&error_body));
        assert!(message.contains("bad request"));
        assert!(message.contains("E123"));
        // Non-object error field is not parsed.
        assert!(parse_pi_messages_error_body(r#"{"error":"boom"}"#).is_none());
    }

    #[tokio::test]
    async fn test_stream_pi_messages_end_to_end() {
        // Mock pi-messages backend: serves an SSE event stream for POST /messages.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 8192];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                let body = concat!(
                    "data: {\"type\":\"start\"}\n\n",
                    "data: {\"type\":\"text_start\",\"contentIndex\":0}\n\n",
                    "data: {\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"Hello\"}\n\n",
                    "data: {\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\" world\"}\n\n",
                    "data: {\"type\":\"done\",\"reason\":\"stop\",\"usage\":{\"input\":1,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":3,\"cost\":{\"input\":0,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0,\"total\":0}}}\n\n",
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, resp.as_bytes()).await;
            }
        });

        let mut model = make_model();
        model.base_url = format!("http://{addr}");
        let context = Context {
            system_prompt: Some("You are helpful".into()),
            messages: vec![],
            tools: None,
        };
        let opts = StreamOptions {
            api_key: Some("test-key".into()),
            ..Default::default()
        };
        let mut stream = stream_pi_messages(&model, &context, Some(&opts));

        use futures::StreamExt;
        let mut kinds = Vec::new();
        let mut final_message: Option<AssistantMessage> = None;
        while let Some(ev) = stream.next().await {
            match ev {
                AssistantMessageEvent::Start { .. } => kinds.push("start"),
                AssistantMessageEvent::TextStart { .. } => kinds.push("text_start"),
                AssistantMessageEvent::TextDelta { .. } => kinds.push("text_delta"),
                AssistantMessageEvent::Done { message, .. } => {
                    kinds.push("done");
                    final_message = Some(message);
                }
                AssistantMessageEvent::Error { error, .. } => {
                    kinds.push("error");
                    eprintln!("unexpected error: {:?}", error.error_message);
                }
                _ => kinds.push("other"),
            }
        }
        assert_eq!(kinds, vec!["start", "text_start", "text_delta", "text_delta", "done"]);
        let message = final_message.expect("terminal done event");
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.content.len(), 1);
        match &message.content[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "Hello world"),
            _ => panic!("expected text block"),
        }
        assert_eq!(message.usage.total_tokens, 3);
    }

    #[tokio::test]
    async fn test_stream_pi_messages_http_error() {
        // Mock backend returning a structured error body.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 8192];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                let body = r#"{"error":{"message":"upstream failed","code":"E_UPSTREAM"}}"#;
                let resp = format!(
                    "HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, resp.as_bytes()).await;
            }
        });

        let mut model = make_model();
        model.base_url = format!("http://{addr}");
        let context = Context {
            system_prompt: None,
            messages: vec![],
            tools: None,
        };
        let opts = StreamOptions {
            api_key: Some("test-key".into()),
            ..Default::default()
        };
        let mut stream = stream_pi_messages(&model, &context, Some(&opts));

        use futures::StreamExt;
        let mut error_message: Option<String> = None;
        while let Some(ev) = stream.next().await {
            if let AssistantMessageEvent::Error { error, .. } = ev {
                error_message = error.error_message;
            }
        }
        let msg = error_message.expect("error event with message");
        assert!(msg.contains("upstream failed"), "got: {msg}");
        assert!(msg.contains("E_UPSTREAM"), "got: {msg}");
    }
}
