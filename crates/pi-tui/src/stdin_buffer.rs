use tokio::sync::mpsc;

const ESC: char = '\x1b';
const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// Events emitted by StdinBuffer.
#[derive(Debug, Clone, PartialEq)]
pub enum StdinEvent {
    Data(String),
    Paste(String),
}

/// Buffers stdin input and emits complete sequences.
///
/// Handles partial escape sequences that arrive across multiple chunks.
pub struct StdinBuffer {
    buffer: String,
    paste_mode: bool,
    paste_buffer: String,
    pending_kitty_codepoint: Option<u32>,
    timeout_ms: u64,
}

impl StdinBuffer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            paste_mode: false,
            paste_buffer: String::new(),
            pending_kitty_codepoint: None,
            timeout_ms: 10,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Process incoming data. Returns a list of complete sequences.
    pub fn process(&mut self, data: &str) -> Vec<StdinEvent> {
        let mut events = Vec::new();

        if data.is_empty() && self.buffer.is_empty() {
            return events;
        }

        self.buffer.push_str(data);

        if self.paste_mode {
            self.paste_buffer.push_str(&self.buffer);
            self.buffer.clear();

            if let Some(end_idx) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_idx].to_string();
                let remaining = self.paste_buffer[end_idx + BRACKETED_PASTE_END.len()..].to_string();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_codepoint = None;

                events.push(StdinEvent::Paste(pasted_content));

                if !remaining.is_empty() {
                    events.extend(self.process(&remaining));
                }
            }
            return events;
        }

        // Check for bracketed paste start
        if let Some(start_idx) = self.buffer.find(BRACKETED_PASTE_START) {
            if start_idx > 0 {
                let before_paste = self.buffer[..start_idx].to_string();
                let result = extract_complete_sequences(&before_paste);
                for seq in result.sequences {
                    if let Some(ev) = self.emit_data_sequence(&seq) {
                        events.push(ev);
                    }
                }
            }

            self.pending_kitty_codepoint = None;
            self.buffer = self.buffer[start_idx + BRACKETED_PASTE_START.len()..].to_string();
            self.paste_mode = true;
            self.paste_buffer = self.buffer.clone();
            self.buffer.clear();

            if let Some(end_idx) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted_content = self.paste_buffer[..end_idx].to_string();
                let remaining = self.paste_buffer[end_idx + BRACKETED_PASTE_END.len()..].to_string();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_codepoint = None;

                events.push(StdinEvent::Paste(pasted_content));

                if !remaining.is_empty() {
                    events.extend(self.process(&remaining));
                }
            }
            return events;
        }

        let result = extract_complete_sequences(&self.buffer);
        self.buffer = result.remainder;

        for seq in result.sequences {
            if let Some(ev) = self.emit_data_sequence(&seq) {
                events.push(ev);
            }
        }

        events
    }

    fn emit_data_sequence(&mut self, sequence: &str) -> Option<StdinEvent> {
        if sequence.len() == 1 {
            let cp = sequence.chars().next().map(|c| c as u32);
            if let Some(codepoint) = cp {
                if Some(codepoint) == self.pending_kitty_codepoint {
                    self.pending_kitty_codepoint = None;
                    return None;
                }
            }
        }

        self.pending_kitty_codepoint = parse_unmodified_kitty_codepoint(sequence);
        Some(StdinEvent::Data(sequence.to_string()))
    }

    /// Flush any remaining buffered data (for timeout-based flushing).
    pub fn flush(&mut self) -> Vec<String> {
        if self.buffer.is_empty() {
            return Vec::new();
        }

        let sequences = vec![self.buffer.clone()];
        self.buffer.clear();
        self.pending_kitty_codepoint = None;
        sequences
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.paste_mode = false;
        self.paste_buffer.clear();
        self.pending_kitty_codepoint = None;
    }

    pub fn get_buffer(&self) -> &str {
        &self.buffer
    }

    /// Create an async channel for receiving processed events.
    pub fn create_channel() -> (mpsc::UnboundedSender<String>, mpsc::UnboundedReceiver<String>) {
        mpsc::unbounded_channel()
    }
}

