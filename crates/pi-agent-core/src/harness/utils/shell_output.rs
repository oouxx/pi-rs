#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::sync::Arc;

use crate::harness::types::{ExecutionEnv, ExecutionError, ShellCaptureResult};
use crate::harness::utils::truncate::DEFAULT_MAX_BYTES;

/// Shared per-chunk callback used by both stdout and stderr capture.
type ChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[allow(clippy::type_complexity)]
pub struct ShellCaptureOptions {
    pub max_bytes: Option<u64>,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub on_chunk: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

pub fn sanitize_binary_output(s: &str) -> String {
    s.chars()
        .filter(|c| {
            let code = *c as u32;
            if code == 0x09 || code == 0x0a || code == 0x0d {
                return true;
            }
            if code <= 0x1f {
                return false;
            }
            if (0xfff9..=0xfffb).contains(&code) {
                return false;
            }
            true
        })
        .collect()
}

/// Remove CSI (`ESC [ ... final`) and OSC (`ESC ] ... BEL` / `ESC \`)
/// sequences. Mirrors the TS `stripAnsi` used by `bash-executor.ts`.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if next == '\u{7}' || (previous == '\u{1b}' && next == '\\') {
                        break;
                    }
                    previous = next;
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Split a chunk at a trailing, incomplete ANSI escape sequence. The pending
/// suffix is prepended to the next chunk, so a sequence split across writes is
/// still stripped (TS `splitIncompleteAnsiSuffix`).
fn split_incomplete_ansi_suffix(input: &str) -> (String, String) {
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            index += 1;
            continue;
        }
        let start = index;
        let Some(kind) = bytes.get(index + 1).copied() else {
            return (input[..start].to_string(), input[start..].to_string());
        };
        match kind {
            b'[' => {
                let mut end = index + 2;
                while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                    end += 1;
                }
                if end >= bytes.len() {
                    return (input[..start].to_string(), input[start..].to_string());
                }
                index = end + 1;
            }
            b']' => {
                let mut end = index + 2;
                let mut terminated = false;
                while end < bytes.len() {
                    if bytes[end] == 0x07 {
                        terminated = true;
                        end += 1;
                        break;
                    }
                    if bytes[end] == 0x1b && bytes.get(end + 1) == Some(&b'\\') {
                        terminated = true;
                        end += 2;
                        break;
                    }
                    end += 1;
                }
                if !terminated {
                    return (input[..start].to_string(), input[start..].to_string());
                }
                index = end;
            }
            _ => index += 2,
        }
    }
    (input.to_string(), String::new())
}

struct CaptureState {
    /// Rolling tail kept for the model-facing output (bounded by `max_bytes`).
    output_chunks: Vec<String>,
    output_bytes: usize,
    /// Every sanitized chunk, never trimmed, used for the "full output" file.
    full_output_chunks: Vec<String>,
    /// Incomplete ANSI sequence carried across chunk boundaries.
    pending_ansi: String,
    total_bytes: usize,
    max_bytes: u64,
}

fn append_capture(state: &mut CaptureState, chunk: &str) -> String {
    state.total_bytes += chunk.len();
    let combined = format!("{}{}", state.pending_ansi, chunk);
    let (complete, pending) = split_incomplete_ansi_suffix(&combined);
    state.pending_ansi = pending;

    let text = sanitize_binary_output(&strip_ansi(&complete)).replace('\r', "");
    state.output_chunks.push(text.clone());
    state.full_output_chunks.push(text.clone());
    state.output_bytes += text.len();
    while state.output_bytes as u64 > state.max_bytes && state.output_chunks.len() > 1 {
        if let Some(removed) = state.output_chunks.first() {
            state.output_bytes -= removed.len();
            state.output_chunks.remove(0);
        }
    }
    text
}

