//! pi-tui — Terminal UI framework with Elm architecture.
//!
//! Built on ratatui 0.29 + crossterm 0.28 with Elm-inspired
//! Model / Msg / update / view pattern.

#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::must_use_candidate,
    clippy::missing_const_for_fn,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::match_same_arms,
    clippy::unnested_or_patterns,
    clippy::redundant_closure,
    clippy::bool_to_int_with_if,
    clippy::uninlined_format_args,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::default_trait_access,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::use_self,
    clippy::or_fun_call,
    clippy::single_match_else,
    clippy::needless_collect,
    clippy::let_underscore_must_use,
    clippy::significant_drop_tightening,
    clippy::trivially_copy_pass_by_ref,
    clippy::redundant_clone,
    clippy::unnecessary_operation,
    clippy::needless_continue,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::derive_partial_eq_without_eq,
    clippy::string_lit_as_bytes,
    clippy::manual_string_new,
    clippy::iter_with_drain,
    clippy::if_not_else,
    clippy::explicit_iter_loop,
    clippy::assigning_clones,
    clippy::implicit_hasher,
    clippy::ignored_unit_patterns,
    clippy::missing_fields_in_debug,
    clippy::too_long_first_doc_paragraph,
    clippy::significant_drop_in_scrutinee,
    clippy::duplicated_attributes,
    clippy::needless_raw_string_hashes,
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
    clippy::large_futures,
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
    dead_code,
    unused_must_use,
)]

pub mod app;
pub mod components;
pub mod keymap;
pub mod terminal;

// Re-export key types
pub use app::{Cmd, Model, Msg};
pub use components::{
    Completer, CompletionItem, CompletionTrigger, DiffView, Editor, EditorMode, Input, Markdown,
    MarkdownTheme, SelectList, TextComponent,
};
pub use keymap::{Action, KeyBind, Keymap};
pub use terminal::{ShutdownGuard, Terminal};

/// Utility: render markdown text to styled lines using ratatui-markdown.
pub fn render_markdown(text: &str) -> Vec<ratatui::text::Line<'static>> {
    use ratatui_markdown::markdown::MarkdownRenderer;
    use ratatui_markdown::theme::ThemeConfig;
    let mut renderer = MarkdownRenderer::new(80);
    let blocks = renderer.parse(text);
    let theme = ThemeConfig::default();
    renderer.render(&blocks, &theme)
}
