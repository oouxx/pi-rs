pub fn str_val(val: Option<&serde_json::Value>) -> Option<&str> {
    val.and_then(|v| v.as_str())
}

pub fn get_text_output(content: &[serde_json::Value]) -> String {
    content
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                block.get("text").and_then(|v| v.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// TS `invalidArgText` — the literal rendered for a non-string argument.
pub fn invalid_arg_text() -> String {
    "[invalid arg]".to_string()
}

pub fn render_tool_path(path: Option<&str>, cwd: &str, empty_fallback: &str) -> String {
    match path {
        Some(p) if !p.is_empty() => {
            if let Some(stripped) = p.strip_prefix(cwd) {
                if stripped.starts_with('/') {
                    format!(".{}", stripped)
                } else {
                    p.to_string()
                }
            } else {
                p.to_string()
            }
        }
        _ => empty_fallback.to_string(),
    }
}

pub fn shorten_path(path: &str, cwd: &str) -> String {
    if let Some(stripped) = path.strip_prefix(cwd) {
        if stripped.starts_with('/') {
            format!(".{}", stripped)
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    }
}

/// TS `normalizeDisplayText`: drop carriage returns so partially-streamed
/// CRLF content does not render stray `\r` glyphs. Tabs are left alone (the
/// tabs are expanded by [`replace_tabs`] separately).
pub fn normalize_display_text(text: &str) -> String {
    text.replace('\r', "")
}

/// TS `replaceTabs`: one tab becomes **three** spaces.
pub fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TS `normalizeDisplayText` strips CR; `replaceTabs` expands a tab to
    /// three spaces (not four).
    #[test]
    fn display_text_normalization_matches_ts() {
        assert_eq!(normalize_display_text("a\r\nb\r"), "a\nb");
        assert_eq!(replace_tabs("a\tb"), "a   b");
        assert_eq!(invalid_arg_text(), "[invalid arg]");
    }
}