pub async fn execute_shell_with_capture(
    env: &dyn ExecutionEnv,
    command: &str,
    options: Option<ShellCaptureOptions>,
) -> std::result::Result<ShellCaptureResult, ExecutionError> {
    let opts = options.unwrap_or(ShellCaptureOptions {
        max_bytes: None,
        abort_signal: None,
        on_chunk: None,
    });

    // The rolling buffer is 2x the temp-file threshold (match TS
    // `maxOutputBytes = DEFAULT_MAX_BYTES * 2`).
    let max_bytes = opts.max_bytes.unwrap_or(DEFAULT_MAX_BYTES * 2);
    let on_chunk: Option<ChunkCallback> = opts.on_chunk.map(Arc::from);
    let state = Arc::new(tokio::sync::Mutex::new(CaptureState {
        output_chunks: Vec::new(),
        output_bytes: 0,
        full_output_chunks: Vec::new(),
        pending_ansi: String::new(),
        total_bytes: 0,
        max_bytes,
    }));

    let state_stdout = state.clone();
    let state_stderr = state.clone();
    let on_chunk_stdout = on_chunk.clone();
    let on_chunk_stderr = on_chunk.clone();

    let exec_options = crate::harness::types::ShellExecOptions {
        cwd: None,
        env: None,
        abort_signal: opts.abort_signal.clone(),
        on_stdout: Some(Box::new(move |chunk: &str| {
            let text = {
                let mut s = state_stdout.blocking_lock();
                append_capture(&mut s, chunk)
            };
            if let Some(callback) = &on_chunk_stdout {
                callback(&text);
            }
        })),
        on_stderr: Some(Box::new(move |chunk: &str| {
            let text = {
                let mut s = state_stderr.blocking_lock();
                append_capture(&mut s, chunk)
            };
            if let Some(callback) = &on_chunk_stderr {
                callback(&text);
            }
        })),
    };

    let result = env.exec(command, exec_options).await;

    let s = state.lock().await;
    let tail_output = s.output_chunks.join("");

    let truncation_result = crate::harness::utils::truncate::truncate_tail(
        &tail_output,
        crate::harness::utils::truncate::TruncationOptions::default(),
    );

    // v0.87.1 calls `ensureTempFile()` once total output exceeds the 50KB
    // threshold *and* whenever the final truncation reports truncation (a
    // line-only truncation can happen below 50KB).
    let needs_full_output = s.total_bytes as u64 > DEFAULT_MAX_BYTES || truncation_result.truncated;
    let full_output_path = if needs_full_output {
        let full_content = s.full_output_chunks.join("");
        match env
            .create_temp_file(Some(crate::harness::types::TempFileOptions {
                prefix: Some("shell-output-".to_string()),
                suffix: Some(".txt".to_string()),
            }))
            .await
        {
            Ok(temp_path) => {
                let _ = env.write_file(&temp_path, &full_content, None).await;
                Some(temp_path)
            }
            Err(_) => None,
        }
    } else {
        None
    };

    match result {
        Ok(exec_result) => {
            let output = if truncation_result.truncated {
                truncation_result.content
            } else {
                tail_output
            };

            Ok(ShellCaptureResult {
                output,
                exit_code: Some(exec_result.exit_code),
                cancelled: false,
                truncated: truncation_result.truncated || full_output_path.is_some(),
                full_output_path,
            })
        }
        Err(e) => match &e {
            ExecutionError::Aborted(_) => Ok(ShellCaptureResult {
                output: tail_output,
                exit_code: None,
                cancelled: true,
                truncated: false,
                full_output_path,
            }),
            _ => Err(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_and_osc() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}body"), "body");
        assert_eq!(strip_ansi("\u{1b}]8;;https://x\u{1b}\\link"), "link");
    }

    #[test]
    fn split_incomplete_ansi_suffix_keeps_partial_sequence() {
        let (complete, pending) = split_incomplete_ansi_suffix("abc\u{1b}[3");
        assert_eq!(complete, "abc");
        assert_eq!(pending, "\u{1b}[3");

        let (complete, pending) = split_incomplete_ansi_suffix("abc\u{1b}[31m");
        assert_eq!(complete, "abc\u{1b}[31m");
        assert!(pending.is_empty());
    }

    #[test]
    fn append_capture_strips_ansi_split_across_chunks() {
        let mut state = CaptureState {
            output_chunks: Vec::new(),
            output_bytes: 0,
            full_output_chunks: Vec::new(),
            pending_ansi: String::new(),
            total_bytes: 0,
            max_bytes: 100,
        };
        append_capture(&mut state, "\u{1b}[3");
        append_capture(&mut state, "1mred\u{1b}[0m");
        assert_eq!(state.output_chunks.join(""), "red");
        assert_eq!(state.full_output_chunks.join(""), "red");
    }
}
