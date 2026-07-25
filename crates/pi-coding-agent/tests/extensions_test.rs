//! Extension system tests for pi-coding-agent.
//!
//! These tests cover ToolDefinition serialization/deserialization.
//! Tests for the old load_extensions/LoadedExtension/ToolInfo system
//! were removed in Phase 6.6 cleanup (those types were dead code
//! replaced by the embedded deno_core JS runtime).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::exit,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::redundant_closure,
    clippy::redundant_clone,
    clippy::uninlined_format_args,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::unreadable_literal,
    clippy::similar_names,
    clippy::large_futures,
    clippy::items_after_statements,
    clippy::let_underscore_must_use,
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
    clippy::derive_partial_eq_without_eq,
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
    clippy::missing_const_for_fn,
    clippy::unnecessary_struct_initialization,
    clippy::string_lit_as_bytes,
    clippy::significant_drop_tightening,
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    unused_must_use,
)]

use pi_coding_agent::core::extensions::ToolDefinition;

// ============================================================================
// Helper functions
// ============================================================================

/// Create a basic tool definition for testing.
fn make_tool_def(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        label: None,
        description: description.to_string(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: None,
        execute: None,
    }
}

// ============================================================================
// ToolDefinition tests
// ============================================================================

#[test]
fn test_tool_definition_serialization_roundtrip() {
    let def = make_tool_def("read_file", "Read a file from the filesystem");
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "read_file");
    assert_eq!(parsed.description, "Read a file from the filesystem");
}

#[test]
fn test_tool_definition_minimal_serialization() {
    let def = ToolDefinition {
        name: "minimal".into(),
        label: None,
        description: String::new(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: None,
        execute: None,
    };
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "minimal");
    assert!(parsed.description.is_empty());
}

#[test]
fn test_tool_definition_with_execution_mode() {
    let def = ToolDefinition {
        name: "sequential_tool".into(),
        label: None,
        description: "A sequential tool".into(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: Some("sequential".into()),
        execute: None,
    };
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.execution_mode, Some("sequential".to_string()));
}

#[test]
fn test_tool_definition_with_parallel_mode() {
    let def = ToolDefinition {
        name: "parallel_tool".into(),
        label: None,
        description: "A parallel tool".into(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: Some("parallel".into()),
        execute: None,
    };
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.execution_mode, Some("parallel".to_string()));
}

