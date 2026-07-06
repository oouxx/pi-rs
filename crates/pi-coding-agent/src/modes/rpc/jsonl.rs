//! JSONL (JSON Lines) framing helpers.
//!
//! Mirrors packages/coding-agent/src/modes/rpc/jsonl.ts
//!
//! Framing is LF-only. Payload strings may contain other Unicode separators.
//! Clients must split records on `\n` only.

use tokio::io::{AsyncBufReadExt, BufReader};

/// Read JSONL lines from a buffered reader, calling `on_line` for each
/// complete JSON line. Returns a cleanup function that stops reading.
pub fn attach_jsonl_line_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    reader: R,
    mut on_line: impl FnMut(String) + Send + 'static,
) -> Box<dyn FnOnce() + Send> {
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let running_clone = running.clone();

    let mut buf_reader = BufReader::new(reader);

    tokio::spawn(async move {
        let mut line = String::new();
        loop {
            if !running_clone.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            line.clear();
            match buf_reader.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                    if !trimmed.is_empty() {
                        on_line(trimmed.to_string());
                    }
                }
                Err(_) => break,
            }
        }
    });

    Box::new(move || {
        running.store(false, std::sync::atomic::Ordering::SeqCst);
    })
}

/// Serialize a value to a JSONL line (JSON + newline).
pub fn serialize_json_line(value: &impl serde::Serialize) -> String {
    match serde_json::to_string(value) {
        Ok(s) => format!("{s}\n"),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_json_line() {
        let json = serde_json::json!({"type": "test", "value": 42});
        let line = serialize_json_line(&json);
        assert_eq!(line, concat!(r#"{"type":"test","value":42}"#, "\n"));
    }

    #[tokio::test]
    async fn test_read_lines() {
        let input = "line1\nline2\nline3\n";
        let reader = tokio::io::BufReader::new(input.as_bytes());

        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let lines_clone = lines.clone();

        let _detach = attach_jsonl_line_reader(reader, move |line| {
            lines_clone.lock().unwrap().push(line);
        });

        // Give the async task time to read
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let result = lines.lock().unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "line1");
        assert_eq!(result[1], "line2");
        assert_eq!(result[2], "line3");
    }
}
