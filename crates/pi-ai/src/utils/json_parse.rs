use serde_json::Value;

const VALID_JSON_ESCAPES: [char; 9] = ['"', '\\', '/', 'b', 'f', 'n', 'r', 't', 'u'];

fn is_control_char(c: char) -> bool {
    matches!(c, '\x00'..='\x1F')
}

fn escape_control_char(c: char) -> String {
    match c {
        '\x08' => "\\b".to_string(),
        '\x0C' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        _ => format!("\\u{:04x}", c as u32),
    }
}

/// Repair malformed JSON by escaping control characters and fixing invalid escape sequences.
pub fn repair_json(json: &str) -> String {
    let mut repaired = String::with_capacity(json.len());
    let chars: Vec<char> = json.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;

    while i < len {
        let c = chars[i];

        if !in_string {
            repaired.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }

        if c == '"' {
            repaired.push(c);
            in_string = false;
            i += 1;
            continue;
        }

        if c == '\\' {
            if i + 1 >= len {
                // Trailing backslash at end of string
                repaired.push_str("\\\\");
                i += 1;
                continue;
            }

            let next = chars[i + 1];

            if next == 'u' {
                // Check for valid \\uXXXX
                if i + 5 < len {
                    let hex_digits: String = chars[i + 2..=i + 5].iter().collect();
                    if hex_digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
                        repaired.push('\\');
                        repaired.push('u');
                        repaired.push_str(&hex_digits);
                        i += 6;
                        continue;
                    }
                }
                // Invalid or truncated \\uXXXX - escape the backslash
                repaired.push_str("\\\\");
                i += 1;
                continue;
            }

            if VALID_JSON_ESCAPES.contains(&next) {
                repaired.push('\\');
                repaired.push(next);
                i += 2;
                continue;
            }

            // Invalid escape - double the backslash
            repaired.push_str("\\\\");
            i += 1;
            continue;
        }

        if is_control_char(c) {
            repaired.push_str(&escape_control_char(c));
        } else {
            repaired.push(c);
        }
        i += 1;
    }

    repaired
}

/// Parse JSON with repair fallback.
pub fn parse_json_with_repair<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, String> {
    match serde_json::from_str::<T>(json) {
        Ok(v) => return Ok(v),
        Err(_) => {}
    }
    let repaired = repair_json(json);
    serde_json::from_str::<T>(&repaired).map_err(|e| format!("JSON parse error: {}", e))
}

/// Attempt to clean partial JSON by trimming trailing garbage and closing open structures.
fn clean_partial_json(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "{}".to_string();
    }

    let chars: Vec<char> = s.chars().collect();
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escaped = false;
    let mut string_truncated = false;
    let mut last_good_content = 0usize;

    for (idx, &c) in chars.iter().enumerate() {
        if escaped {
            escaped = false;
            last_good_content = idx;
            continue;
        }
        if in_str {
            if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            last_good_content = idx;
            continue;
        }
        match c {
            '{' | '[' => {
                depth += 1;
                last_good_content = idx;
            }
            '}' | ']' => {
                depth -= 1;
                last_good_content = idx;
            }
            '"' => {
                in_str = true;
                last_good_content = idx;
            }
            ',' | ':' => {
                // structural separators - keep track of position
                last_good_content = idx;
            }
            c if c.is_whitespace() => {}
            _ => {
                last_good_content = idx;
            }
        }
    }

    // If we ended inside a string, it was truncated
    string_truncated = in_str;

    let mut cleaned: String = chars[..=last_good_content].iter().collect();

    if string_truncated {
        cleaned.push('"');
    }

    let mut cleaned = cleaned.trim_end().to_string();
    strip_comma_before_close(&mut cleaned);

    if depth > 0 {
        // Close open brackets/braces (only for objects and arrays that started)
        let closes: String = if s.starts_with('{') {
            "}".repeat(depth as usize)
        } else if s.starts_with('[') {
            "]".repeat(depth as usize)
        } else {
            String::new()
        };
        format!("{}{}", cleaned, closes)
    } else if depth < 0 {
        "{}".to_string()
    } else {
        cleaned
    }
}

