use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use super::truncate::{self, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};

#[derive(Debug, Clone)]
pub struct OutputAccumulatorOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
    pub temp_file_prefix: Option<String>,
}

impl Default for OutputAccumulatorOptions {
    fn default() -> Self {
        Self {
            max_lines: None,
            max_bytes: None,
            temp_file_prefix: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    pub content: String,
    pub truncation: TruncationResult,
    pub full_output_path: Option<String>,
}

fn default_temp_file_path(prefix: &str) -> PathBuf {
    let id = uuid::Uuid::new_v4();
    let mut hex = [0u8; 16];
    hex.copy_from_slice(id.as_bytes());
    let hex_str: String = hex.iter().map(|b| format!("{:02x}", b)).collect();
    let tmp = std::env::temp_dir();
    tmp.join(format!("{}-{}.log", prefix, &hex_str[..16]))
}

/// Incrementally tracks streaming output with bounded memory.
///
/// Appends text chunks, keeps only a decoded tail for display snapshots,
/// and writes to a temp file when the full output needs to be preserved.
pub struct OutputAccumulator {
    max_lines: usize,
    max_bytes: usize,
    max_rolling_bytes: usize,
    temp_file_prefix: String,

    tail_text: String,
    tail_bytes: usize,
    tail_starts_at_line_boundary: bool,
    total_raw_bytes: usize,
    total_decoded_bytes: usize,
    completed_lines: usize,
    total_lines: usize,
    current_line_bytes: usize,
    has_open_line: bool,
    finished: bool,

    temp_file_path: Option<PathBuf>,
    /// Raw bytes held in memory before temp file is opened
    pending_chunks: Vec<u8>,
}

impl OutputAccumulator {
    pub fn new(options: OutputAccumulatorOptions) -> Self {
        let max_lines = options.max_lines.unwrap_or(DEFAULT_MAX_LINES);
        let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
        let max_rolling_bytes = max_bytes.saturating_mul(2).max(1);
        let temp_file_prefix = options
            .temp_file_prefix
            .unwrap_or_else(|| "pi-output".into());

        Self {
            max_lines,
            max_bytes,
            max_rolling_bytes,
            temp_file_prefix,
            tail_text: String::new(),
            tail_bytes: 0,
            tail_starts_at_line_boundary: true,
            total_raw_bytes: 0,
            total_decoded_bytes: 0,
            completed_lines: 0,
            total_lines: 0,
            current_line_bytes: 0,
            has_open_line: false,
            finished: false,
            temp_file_path: None,
            pending_chunks: Vec::new(),
        }
    }

    pub fn append(&mut self, data: &[u8]) {
        if self.finished {
            panic!("Cannot append to a finished output accumulator");
        }

        self.total_raw_bytes += data.len();
        self.append_decoded_text(&String::from_utf8_lossy(data));

        if self.temp_file_path.is_some() || self.should_use_temp_file() {
            self.ensure_temp_file();
            if let Some(ref path) = self.temp_file_path {
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .expect("failed to open temp file");
                file.write_all(data).expect("failed to write to temp file");
            }
        } else if !data.is_empty() {
            self.pending_chunks.extend_from_slice(data);
        }
    }

    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        if self.should_use_temp_file() {
            self.ensure_temp_file();
        }
    }

    pub fn snapshot(&self, persist_if_truncated: bool) -> OutputSnapshot {
        let snapshot_text = self.get_snapshot_text();
        let tail_truncation = truncate::truncate_tail(
            &snapshot_text,
            Some(truncate::TruncationOptions {
                max_lines: Some(self.max_lines),
                max_bytes: Some(self.max_bytes),
            }),
        );

        let truncated =
            self.total_lines > self.max_lines || self.total_decoded_bytes > self.max_bytes;
        let truncated_by = if truncated {
            Some(tail_truncation.truncated_by.clone().unwrap_or_else(|| {
                if self.total_decoded_bytes > self.max_bytes {
                    "bytes".to_string()
                } else {
                    "lines".to_string()
                }
            }))
        } else {
            None
        };

        // Build a final TruncationResult with the original totals
        let mut truncation = TruncationResult {
            content: tail_truncation.content.clone(),
            truncated,
            truncated_by,
            total_lines: self.total_lines,
            total_bytes: self.total_decoded_bytes,
            output_lines: tail_truncation.output_lines,
            output_bytes: tail_truncation.output_bytes,
            last_line_partial: tail_truncation.last_line_partial,
            first_line_exceeds_limit: tail_truncation.first_line_exceeds_limit,
            max_lines: self.max_lines,
            max_bytes: self.max_bytes,
        };
        truncation.content = tail_truncation.content;
        truncation.output_lines = tail_truncation.output_lines;
        truncation.output_bytes = tail_truncation.output_bytes;

        // If the output was truncated and the caller wants to persist, flush to temp file
        let mut snapshot = OutputSnapshot {
            content: truncation.content.clone(),
            truncation,
            full_output_path: self
                .temp_file_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
        };

        if persist_if_truncated && self.should_use_temp_file() && self.temp_file_path.is_none() {
            // Force temp file creation
            let mut acc = Self::new(OutputAccumulatorOptions {
                max_lines: Some(self.max_lines),
                max_bytes: Some(self.max_bytes),
                temp_file_prefix: Some(self.temp_file_prefix.clone()),
            });
            acc.pending_chunks = self.pending_chunks.clone();
            acc.total_raw_bytes = self.total_raw_bytes;
            acc.ensure_temp_file();
            snapshot.full_output_path = acc
                .temp_file_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
        }

        snapshot
    }

    pub fn get_last_line_bytes(&self) -> usize {
        self.current_line_bytes
    }

    fn append_decoded_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        let bytes = text.len();
        self.total_decoded_bytes += bytes;

        self.tail_text.push_str(text);
        self.tail_bytes += bytes;
        if self.tail_bytes > self.max_rolling_bytes.saturating_mul(2) {
            self.trim_tail();
        }

        let mut newlines = 0;
        let mut last_newline = None;
        let text_bytes = text.as_bytes();
        for (i, &b) in text_bytes.iter().enumerate() {
            if b == b'\n' {
                newlines += 1;
                last_newline = Some(i);
            }
        }

        if newlines == 0 {
            self.current_line_bytes += bytes;
            self.has_open_line = true;
        } else {
            self.completed_lines += newlines;
            if let Some(nl_pos) = last_newline {
                let tail = &text[nl_pos + 1..];
                self.current_line_bytes = tail.len();
                self.has_open_line = !tail.is_empty();
            }
        }
        self.total_lines = self.completed_lines + if self.has_open_line { 1 } else { 0 };
    }