/// Check if a string is a complete escape sequence or needs more data.
fn is_complete_sequence(data: &str) -> SequenceStatus {
    if !data.starts_with(ESC) {
        return SequenceStatus::NotEscape;
    }

    if data.len() == 1 {
        return SequenceStatus::Incomplete;
    }

    let after_esc = &data[1..];

    if after_esc.starts_with('[') {
        // CSI sequences: ESC [
        if after_esc.starts_with("[M") {
            return if data.len() >= 6 {
                SequenceStatus::Complete
            } else {
                SequenceStatus::Incomplete
            };
        }
        return is_complete_csi(data);
    }

    if after_esc.starts_with(']') {
        return is_complete_osc(data);
    }

    if after_esc.starts_with('P') {
        return is_complete_dcs(data);
    }

    if after_esc.starts_with('_') {
        return is_complete_apc(data);
    }

    // SS3 sequences: ESC O
    if after_esc.starts_with('O') {
        return if after_esc.len() >= 2 {
            SequenceStatus::Complete
        } else {
            SequenceStatus::Incomplete
        };
    }

    // Meta key: ESC followed by single char
    if after_esc.len() == 1 {
        return SequenceStatus::Complete;
    }

    SequenceStatus::Complete
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SequenceStatus {
    Complete,
    Incomplete,
    NotEscape,
}

fn is_complete_csi(data: &str) -> SequenceStatus {
    if !data.starts_with(&format!("{}[", ESC)) {
        return SequenceStatus::Complete;
    }

    if data.len() < 3 {
        return SequenceStatus::Incomplete;
    }

    let payload = &data[2..];
    let last_char = payload.chars().last().unwrap();
    let last_code = last_char as u32;

    if (0x40..=0x7e).contains(&last_code) {
        // SGR mouse sequences: ESC[<B;X;Ym or ESC[<B;X;YM
        if payload.starts_with('<') {
            let mouse_pattern = format!(r"^<\d+;\d+;\d+[Mm]$");
            let re = regex_lite::Regex::new(&mouse_pattern).unwrap();
            if re.is_match(payload) {
                return SequenceStatus::Complete;
            }
            if last_char == 'M' || last_char == 'm' {
                let inner = &payload[1..payload.len() - 1];
                let parts: Vec<&str> = inner.split(';').collect();
                if parts.len() == 3 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) {
                    return SequenceStatus::Complete;
                }
            }
            return SequenceStatus::Incomplete;
        }
        return SequenceStatus::Complete;
    }

    SequenceStatus::Incomplete
}

fn is_complete_osc(data: &str) -> SequenceStatus {
    if !data.starts_with(&format!("{}]", ESC)) {
        return SequenceStatus::Complete;
    }

    if data.ends_with(&format!("{}\\", ESC)) || data.ends_with('\x07') {
        return SequenceStatus::Complete;
    }

    SequenceStatus::Incomplete
}

fn is_complete_dcs(data: &str) -> SequenceStatus {
    if !data.starts_with(&format!("{}P", ESC)) {
        return SequenceStatus::Complete;
    }

    if data.ends_with(&format!("{}\\", ESC)) {
        return SequenceStatus::Complete;
    }

    SequenceStatus::Incomplete
}

fn is_complete_apc(data: &str) -> SequenceStatus {
    if !data.starts_with(&format!("{}_", ESC)) {
        return SequenceStatus::Complete;
    }

    if data.ends_with(&format!("{}\\", ESC)) {
        return SequenceStatus::Complete;
    }

    SequenceStatus::Incomplete
}

struct ExtractResult {
    sequences: Vec<String>,
    remainder: String,
}

