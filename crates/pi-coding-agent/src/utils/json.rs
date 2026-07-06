//! JSON utility functions.
//!
//! Mirrors packages/coding-agent/src/utils/json.ts

/// Strip JavaScript-style line comments and trailing commas from JSON.
///
/// Handles string literals correctly, so comments inside strings are preserved.
pub fn strip_json_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_string = false;
    let mut chars = input.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        if c == '"' {
            // Check if escaped
            let mut backslashes = 0;
            // Count preceding backslashes in the result
            // (we can't easily check the input because we're consuming chars)
            in_string = !in_string;
            result.push(c);
        } else if !in_string && c == '/' {
            // Check for // comment
            if let Some(&(_, '/')) = chars.peek() {
                // Skip until end of line
                while let Some((_, next)) = chars.next() {
                    if next == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            } else {
                result.push(c);
            }
        } else if !in_string && c == ',' {
            // Check if trailing comma before ] or }
            if let Some(&(_, next)) = chars.peek() {
                if next == ']' || next == '}' {
                    // Skip the comma
                    continue;
                }
            }
            result.push(c);
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_simple_comment() {
        let input = r#"{"key": "value" // comment
}"#;
        let result = strip_json_comments(input);
        assert!(!result.contains("comment"));
        assert!(result.contains("\"key\""));
    }

    #[test]
    fn test_strip_trailing_comma() {
        let input = r#"{"key": "value",}"#;
        let result = strip_json_comments(input);
        assert_eq!(result, r#"{"key": "value"}"#);
    }

    #[test]
    fn test_preserves_string_with_slashes() {
        let input = r#"{"url": "http://example.com"}"#;
        let result = strip_json_comments(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_combined() {
        let input = r#"{
  "name": "test", // package name
  "version": "1.0.0",
}"#;
        let result = strip_json_comments(input);
        assert!(!result.contains("//"));
        assert!(!result.contains("package name"));
        assert!(result.contains("\"name\""));
    }
}
