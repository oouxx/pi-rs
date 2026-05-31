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

pub fn invalid_arg_text() -> String {
    "<invalid argument>".to_string()
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

pub fn normalize_display_text(text: &str) -> String {
    text.replace('\t', "    ")
}

pub fn replace_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}