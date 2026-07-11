use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024;
pub const GREP_MAX_LINE_LENGTH: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<String>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub last_line_partial: bool,
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct TruncationOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
}

impl Default for TruncationOptions {
    fn default() -> Self {
        Self {
            max_lines: None,
            max_bytes: None,
        }
    }
}

fn split_lines_for_counting(content: &str) -> Vec<&str> {
    let raw: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') && raw.len() > 1 {
        raw[..raw.len() - 1].to_vec()
    } else {
        raw
    }
}

fn byte_len(s: &str) -> usize {
    s.len()
}

pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub fn truncate_head(content: &str, options: Option<TruncationOptions>) -> TruncationResult {
    let opts = options.unwrap_or_default();
    let max_lines = opts.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = opts.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let total_bytes = byte_len(content);
    let lines = split_lines_for_counting(content);
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
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

    let first_line_bytes = byte_len(lines[0]);
    if first_line_bytes > max_bytes {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some("bytes".to_string()),
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
    let mut output_bytes_count: usize = 0;
    let mut truncated_by = "lines";

    for (i, line) in lines.iter().enumerate() {
        if i >= max_lines {
            break;
        }
        let line_bytes = byte_len(line) + if i > 0 { 1 } else { 0 };
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = "bytes";
            break;
        }
        output_lines_arr.push(line);
        output_bytes_count += line_bytes;
    }

    if output_lines_arr.len() >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = "lines";
    }

    let output_content = output_lines_arr.join("\n");
    let final_output_bytes = byte_len(&output_content);

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by.to_string()),
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

pub fn truncate_tail(content: &str, options: Option<TruncationOptions>) -> TruncationResult {
    let opts = options.unwrap_or_default();
    let max_lines = opts.max_lines.unwrap_or(DEFAULT_MAX_LINES);
    let max_bytes = opts.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let total_bytes = byte_len(content);

    // Split on newlines. A trailing newline produces an empty final segment
    // that should NOT be counted as a line.
    let raw_lines: Vec<&str> = content.split('\n').collect();
    let has_trailing_newline = content.ends_with('\n');
    let total_lines = if has_trailing_newline {
        raw_lines.len() - 1
    } else {
        raw_lines.len()
    };
    let lines: Vec<String> = raw_lines
        .iter()
        .take(if has_trailing_newline { raw_lines.len() - 1 } else { raw_lines.len() })
        .map(|s| s.to_string())
        .collect();

    if total_lines <= max_lines && total_bytes <= max_bytes {
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
    let mut output_bytes_count: usize = 0;
    let mut truncated_by = "lines";
    let mut last_line_partial = false;

    for line in lines.iter().rev() {
        if output_lines_arr.len() >= max_lines {
            break;
        }
        let line_bytes = byte_len(line) + if !output_lines_arr.is_empty() { 1 } else { 0 };
        if output_bytes_count + line_bytes > max_bytes {
            truncated_by = "bytes";
            if output_lines_arr.is_empty() {
                let truncated_line = truncate_string_to_bytes_from_end(line, max_bytes);
                output_bytes_count = byte_len(&truncated_line);
                last_line_partial = true;
                output_lines_arr.insert(0, truncated_line);
            }
            break;
        }
        output_lines_arr.insert(0, line.clone());
        output_bytes_count += line_bytes;
    }

    if output_lines_arr.len() >= max_lines && output_bytes_count <= max_bytes {
        truncated_by = "lines";
    }

    let output_content = output_lines_arr.join("\n");
    let final_output_bytes = byte_len(&output_content);

    TruncationResult {
        content: output_content,
        truncated: true,
        truncated_by: Some(truncated_by.to_string()),
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
    let bytes = s.as_bytes();
    if bytes.len() <= max_bytes {
        return s.to_string();
    }
    let mut start = bytes.len() - max_bytes;
    while start < bytes.len() && (bytes[start] & 0xc0) == 0x80 {
        start += 1;
    }
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

pub fn truncate_line(line: &str, max_chars: Option<usize>) -> (String, bool) {
    let max = max_chars.unwrap_or(GREP_MAX_LINE_LENGTH);
    if line.len() <= max {
        return (line.to_string(), false);
    }
    (format!("{}... [truncated]", &line[..max]), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500B");
        assert_eq!(format_size(1024), "1.0KB");
        assert_eq!(format_size(1024 * 1024), "1.0MB");
    }

    #[test]
    fn test_truncate_head_no_truncation() {
        let content = "line1\nline2\nline3";
        let result = truncate_head(
            content,
            Some(TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1024),
            }),
        );
        assert!(!result.truncated);
        assert_eq!(result.total_lines, 3);
    }

    #[test]
    fn test_truncate_head_by_lines() {
        let content = "line1\nline2\nline3\nline4\nline5";
        let result = truncate_head(
            content,
            Some(TruncationOptions {
                max_lines: Some(3),
                max_bytes: Some(1024),
            }),
        );
        assert!(result.truncated);
        assert_eq!(result.truncated_by, Some("lines".to_string()));
        assert_eq!(result.output_lines, 3);
    }

    #[test]
    fn test_truncate_head_first_line_exceeds() {
        let long_line = "a".repeat(300);
        let result = truncate_head(
            &long_line,
            Some(TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(100),
            }),
        );
        assert!(result.first_line_exceeds_limit);
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_truncate_tail_no_truncation() {
        let content = "line1\nline2\nline3";
        let result = truncate_tail(
            content,
            Some(TruncationOptions {
                max_lines: Some(10),
                max_bytes: Some(1024),
            }),
        );
        assert!(!result.truncated);
    }

    #[test]
    fn test_truncate_line() {
        let (text, was_truncated) = truncate_line("short", Some(10));
        assert_eq!(text, "short");
        assert!(!was_truncated);

        let long_line = "a".repeat(600);
        let (text, was_truncated) = truncate_line(&long_line, Some(500));
        assert!(was_truncated);
        assert!(text.ends_with("[truncated]"));
    }
}
