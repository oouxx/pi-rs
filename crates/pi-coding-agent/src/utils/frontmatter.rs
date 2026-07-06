//! Frontmatter parsing (--- yaml --- content).
//!
//! Mirrors packages/coding-agent/src/utils/frontmatter.ts

/// Parsed frontmatter result.
#[derive(Debug, Clone)]
pub struct ParsedFrontmatter {
    pub frontmatter: serde_json::Value,
    pub body: String,
}

/// Normalize newlines to \n.
fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

/// Extract raw YAML frontmatter string from content.
fn extract_frontmatter(content: &str) -> (Option<String>, String) {
    let normalized = normalize_newlines(content);

    if !normalized.starts_with("---") {
        return (None, normalized);
    }

    // Find closing --- in the content after the opening
    // The opening "---\n" occupies indices 0..3, so search from index 3
    if let Some(end_of_front) = normalized[3..].find("\n---") {
        // end_of_front is the position of '\n' before "---" within the slice [3..]
        // In the original string, the '\n' is at 3 + end_of_front
        let end_index = 3 + end_of_front;
        // YAML content is between the opening "---\n" (indices 0..3) and the closing "\n---"
        let yaml_string = if end_index > 4 {
            normalized[4..end_index].to_string()
        } else {
            String::new()
        };
        // Body starts after the closing "\n---" which is at end_index..end_index+4
        // plus the newline after it (or not, trim handles it)
        let body = normalized[end_index + 4..].trim().to_string();
        (Some(yaml_string), body)
    } else {
        (None, normalized)
    }
}

/// Parse frontmatter from content.
///
/// Returns the parsed frontmatter (as JSON Value) and the body text.
/// If no frontmatter is found, returns an empty object and the full content as body.
pub fn parse_frontmatter(content: &str) -> ParsedFrontmatter {
    let (yaml_string, body) = extract_frontmatter(content);

    let frontmatter = match yaml_string {
        Some(yaml) => {
            // Simple YAML key-value parsing (supports: key: value)
            let mut map = serde_json::Map::new();
            for line in yaml.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    let k = key.trim().to_string();
                    let v = value.trim().trim_matches('"').to_string();
                    map.insert(k, serde_json::Value::String(v));
                }
            }
            serde_json::Value::Object(map)
        }
        None => serde_json::Value::Object(serde_json::Map::new()),
    };

    ParsedFrontmatter { frontmatter, body }
}

/// Strip frontmatter from content, returning only the body.
pub fn strip_frontmatter(content: &str) -> String {
    parse_frontmatter(content).body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_frontmatter() {
        let result = parse_frontmatter("hello world");
        assert!(result.frontmatter.as_object().unwrap().is_empty());
        assert_eq!(result.body, "hello world");
    }

    #[test]
    fn test_with_frontmatter() {
        let content = "---\nkey: value\n---\n\nbody text";
        let result = parse_frontmatter(content);
        assert_eq!(result.frontmatter["key"], "value");
        assert_eq!(result.body, "body text");
    }

    #[test]
    fn test_strip_frontmatter() {
        let content = "---\nkey: value\n---\n\nbody text";
        assert_eq!(strip_frontmatter(content), "body text");
    }

    #[test]
    fn test_unclosed_frontmatter() {
        let content = "---\nkey: value";
        let result = parse_frontmatter(content);
        assert!(result.frontmatter.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_empty_frontmatter() {
        let content = "---\n---\n\nbody";
        let result = parse_frontmatter(content);
        assert!(result.frontmatter.as_object().unwrap().is_empty());
        assert_eq!(result.body, "body");
    }
}
