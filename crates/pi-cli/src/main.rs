//! pi-cli — CLI binary entry point for the pi coding agent.
//!
//! Mirrors packages/coding-agent/src/cli.ts

#![allow(
    clippy::redundant_closure,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::redundant_closure_for_method_calls,
    clippy::missing_const_for_fn,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::similar_names,
    clippy::large_futures,
    clippy::items_after_statements,
    clippy::default_trait_access,
    clippy::significant_drop_tightening,
    clippy::used_underscore_binding,
    clippy::used_underscore_items,
    clippy::disallowed_methods,
    clippy::unnecessary_debug_formatting,
    clippy::unused_async,
    clippy::inefficient_to_string,
    clippy::needless_pass_by_ref_mut,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::single_char_pattern,
    clippy::float_cmp,
    clippy::struct_excessive_bools,
    clippy::option_as_ref_cloned,
    clippy::option_option,
    clippy::format_collect,
    clippy::branches_sharing_code,
    clippy::comparison_chain,
    clippy::manual_assert,
    clippy::no_effect_underscore_binding,
    clippy::implicit_clone,
    clippy::unchecked_time_subtraction,
    clippy::useless_let_if_seq,
    clippy::incompatible_msrv,
    clippy::manual_let_else,
    clippy::match_same_arms,
    clippy::needless_continue,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::trivially_copy_pass_by_ref,
    clippy::use_self,
    clippy::or_fun_call,
    clippy::manual_string_new,
    clippy::single_match_else,
    clippy::needless_collect,
    clippy::duplicated_attributes,
    clippy::unreadable_literal,
    clippy::cast_abs_to_unsigned,
    clippy::cast_possible_wrap,
    clippy::fallible_impl_from,
    clippy::return_self_not_must_use,
    clippy::should_implement_trait,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::vec_init_then_push,
    clippy::wildcard_imports,
    clippy::enum_glob_use,
    clippy::cognitive_complexity,
    clippy::future_not_send,
    clippy::equatable_if_let,
    clippy::bool_to_int_with_if,
    clippy::cast_lossless,
    clippy::manual_pattern_char_comparison,
    clippy::derivable_impls,
    clippy::collapsible_match,
    clippy::collapsible_if,
    clippy::await_holding_lock,
    clippy::format_push_string,
    clippy::redundant_pattern_matching,
    clippy::option_map_or_none,
    clippy::option_map_unit_fn,
    clippy::result_map_or_into_option,
    clippy::unnecessary_wraps,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::exit,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::missing_fields_in_debug,
    clippy::too_long_first_doc_paragraph,
    clippy::significant_drop_in_scrutinee,
    clippy::iter_with_drain,
    clippy::if_not_else,
    clippy::explicit_iter_loop,
    clippy::assigning_clones,
    clippy::implicit_hasher,
    clippy::ignored_unit_patterns,
    clippy::unnecessary_struct_initialization,
    clippy::string_lit_as_bytes,
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    unused_must_use,
)]

use std::path::Path;
use std::process;

/// Load a .env file if it exists. Returns (true, errors) where true means
/// at least one variable was loaded.
fn load_env_file(path: &Path) -> (bool, Vec<String>) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (false, vec![]),
    };

    let mut loaded = false;
    let mut errors = vec![];

    for (lineno, line) in content.lines().enumerate() {
        let line = line.trim();
        // Skip blank lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Split on first '='
        if let Some(eq_pos) = line.find('=') {
            let key = line[..eq_pos].trim().to_string();
            let value = line[eq_pos + 1..].trim().to_string();
            if key.is_empty() {
                errors.push(format!("line {}: empty key", lineno + 1));
                continue;
            }
            // Only set if not already set in environment (don't override explicit env vars)
            if std::env::var(&key).is_err() {
                std::env::set_var(&key, &value);
                loaded = true;
            }
        } else {
            errors.push(format!("line {}: no '=' found", lineno + 1));
        }
    }

    (loaded, errors)
}

#[tokio::main]
async fn main() {
    // Load .env files: first .atrading/.env relative to cwd, then .env
    if let Ok(cwd) = std::env::current_dir() {
        // Prefer .atrading/.env (for the trading-agent project layout)
        let atrading_env = cwd.join(".atrading").join(".env");
        load_env_file(&atrading_env);

        // Fallback to .env
        let dot_env = cwd.join(".env");
        load_env_file(&dot_env);
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = pi_cli::args::parse_args(&args);
    let exit_code = pi_cli::run::run(&parsed).await;
    process::exit(exit_code);
}
