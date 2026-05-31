/// Strip ANSI escape sequences and compute the visible display width of a string.
pub fn visible_width(s: &str) -> usize {
    // Strip ANSI escape sequences
    let stripped = strip_ansi(s);
    unicode_width::UnicodeWidthStr::width(stripped.as_str())
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        if in_escape {
            // Check for CSI sequence: ESC [
            // Most ANSI escapes end with a letter (A-Z, a-z)
            if ch.is_ascii_alphabetic() || ch == '~' {
                in_escape = false;
            }
            continue;
        }
        // Handle OSC (Operating System Command) sequences: ESC ]
        // These end with BEL (\x07) or ST (\x1b\\)
        result.push(ch);
    }

    result
}

/// Truncate a string to fit within the given display width,
/// adding "..." if truncation occurred.
pub fn truncate_to_width(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if visible_width(s) <= width {
        return s.to_string();
    }
    let mut result = String::new();
    let mut current_width = 0;
    let ellipsis = "...";
    let ellipsis_width = 3;

    for ch in s.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width + ellipsis_width > width {
            break;
        }
        current_width += ch_width;
        result.push(ch);
    }
    result.push_str(ellipsis);
    result
}

/// Wrap text to fit within a given width, preserving ANSI escape sequences.
/// Returns a vector of lines, each no wider than `width` display columns.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut line_width = 0;

    let mut in_escape = false;
    let mut escape_buf = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        // Handle ANSI escape sequences — pass them through unchanged
        if ch == '\x1b' {
            escape_buf.push(ch);
            in_escape = true;
            continue;
        }
        if in_escape {
            escape_buf.push(ch);
            current_line.push(ch);
            if ch.is_ascii_alphabetic() || ch == '~' {
                in_escape = false;
                escape_buf.clear();
            }
            continue;
        }

        // Handle newlines
        if ch == '\n' {
            lines.push(current_line);
            current_line = String::new();
            line_width = 0;
            continue;
        }

        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

        // Word wrapping: if the word doesn't fit, start a new line
        if line_width + ch_width > width {
            // Try to break at word boundary
            if let Some(space_idx) = current_line.rfind(' ') {
                let after_space = current_line[space_idx + 1..].to_string();
                if !after_space.trim().is_empty() {
                    let kept = current_line[..space_idx].to_string();
                    if !kept.is_empty() {
                        lines.push(kept);
                    }
                    current_line = after_space;
                    line_width = visible_width(&current_line);
                    current_line.push(ch);
                    line_width += ch_width;
                    continue;
                }
            }
            // Hard break at width
            if !current_line.is_empty() {
                lines.push(current_line);
            }
            current_line = String::new();
            line_width = 0;
        }

        current_line.push(ch);
        line_width += ch_width;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Apply a background color function to each character of a line.
pub fn apply_background_to_line(line: &str, bg_fn: &dyn Fn(&str) -> String) -> String {
    bg_fn(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visible_width_plain() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn test_visible_width_ansi() {
        assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
    }

    #[test]
    fn test_visible_width_cjk() {
        // CJK characters are 2 columns wide
        assert_eq!(visible_width("你好"), 4);
    }

    #[test]
    fn test_truncate_to_width_no_truncation() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_to_width_truncated() {
        let result = truncate_to_width("hello world", 8);
        assert!(result.ends_with("..."));
        assert!(visible_width(&result) <= 8);
    }

    #[test]
    fn test_truncate_to_width_zero() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn test_wrap_text_no_wrap() {
        let result = wrap_text_with_ansi("hello", 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "hello");
    }

    #[test]
    fn test_wrap_text_with_newlines() {
        let result = wrap_text_with_ansi("line1\nline2", 10);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "line1");
        assert_eq!(result[1], "line2");
    }

    #[test]
    fn test_wrap_text_width() {
        let result = wrap_text_with_ansi("hello world", 5);
        assert_eq!(result.len(), 2);
    }
}
