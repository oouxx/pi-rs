/// Compute the visible display width of a string.
pub fn visible_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
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

/// Wrap text in an OSC 8 hyperlink sequence.
/// Creates a clickable link in terminals that support OSC 8 hyperlinks.
pub fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", url, text)
}

/// Check if a character is whitespace (space, tab, newline).
pub fn is_whitespace_char(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\n'
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
    fn test_visible_width_cjk() {
        assert_eq!(visible_width("\u{4f60}\u{597d}"), 4);
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
    fn test_visible_width_regional_indicators() {
        let partial = "\u{1F1E8}";
        let w = visible_width(partial);
        assert!(w >= 1, "Regional indicator should have non-zero width");
    }

    #[test]
    fn test_visible_width_full_flags() {
        let flags = ["\u{1F1EF}\u{1F1F5}", "\u{1F1FA}\u{1F1F8}"];
        for flag in &flags {
            assert_eq!(visible_width(flag), 2, "Flag {} should be width 2", flag);
        }
    }

    #[test]
    fn test_visible_width_emoji() {
        let emojis = ["👍", "✅", "⚡"];
        for emoji in &emojis {
            assert_eq!(visible_width(emoji), 2, "Emoji {} should be width 2", emoji);
        }
    }

    #[test]
    fn test_truncate_to_width_fits_exact() {
        assert_eq!(truncate_to_width("hello", 5), "hello");
        assert_eq!(truncate_to_width("hi", 10), "hi");
    }

    #[test]
    fn test_hyperlink_format() {
        let result = hyperlink("click me", "https://example.com");
        assert_eq!(result, "\x1b]8;;https://example.com\x1b\\click me\x1b]8;;\x1b\\");
    }

    #[test]
    fn test_truncate_to_width_malformed() {
        let result = truncate_to_width("hello", 3);
        assert!(result.ends_with("..."));
    }
}
