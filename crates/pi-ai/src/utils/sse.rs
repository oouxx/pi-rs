//! SSE (Server-Sent Events) parser shared across providers.
//!
//! Ported from the TypeScript `iterateSseMessages` / `decodeSseLine` / `flushSseEvent`
//! functions in the original pi codebase.

/// A parsed SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSentEvent {
    /// The event type field (e.g. "message_start", "content_block_delta").
    pub event: Option<String>,
    /// The concatenated data field(s).
    pub data: String,
    /// Raw lines that make up this event (for debugging).
    pub raw: Vec<String>,
}

/// Mutable state for the SSE line decoder.
#[derive(Debug, Clone, Default)]
pub struct SseDecoderState {
    pub event: Option<String>,
    pub data: Vec<String>,
    pub raw: Vec<String>,
}

/// Flush the current decoder state into a `ServerSentEvent`, if there is
/// anything to flush. Resets the state afterwards.
pub fn flush_sse_event(state: &mut SseDecoderState) -> Option<ServerSentEvent> {
    if state.event.is_none() && state.data.is_empty() {
        return None;
    }

    let event = ServerSentEvent {
        event: state.event.take(),
        data: state.data.join("\n"),
        raw: std::mem::take(&mut state.raw),
    };
    state.data.clear();
    Some(event)
}

/// Decode one SSE line, mutating `state`.
///
/// Returns `Some(ServerSentEvent)` when a blank line is encountered
/// (which signals the end of an event in the SSE protocol).
pub fn decode_sse_line(line: &str, state: &mut SseDecoderState) -> Option<ServerSentEvent> {
    // Blank line → end of event
    if line.is_empty() {
        return flush_sse_event(state);
    }

    state.raw.push(line.to_string());

    // Comment line (starts with ':') — ignore
    if line.starts_with(':') {
        return None;
    }

    let delimiter_index = line.find(':');
    let field_name = match delimiter_index {
        Some(idx) => &line[..idx],
        None => line,
    };
    let mut value = match delimiter_index {
        Some(idx) => &line[idx + 1..],
        None => "",
    };
    // Strip a single leading space after the colon (SSE spec)
    if value.starts_with(' ') {
        value = &value[1..];
    }

    if field_name == "event" {
        state.event = Some(value.to_string());
    } else if field_name == "data" {
        state.data.push(value.to_string());
    }

    None
}

/// Find the index of the next line break (`\r`, `\n`, or `\r\n`) in `text`.
fn next_line_break_index(text: &str) -> Option<usize> {
    let cr = text.find('\r');
    let lf = text.find('\n');
    match (cr, lf) {
        (None, None) => None,
        (Some(c), None) => Some(c),
        (None, Some(n)) => Some(n),
        (Some(c), Some(n)) => Some(c.min(n)),
    }
}

/// Consume one line from the beginning of `text`.
///
/// Returns `Some((line, rest))` where `line` does NOT include the line
/// terminator and `rest` is everything after it (including `\r\n` consumed as one).
/// Returns `None` if no complete line is available yet.
pub fn consume_line(text: &str) -> Option<(&str, &str)> {
    let line_break_idx = next_line_break_index(text)?;

    let mut next_idx = line_break_idx + 1;
    // Handle \r\n as a single line break
    let line_len = if text.as_bytes()[line_break_idx] == b'\r'
        && next_idx < text.len()
        && text.as_bytes()[next_idx] == b'\n'
    {
        next_idx += 1;
        line_break_idx
    } else {
        line_break_idx
    };

    Some((&text[..line_len], &text[next_idx..]))
}

