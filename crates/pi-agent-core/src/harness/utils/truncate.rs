pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: u64 = 500_000;
pub const GREP_MAX_LINE_LENGTH: usize = 500;

#[derive(Debug, Clone)]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub last_line_partial: bool,
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

fn utf8_byte_length(s: &str) -> usize {
    s.as_bytes().len()
}

pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let total_bytes = utf8_byte_length(content);
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.len() > 1 && lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes as u64 <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    if total_lines > 0 && utf8_byte_length(lines[0]) as u64 > max_bytes {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some(TruncatedBy::Bytes),
            total_lines,
            total_bytes,
            output_lines: 0,
            output_bytes: 0,
            last_line_partial: false,
            first_line_exceeds_limit: true,
            max_lines,
            max_bytes,
        };
    }

    let mut output_lines_arr: Vec<&str> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;

    for (i, line) in lines.iter().enumerate().take(max_lines) {
        let line_bytes = utf8_byte_length(line) + if i > 0 { 1 } else { 0 };
        if output_bytes_count as u64 + line_bytes as u64 > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output_lines_arr.push(line);
        output_bytes_count += line_bytes;
    }

    if output_lines_arr.len() >= max_lines && output_bytes_count as u64 <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let output_content = output_lines_arr.join("\n");
    let final_output_bytes = utf8_byte_length(&output_content);

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines: output_lines_arr.len(),
        output_bytes: final_output_bytes,
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

pub fn truncate_tail(content: &str, options: TruncationOptions) -> TruncationResult {
    let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let total_bytes = utf8_byte_length(content);
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.len() > 1 && lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes as u64 <= max_bytes {
        return TruncationResult {
            content: content.to_string(),
            truncated: false,
            truncated_by: None,
            total_lines,
            total_bytes,
            output_lines: total_lines,
            output_bytes: total_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
            max_lines,
            max_bytes,
        };
    }

    let mut output_lines_arr: Vec<String> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;
    let mut last_line_partial = false;

    for i in (0..lines.len()).rev().take(max_lines) {
        let line = lines[i];
        let line_bytes = utf8_byte_length(line) + if !output_lines_arr.is_empty() { 1 } else { 0 };
        if output_bytes_count as u64 + line_bytes as u64 > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            if output_lines_arr.is_empty() {
                let truncated_line = truncate_string_to_bytes_from_end(line, max_bytes as usize);
                output_bytes_count = utf8_byte_length(&truncated_line);
                output_lines_arr.insert(0, truncated_line);
                last_line_partial = true;
            }
            break;
        }
        output_lines_arr.insert(0, line.to_string());
        output_bytes_count += line_bytes;
    }

    if output_lines_arr.len() >= max_lines && output_bytes_count as u64 <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let output_content = output_lines_arr.join("\n");
    let final_output_bytes = utf8_byte_length(&output_content);

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines: output_lines_arr.len(),
        output_bytes: final_output_bytes,
        last_line_partial,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

fn truncate_string_to_bytes_from_end(s: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return String::new();
    }

    let mut output_bytes = 0usize;
    let mut start = s.len();

    for (i, ch) in s.char_indices().rev() {
        let char_bytes = ch.len_utf8();
        if output_bytes + char_bytes > max_bytes {
            break;
        }
        output_bytes += char_bytes;
        start = i;
    }

    if start < s.len() {
        s[start..].to_string()
    } else {
        String::new()
    }
}

pub fn truncate_line(line: &str, max_chars: Option<usize>) -> (String, bool) {
    let max = max_chars.unwrap_or(GREP_MAX_LINE_LENGTH);
    if line.len() <= max {
        return (line.to_string(), false);
    }
    (format!("{}... [truncated]", &line[..max]), true)
}

pub struct TruncationOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<u64>,
}

impl Default for TruncationOptions {
    fn default() -> Self {
        Self {
            max_lines: None,
            max_bytes: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_head_no_truncation() {
        let content = "Hello\nWorld";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1000),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.content, content);
        assert_eq!(result.total_lines, 2);
    }

