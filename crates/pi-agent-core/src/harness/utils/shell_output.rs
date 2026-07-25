#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unnecessary_struct_initialization,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::redundant_clone,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
    clippy::bool_to_int_with_if,
    clippy::needless_continue,
    clippy::items_after_statements,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_lit_as_bytes,
    clippy::trivially_copy_pass_by_ref,
    clippy::single_char_pattern,
    clippy::format_push_string,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::needless_raw_string_hashes,
    clippy::unnecessary_fold,
    clippy::needless_pass_by_ref_mut,
    clippy::map_identity,
    clippy::needless_return_with_question_mark,
    clippy::needless_lifetimes,
    clippy::similar_names,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::large_enum_variant,
    clippy::enum_glob_use,
    clippy::future_not_send,
    clippy::should_implement_trait,
    clippy::new_without_default,
    clippy::return_self_not_must_use,
    clippy::use_self,












    clippy::significant_drop_tightening,

    clippy::default_trait_access,

    clippy::iter_with_drain,

    clippy::if_not_else,

    clippy::explicit_iter_loop,

    clippy::assigning_clones,

    clippy::implicit_hasher,

    clippy::ignored_unit_patterns,

    clippy::missing_fields_in_debug,

    clippy::or_fun_call,

    clippy::too_long_first_doc_paragraph,

    clippy::manual_string_new,

    clippy::single_match_else,

    clippy::significant_drop_in_scrutinee,

    clippy::needless_collect,

    clippy::duplicated_attributes,

)]
use std::sync::Arc;

use crate::harness::types::{ExecutionEnv, ExecutionError, ShellCaptureResult};
use crate::harness::utils::truncate::DEFAULT_MAX_BYTES;

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

    // Save full output to temp file if output was truncated
    {
        let mut s = state.lock().await;
        let total_bytes = s.total_bytes as u64;
        if total_bytes > max_bytes {
            let full_content = s.output_chunks.join("");
            if let Ok(temp_path) = env
                .create_temp_file(Some(crate::harness::types::TempFileOptions {
                    prefix: Some("shell-output-".to_string()),
                    suffix: Some(".txt".to_string()),
                }))
                .await
            {
                let _ = env.write_file(&temp_path, &full_content, None).await;
                s.full_output_path = Some(temp_path);
            }
        }
    }

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
                truncated: truncation_result.truncated || full_output_path.is_some(),
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