    fn trim_tail(&mut self) {
        if self.tail_bytes <= self.max_rolling_bytes {
            return;
        }

        let bytes = self.tail_text.as_bytes();
        let max_rolling = self.max_rolling_bytes;

        let start = if bytes.len() > max_rolling {
            let start_pos = bytes.len() - max_rolling;
            // Ensure we don't split a multi-byte character
            let mut pos = start_pos;
            while pos < bytes.len() && (bytes[pos] & 0xc0) == 0x80 {
                pos += 1;
            }
            pos
        } else {
            0
        };

        self.tail_starts_at_line_boundary = if start == 0 {
            self.tail_starts_at_line_boundary
        } else {
            bytes[start - 1] == b'\n'
        };

        self.tail_text = String::from_utf8_lossy(&bytes[start..]).to_string();
        self.tail_bytes = self.tail_text.len();
    }

    fn get_snapshot_text(&self) -> String {
        if self.tail_starts_at_line_boundary {
            return self.tail_text.clone();
        }

        match self.tail_text.find('\n') {
            Some(pos) => self.tail_text[pos + 1..].to_string(),
            None => self.tail_text.clone(),
        }
    }

    fn should_use_temp_file(&self) -> bool {
        self.total_raw_bytes > self.max_bytes
            || self.total_decoded_bytes > self.max_bytes
            || self.total_lines > self.max_lines
    }

    fn ensure_temp_file(&mut self) {
        if self.temp_file_path.is_some() {
            return;
        }
        let path = default_temp_file_path(&self.temp_file_prefix);
        let mut file = File::create(&path).expect("failed to create temp file");
        if !self.pending_chunks.is_empty() {
            file.write_all(&self.pending_chunks)
                .expect("failed to write pending chunks to temp file");
        }
        self.pending_chunks.clear();
        self.temp_file_path = Some(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_accumulator_basic() {
        let mut acc = OutputAccumulator::new(OutputAccumulatorOptions {
            max_lines: Some(100),
            max_bytes: Some(1024),
            temp_file_prefix: Some("test".into()),
        });

        acc.append(b"hello\nworld\n");
        acc.finish();

        let snap = acc.snapshot(false);
        assert!(snap.content.contains("hello"));
        assert!(snap.content.contains("world"));
        assert!(!snap.truncation.truncated);
    }

    #[test]
    fn test_output_accumulator_truncation_by_lines() {
        let mut acc = OutputAccumulator::new(OutputAccumulatorOptions {
            max_lines: Some(2),
            max_bytes: Some(1024),
            temp_file_prefix: None,
        });

        acc.append(b"line1\nline2\nline3\nline4\n");
        acc.finish();

        let snap = acc.snapshot(false);
        assert!(snap.truncation.truncated);
        assert_eq!(snap.truncation.truncated_by.as_deref(), Some("lines"));
    }

    #[test]
    fn test_output_accumulator_temp_file_on_truncation() {
        let mut acc = OutputAccumulator::new(OutputAccumulatorOptions {
            max_lines: Some(2),
            max_bytes: Some(10),
            temp_file_prefix: Some("test".into()),
        });

        acc.append(b"hello world this is a long line that should trigger temp file\n");
        acc.finish();

        let snap = acc.snapshot(false);
        assert!(snap.truncation.truncated);
        // Should have a temp file path since content exceeds limits
        assert!(snap.full_output_path.is_some());
    }

    #[test]
    fn test_output_accumulator_empty() {
        let mut acc = OutputAccumulator::new(OutputAccumulatorOptions::default());
        acc.finish();
        let snap = acc.snapshot(false);
        assert!(snap.content.is_empty());
        assert!(!snap.truncation.truncated);
    }

    #[test]
    fn test_output_accumulator_no_truncation() {
        let mut acc = OutputAccumulator::new(OutputAccumulatorOptions {
            max_lines: Some(1000),
            max_bytes: Some(100000),
            temp_file_prefix: None,
        });

        let input = b"short line\n";
        acc.append(input);
        acc.finish();

        let snap = acc.snapshot(false);
        assert_eq!(snap.content, "short line\n");
        assert!(!snap.truncation.truncated);
    }

    #[test]
    fn test_get_last_line_bytes() {
        let mut acc = OutputAccumulator::new(OutputAccumulatorOptions::default());
        acc.append(b"hello\nworld");
        assert_eq!(acc.get_last_line_bytes(), 5); // "world" is 5 bytes
        acc.append(b"!\n");
        assert_eq!(acc.get_last_line_bytes(), 0); // after newline, current is empty
    }
}