    #[test]
    fn test_truncate_head_by_lines() {
        let content = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(3),
                max_bytes: Some(10000),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.output_lines, 3);
        assert!(result.content.contains("line1"));
        assert!(result.content.contains("line3"));
        assert!(!result.content.contains("line4"));
    }

    #[test]
    fn test_truncate_head_by_bytes() {
        let content = "éé\nabc";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(4),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        assert_eq!(result.content, "éé");
        assert_eq!(result.output_bytes, 4);
        assert!(!result.first_line_exceeds_limit);
    }

    #[test]
    fn test_truncate_head_first_line_exceeds() {
        let result = truncate_head(
            "éé\nabc",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(3),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.content, "");
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        assert!(result.first_line_exceeds_limit);
    }

    #[test]
    fn test_truncate_head_empty_content() {
        let result = truncate_head(
            "",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1000),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.total_lines, 1);
    }

    #[test]
    fn test_truncate_head_single_line() {
        let result = truncate_head(
            "Hello",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1000),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.total_lines, 1);
    }

    #[test]
    fn test_truncate_tail_no_truncation() {
        let content = "Hello\nWorld";
        let result = truncate_tail(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1000),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.content, content);
    }

    #[test]
    fn test_truncate_tail_by_lines() {
        let content = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_tail(
            content,
            TruncationOptions {
                max_lines: Some(3),
                max_bytes: Some(10000),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.output_lines, 3);
        assert!(result.content.contains("line3"));
        assert!(result.content.contains("line5"));
        assert!(!result.content.contains("line1"));
    }

    #[test]
    fn test_truncate_tail_by_bytes() {
        let result = truncate_tail(
            "aé🙂b",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(5),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        assert!(result.last_line_partial);
        assert_eq!(result.output_bytes, 5);
        assert_eq!(result.content, "🙂b");
    }

    #[test]
    fn test_truncate_tail_oversized_single_line() {
        let input = format!("{}\n", "X".repeat(300_000));
        let result = truncate_tail(
            &input,
            TruncationOptions {
                max_lines: Some(100),
                max_bytes: Some(1024),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.output_bytes, 1024);
        assert_eq!(result.output_lines, 1);
        assert!(result.last_line_partial);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    }

    #[test]
    fn test_truncate_tail_drops_oversized_char() {
        let result = truncate_tail(
            "abc🙂",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(3),
            },
        );
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
        assert!(result.last_line_partial);
        assert_eq!(result.output_bytes, 0);
    }

    #[test]
    fn test_truncate_tail_empty_content() {
        let result = truncate_tail(
            "",
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1000),
            },
        );
        assert!(!result.truncated);
    }

    #[test]
    fn test_truncate_line_no_truncation() {
        let (result, truncated) = truncate_line("Hello world", None);
        assert_eq!(result, "Hello world");
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_line_with_limit() {
        let (result, truncated) = truncate_line("Hello world", Some(5));
        assert_eq!(result, "Hello... [truncated]");
        assert!(truncated);
    }

    #[test]
    fn test_truncate_line_at_limit() {
        let (result, truncated) = truncate_line("Hello", Some(5));
        assert_eq!(result, "Hello");
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_line_default_limit() {
        let long_line = "a".repeat(600);
        let (result, truncated) = truncate_line(&long_line, None);
        assert!(truncated);
        assert!(result.contains("[truncated]"));
    }

    #[test]
    fn test_utf8_byte_length() {
        assert_eq!(utf8_byte_length("a"), 1);
        assert_eq!(utf8_byte_length("é"), 2);
        assert_eq!(utf8_byte_length("🙂"), 4);
        assert_eq!(utf8_byte_length("aé🙂\nb"), 9);
    }

    #[test]
    fn test_truncate_head_counts_bytes_correctly() {
        let content = "aé🙂\nb";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(100),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.total_bytes, 9);
        assert_eq!(result.output_bytes, 9);
    }

    #[test]
    fn test_truncate_head_trailing_newline() {
        let content = "line1\nline2\n";
        let result = truncate_head(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1000),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.total_lines, 2);
    }

    #[test]
    fn test_truncate_tail_trailing_newline() {
        let content = "line1\nline2\n";
        let result = truncate_tail(
            content,
            TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1000),
            },
        );
        assert!(!result.truncated);
        assert_eq!(result.total_lines, 2);
    }

    #[test]
    fn test_default_options() {
        let opts = TruncationOptions::default();
        assert!(opts.max_lines.is_none());
        assert!(opts.max_bytes.is_none());
    }
}
