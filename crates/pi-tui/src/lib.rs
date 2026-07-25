//! pi-tui — Terminal UI framework with Elm architecture.
//!
//! Built on ratatui 0.29 + crossterm 0.28 with Elm-inspired
//! Model / Msg / update / view pattern.

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
    clippy::clone_on_ref_ptr,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
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
    clippy::similar_names,
    clippy::needless_raw_string_hashes,
    clippy::unnested_or_patterns,
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
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
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