fn extract_complete_sequences(buffer: &str) -> ExtractResult {
    let mut sequences = Vec::new();
    let mut pos = 0;
    let len = buffer.len();

    while pos < len {
        let remaining = &buffer[pos..];

        if remaining.starts_with(ESC) {
            let mut found = false;
            let mut seq_end = 1;
            while seq_end <= remaining.len() {
                let candidate = &remaining[..seq_end];
                match is_complete_sequence(candidate) {
                    SequenceStatus::Complete => {
                        if candidate == "\x1b\x1b" {
                            if seq_end < remaining.len() {
                                let next = remaining.as_bytes()[seq_end];
                                if next == b'['
                                    || next == b']'
                                    || next == b'O'
                                    || next == b'P'
                                    || next == b'_'
                                {
                                    sequences.push(ESC.to_string());
                                    pos += 1;
                                    found = true;
                                    break;
                                }
                            }
                        }
                        sequences.push(candidate.to_string());
                        pos += seq_end;
                        found = true;
                        break;
                    }
                    SequenceStatus::Incomplete => {
                        seq_end += 1;
                    }
                    SequenceStatus::NotEscape => {
                        sequences.push(candidate.to_string());
                        pos += seq_end;
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                return ExtractResult {
                    sequences,
                    remainder: remaining.to_string(),
                };
            }
        } else {
            sequences.push(buffer[pos..pos + 1].to_string());
            pos += 1;
        }
    }

    ExtractResult {
        sequences,
        remainder: String::new(),
    }
}

fn parse_unmodified_kitty_codepoint(sequence: &str) -> Option<u32> {
    let re = regex_lite::Regex::new(r"^\x1b\[(\d+)(:\d*)?(:\d+)?u$").ok()?;
    let caps = re.captures(sequence)?;
    let codepoint: u32 = caps[1].parse().ok()?;
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_char() {
        let mut buf = StdinBuffer::new();
        let events = buf.process("a");
        assert_eq!(events, vec![StdinEvent::Data("a".to_string())]);
    }

    #[test]
    fn test_multiple_chars() {
        let mut buf = StdinBuffer::new();
        let events = buf.process("hello");
        assert_eq!(events.len(), 5);
        for ev in &events {
            match ev {
                StdinEvent::Data(s) => assert_eq!(s.len(), 1),
                _ => panic!("expected Data events"),
            }
        }
    }

    #[test]
    fn test_escape_complete() {
        let mut buf = StdinBuffer::new();
        let events = buf.process("\x1b[A"); // cursor up
        assert_eq!(events, vec![StdinEvent::Data("\x1b[A".to_string())]);
    }

    #[test]
    fn test_escape_partial_then_complete() {
        let mut buf = StdinBuffer::new();
        let events1 = buf.process("\x1b[");
        assert!(events1.is_empty()); // incomplete

        let events2 = buf.process("A");
        assert_eq!(events2, vec![StdinEvent::Data("\x1b[A".to_string())]);
    }

    #[test]
    fn test_bracketed_paste() {
        let mut buf = StdinBuffer::new();
        let events = buf.process(&format!("{}hello world{}", BRACKETED_PASTE_START, BRACKETED_PASTE_END));
        assert_eq!(events, vec![StdinEvent::Paste("hello world".to_string())]);
    }

    #[test]
    fn test_text_before_paste() {
        let mut buf = StdinBuffer::new();
        let events = buf.process(&format!("a{}hello{}", BRACKETED_PASTE_START, BRACKETED_PASTE_END));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], StdinEvent::Data("a".to_string()));
        assert_eq!(events[1], StdinEvent::Paste("hello".to_string()));
    }

    #[test]
    fn test_partial_paste_chunks() {
        let mut buf = StdinBuffer::new();
        let events1 = buf.process("\x1b[200~");
        assert!(events1.is_empty());
        assert!(buf.paste_mode);

        let events2 = buf.process("paste content");
        assert!(events2.is_empty());
        assert!(buf.paste_mode);

        let events3 = buf.process("\x1b[201~");
        assert_eq!(events3, vec![StdinEvent::Paste("paste content".to_string())]);
        assert!(!buf.paste_mode);
    }

    #[test]
    fn test_flush() {
        let mut buf = StdinBuffer::new();
        buf.process("\x1b["); // incomplete
        assert!(buf.get_buffer().len() > 0);

        let flushed = buf.flush();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0], "\x1b[");
        assert!(buf.get_buffer().is_empty());
    }

    #[test]
    fn test_clear() {
        let mut buf = StdinBuffer::new();
        buf.process("\x1b["); // incomplete
        buf.clear();
        assert!(buf.get_buffer().is_empty());
        assert!(!buf.paste_mode);
    }

    #[test]
    fn test_escape_meta_key() {
        let mut buf = StdinBuffer::new();
        let events = buf.process("\x1bx"); // ESC + x = meta key
        assert_eq!(events, vec![StdinEvent::Data("\x1bx".to_string())]);
    }

    #[test]
    fn test_escape_ss3() {
        let mut buf = StdinBuffer::new();
        let events = buf.process("\x1bOP"); // F1
        assert_eq!(events, vec![StdinEvent::Data("\x1bOP".to_string())]);
    }

    #[test]
    fn test_sgr_mouse() {
        let mut buf = StdinBuffer::new();
        let events = buf.process("\x1b[<35;20;5M");
        assert_eq!(events, vec![StdinEvent::Data("\x1b[<35;20;5M".to_string())]);
    }

    #[test]
    fn test_kitty_codepoint_dedup() {
        let mut buf = StdinBuffer::new();
        let events = buf.process("\x1b[97u");
        assert_eq!(events.len(), 1);

        let events2 = buf.process("a");
        assert!(events2.is_empty());
    }

    #[test]
    fn test_is_complete_sequence() {
        assert_eq!(is_complete_sequence("a"), SequenceStatus::NotEscape);
        assert_eq!(is_complete_sequence("\x1b"), SequenceStatus::Incomplete);
        assert_eq!(is_complete_sequence("\x1b[A"), SequenceStatus::Complete);
        assert_eq!(is_complete_sequence("\x1b[200~"), SequenceStatus::Complete);
    }

    #[test]
    fn test_extract_complete_sequences() {
        let result = extract_complete_sequences("ab\x1b[Acd");
        assert_eq!(result.sequences.len(), 5);
        assert_eq!(result.sequences[0], "a");
        assert_eq!(result.sequences[1], "b");
        assert_eq!(result.sequences[2], "\x1b[A");
        assert_eq!(result.sequences[3], "c");
        assert_eq!(result.sequences[4], "d");
    }
}