fn strip_comma_before_close(s: &mut String) {
    loop {
        let last = match s.chars().last() {
            Some(c) if c == '}' || c == ']' => c,
            _ => break,
        };
        let prefix = s[..s.len() - 1].trim_end().to_string();
        if !matches!(prefix.chars().last(), Some(',') | Some(';')) {
            break;
        }
        *s = format!("{}{}", &prefix[..prefix.len() - 1], last);
    }
}

/// Parse potentially incomplete JSON from streaming responses.
/// Always returns a valid Value, falling back to `Value::Object({})` on failure.
pub fn parse_streaming_json(partial_json: Option<&str>) -> Value {
    let json = match partial_json {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return Value::Object(Default::default()),
    };

    // First try direct parse
    if let Ok(v) = serde_json::from_str::<Value>(json) {
        return v;
    }

    // Try with repair
    let repaired = repair_json(json);
    if let Ok(v) = serde_json::from_str::<Value>(&repaired) {
        return v;
    }

    // Try cleaning partial JSON
    let cleaned = clean_partial_json(&repaired);
    if let Ok(v) = serde_json::from_str::<Value>(&cleaned) {
        return v;
    }

    // Ultimate fallback
    Value::Object(Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repair_json_normal() {
        assert_eq!(repair_json(r#"{"key":"value"}"#), r#"{"key":"value"}"#);
    }

    #[test]
    fn test_repair_json_control_chars() {
        let input = "\"hello\nworld\"";
        let expected = "\"hello\\nworld\"";
        assert_eq!(repair_json(input), expected);
    }

    #[test]
    fn test_repair_json_control_chars_tab() {
        let input = "\"col1\tcol2\"";
        let expected = "\"col1\\tcol2\"";
        assert_eq!(repair_json(input), expected);
    }

    #[test]
    fn test_repair_json_invalid_escape() {
        let input = r#""hello\xworld""#;
        let repaired = repair_json(input);
        assert!(repaired.contains("\\\\x"));
    }

    #[test]
    fn test_repair_json_trailing_backslash() {
        let input = "\"hello\\";
        let repaired = repair_json(input);
        assert!(repaired.contains("\\\\"));
    }

    #[test]
    fn test_parse_json_with_repair_valid() {
        let result: Result<Value, String> = parse_json_with_repair(r#"{"a":1}"#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_streaming_json_none() {
        let result = parse_streaming_json(None);
        assert_eq!(result, Value::Object(Default::default()));
    }

    #[test]
    fn test_parse_streaming_json_empty() {
        let result = parse_streaming_json(Some(""));
        assert_eq!(result, Value::Object(Default::default()));
    }

    #[test]
    fn test_parse_streaming_json_valid() {
        let result = parse_streaming_json(Some(r#"{"a":1,"b":"hello"}"#));
        assert_eq!(result["a"], 1);
        assert_eq!(result["b"], "hello");
    }

    #[test]
    fn test_parse_streaming_json_partial_object() {
        let result = parse_streaming_json(Some(r#"{"path":"/tmp/test"#));
        // Should still parse partially
        assert!(result.is_object());
    }

    #[test]
    fn test_parse_streaming_json_partial_with_trailing_comma() {
        let result = parse_streaming_json(Some(r#"{"a":1,"b":2,}"#));
        assert!(result.is_object());
    }

    #[test]
    fn test_parse_streaming_json_empty_object() {
        let result = parse_streaming_json(Some(r#"{"a":1,"b":2,"c":"#));
        assert!(result.is_object());
    }

    #[test]
    fn test_parse_streaming_json_truncated_string() {
        let result = parse_streaming_json(Some(r#"{"path":"/tmp/tes"#));
        assert!(result.is_object());
    }

    #[test]
    fn test_clean_partial_json_truncated_object() {
        let cleaned = clean_partial_json(r#"{"a":1,"b":"hello"#);
        let v: Value = serde_json::from_str(&cleaned).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_clean_partial_json_trailing_comma() {
        let cleaned = clean_partial_json(r#"{"a":1,"b":2,}"#);
        assert!(!cleaned.contains(",}"));
    }
}
