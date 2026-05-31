use std::sync::Arc;

use crate::harness::types::{ExecutionEnv, ExecutionError, ShellCaptureResult};
use crate::harness::utils::truncate::DEFAULT_MAX_BYTES;

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
            if code >= 0xfff9 && code <= 0xfffb {
                return false;
            }
            true
        })
        .collect()
}

struct CaptureState {
    output_chunks: Vec<String>,
    output_bytes: usize,
    total_bytes: usize,
    full_output_path: Option<String>,
    max_bytes: u64,
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

    let max_bytes = opts.max_bytes.unwrap_or(DEFAULT_MAX_BYTES * 2);
    let state = Arc::new(tokio::sync::Mutex::new(CaptureState {
        output_chunks: Vec::new(),
        output_bytes: 0,
        total_bytes: 0,
        full_output_path: None,
        max_bytes,
    }));

    let state_stdout = state.clone();
    let state_stderr = state.clone();

    let exec_options = crate::harness::types::ExecutionEnvExecOptions {
        cwd: None,
        env: None,
        abort_signal: opts.abort_signal.clone(),
        on_stdout: Some(Box::new(move |chunk: &str| {
            let text = sanitize_binary_output(chunk).replace('\r', "");
            let mut s = state_stdout.blocking_lock();
            s.total_bytes += chunk.len();
            s.output_chunks.push(text.clone());
            s.output_bytes += text.len();
            while s.output_bytes as u64 > s.max_bytes && s.output_chunks.len() > 1 {
                if let Some(removed) = s.output_chunks.first() {
                    s.output_bytes -= removed.len();
                    s.output_chunks.remove(0);
                }
            }
        })),
        on_stderr: Some(Box::new(move |chunk: &str| {
            let text = sanitize_binary_output(chunk).replace('\r', "");
            let mut s = state_stderr.blocking_lock();
            s.total_bytes += chunk.len();
            s.output_chunks.push(text.clone());
            s.output_bytes += text.len();
            while s.output_bytes as u64 > s.max_bytes && s.output_chunks.len() > 1 {
                if let Some(removed) = s.output_chunks.first() {
                    s.output_bytes -= removed.len();
                    s.output_chunks.remove(0);
                }
            }
        })),
    };

    let result = env.exec(command, exec_options).await;

    let s = state.lock().await;
    let full_output_path = s.full_output_path.clone();

    match result {
        Ok(exec_result) => {
            let tail_output = s.output_chunks.join("");
            let truncation_result = crate::harness::utils::truncate::truncate_tail(
                &tail_output,
                crate::harness::utils::truncate::TruncationOptions::default(),
            );

            let output = if truncation_result.truncated {
                truncation_result.content
            } else {
                tail_output
            };

            Ok(ShellCaptureResult {
                output,
                exit_code: Some(exec_result.exit_code),
                cancelled: false,
                truncated: truncation_result.truncated,
                full_output_path,
            })
        }
        Err(e) => match &e {
            ExecutionError::Aborted(_) => {
                let tail_output = s.output_chunks.join("");
                Ok(ShellCaptureResult {
                    output: tail_output,
                    exit_code: None,
                    cancelled: true,
                    truncated: false,
                    full_output_path,
                })
            }
            _ => Err(e),
        },
    }
}