/// Parse a full SSE byte stream into a Vec of ServerSentEvents.
///
/// This is a synchronous function that takes the complete response body
/// as bytes and returns all parsed events. Useful for testing.
pub fn parse_sse_body(body: &[u8]) -> Vec<ServerSentEvent> {
    let text = String::from_utf8_lossy(body);
    let mut state = SseDecoderState::default();
    let mut events = Vec::new();
    let mut remaining = text.as_ref();

    while let Some((line, rest)) = consume_line(remaining) {
        remaining = rest;
        if let Some(event) = decode_sse_line(line, &mut state) {
            events.push(event);
        }
    }

    // Handle trailing data (no final blank line)
    if !remaining.is_empty() {
        if let Some(event) = decode_sse_line(remaining, &mut state) {
            events.push(event);
        }
    }

    // Flush any remaining event
    if let Some(event) = flush_sse_event(&mut state) {
        events.push(event);
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // Tests for consume_line
    // ============================================================

    #[test]
    fn test_consume_line_simple() {
        let (line, rest) = consume_line("hello\nworld").unwrap();
        assert_eq!(line, "hello");
        assert_eq!(rest, "world");
    }

    #[test]
    fn test_consume_line_crlf() {
        let (line, rest) = consume_line("hello\r\nworld").unwrap();
        assert_eq!(line, "hello");
        assert_eq!(rest, "world");
    }

    #[test]
    fn test_consume_line_cr_only() {
        let (line, rest) = consume_line("hello\rworld").unwrap();
        assert_eq!(line, "hello");
        assert_eq!(rest, "world");
    }

    #[test]
    fn test_consume_line_incomplete() {
        assert!(consume_line("no_newline").is_none());
    }

    #[test]
    fn test_consume_line_empty() {
        let (line, rest) = consume_line("\nrest").unwrap();
        assert_eq!(line, "");
        assert_eq!(rest, "rest");
    }

    // ============================================================
    // Tests for decode_sse_line
    // ============================================================

    #[test]
    fn test_decode_sse_line_event_field() {
        let mut state = SseDecoderState::default();
        let result = decode_sse_line("event: message_start", &mut state);
        assert!(result.is_none());
        assert_eq!(state.event, Some("message_start".to_string()));
    }

    #[test]
    fn test_decode_sse_line_data_field() {
        let mut state = SseDecoderState::default();
        let result = decode_sse_line("data: {\"type\":\"test\"}", &mut state);
        assert!(result.is_none());
        assert_eq!(state.data, vec!["{\"type\":\"test\"}"]);
    }

    #[test]
    fn test_decode_sse_line_data_without_space() {
        let mut state = SseDecoderState::default();
        let result = decode_sse_line("data:{\"key\":\"value\"}", &mut state);
        assert!(result.is_none());
        assert_eq!(state.data, vec!["{\"key\":\"value\"}"]);
    }

    #[test]
    fn test_decode_sse_line_comment() {
        let mut state = SseDecoderState::default();
        let result = decode_sse_line(": this is a comment", &mut state);
        assert!(result.is_none());
        assert!(state.event.is_none());
        assert!(state.data.is_empty());
    }

    #[test]
    fn test_decode_sse_line_field_without_value() {
        let mut state = SseDecoderState::default();
        let result = decode_sse_line("event", &mut state);
        assert!(result.is_none());
        assert_eq!(state.event, Some("".to_string()));
    }

    #[test]
    fn test_decode_sse_line_blank_triggers_flush() {
        let mut state = SseDecoderState {
            event: Some("message_start".to_string()),
            data: vec!["{}".to_string()],
            raw: vec!["event: message_start".to_string(), "data: {}".to_string()],
        };
        let result = decode_sse_line("", &mut state);
        assert!(result.is_some());
        let event = result.unwrap();
        assert_eq!(event.event, Some("message_start".to_string()));
        assert_eq!(event.data, "{}");
        // State should be reset
        assert!(state.event.is_none());
        assert!(state.data.is_empty());
        assert!(state.raw.is_empty());
    }

    // ============================================================
    // Tests for flush_sse_event
    // ============================================================

    #[test]
    fn test_flush_empty_state_returns_none() {
        let mut state = SseDecoderState::default();
        assert!(flush_sse_event(&mut state).is_none());
    }

    #[test]
    fn test_flush_resets_state() {
        let mut state = SseDecoderState {
            event: Some("test".to_string()),
            data: vec!["hello".to_string()],
            raw: vec!["event: test".to_string(), "data: hello".to_string()],
        };
        let event = flush_sse_event(&mut state).unwrap();
        assert_eq!(event.event, Some("test".to_string()));
        assert_eq!(event.data, "hello");
        assert_eq!(event.raw.len(), 2);
        // State reset
        assert!(state.event.is_none());
        assert!(state.data.is_empty());
        assert!(state.raw.is_empty());
    }

    #[test]
    fn test_flush_multiple_data_lines() {
        let mut state = SseDecoderState {
            event: Some("delta".to_string()),
            data: vec!["line1".to_string(), "line2".to_string()],
            raw: vec![],
        };
        let event = flush_sse_event(&mut state).unwrap();
        assert_eq!(event.data, "line1\nline2");
    }

    // ============================================================
    // Tests for parse_sse_body (integration)
    // ============================================================

    #[test]
    fn test_parse_sse_simple_event() {
        let body = b"event: test\ndata: hello\n\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, Some("test".to_string()));
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn test_parse_sse_multiple_events() {
        let body = b"event: e1\ndata: d1\n\nevent: e2\ndata: d2\n\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, Some("e1".to_string()));
        assert_eq!(events[0].data, "d1");
        assert_eq!(events[1].event, Some("e2".to_string()));
        assert_eq!(events[1].data, "d2");
    }

    #[test]
    fn test_parse_sse_data_only_no_event_field() {
        let body = b"data: payload\n\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, None);
        assert_eq!(events[0].data, "payload");
    }

    #[test]
    fn test_parse_sse_multi_line_data() {
        let body = b"event: msg\ndata: {\ndata: \"key\": 1\ndata: }\n\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "{\n\"key\": 1\n}");
    }

    #[test]
    fn test_parse_sse_comment_lines_ignored() {
        let body = b": comment line\nevent: real\ndata: value\n\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, Some("real".to_string()));
        assert_eq!(events[0].data, "value");
    }

    #[test]
    fn test_parse_sse_crlf() {
        let body = b"event: test\r\ndata: value\r\n\r\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, Some("test".to_string()));
        assert_eq!(events[0].data, "value");
    }

    #[test]
    fn test_parse_sse_no_trailing_blank_line() {
        // Some servers don't send a final blank line
        let body = b"event: last\ndata: final";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, Some("last".to_string()));
        assert_eq!(events[0].data, "final");
    }

    #[test]
    fn test_parse_sse_anthropic_message_start() {
        let body = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_001\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"usage\":{\"input_tokens\":100,\"output_tokens\":0}}}\n\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, Some("message_start".to_string()));
        assert!(events[0].data.contains("msg_001"));
    }

    #[test]
    fn test_parse_sse_anthropic_content_block_delta() {
        let body = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n";
        let events = parse_sse_body(body);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, Some("content_block_delta".to_string()));
        assert!(events[0].data.contains("text_delta"));
    }
}
