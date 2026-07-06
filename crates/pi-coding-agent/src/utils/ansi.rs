//! ANSI escape code handling — strip and detect ANSI sequences.
//!
//! Mirrors packages/coding-agent/src/utils/ansi.ts

const ESC: u8 = 27;  // \x1b
const CSI: u8 = 155; // \x9b
const BEL: u8 = 7;   // \x07

/// Check if a string contains ANSI escape sequences.
/// Looks for ESC[ (CSI) sequences.
pub fn has_ansi(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == ESC && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            return true;
        }
        i += 1;
    }
    false
}

/// Strip ANSI escape sequences from a string.
/// Handles CSI sequences: ESC[...<final>
pub fn strip_ansi(input: &str) -> String {
    let mut result = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == ESC && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            while i < bytes.len() {
                let b = bytes[i];
                i += 1;
                if b.is_ascii_alphabetic() || b == b'~' || b == BEL {
                    break;
                }
            }
        } else {
            result.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(result).unwrap_or_else(|_| input.to_string())
}

/// Get the visible length of a string (without ANSI codes).
pub fn ansi_visible_length(input: &str) -> usize {
    strip_ansi(input).chars().count()
}

/// Truncate a string to a given visible width, preserving ANSI codes.
pub fn truncate_ansi(input: &str, max_width: usize) -> String {
    let stripped = strip_ansi(input);
    if stripped.chars().count() <= max_width {
        return input.to_string();
    }

    let mut visible = 0usize;
    let mut result = String::new();
    let mut in_escape = false;

    for c in input.chars() {
        if in_escape {
            result.push(c);
            if c == 'm' || c as u8 == BEL {
                in_escape = false;
            }
            continue;
        }
        if (c as u8) == ESC || (c as u8) == CSI {
            in_escape = true;
            result.push(c);
            continue;
        }
        if visible >= max_width {
            break;
        }
        result.push(c);
        visible += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_ansi() {
        assert!(has_ansi("\u{1b}[31mred\u{1b}[0m"));
        assert!(!has_ansi("plain text"));
    }

    #[test]
    fn test_strip_ansi() {
        let result = strip_ansi("\u{1b}[31mhello\u{1b}[0m world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(strip_ansi("hello"), "hello");
    }

    #[test]
    fn test_ansi_visible_length() {
        assert_eq!(ansi_visible_length("\u{1b}[31mhello\u{1b}[0m"), 5);
        assert_eq!(ansi_visible_length("hello"), 5);
    }

    #[test]
    fn test_truncate_ansi() {
        let input = "\u{1b}[31mhello\u{1b}[0m world";
        let result = truncate_ansi(input, 5);
        assert!(result.contains("hello"));
        assert!(!result.contains("world"));
    }

    #[test]
    fn test_truncate_ansi_shorter_than_max() {
        let input = "hi";
        assert_eq!(truncate_ansi(input, 10), "hi");
    }
}
