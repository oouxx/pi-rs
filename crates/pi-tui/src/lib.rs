//! pi-tui — Terminal UI framework with Elm architecture.
//!
//! Built on ratatui 0.29 + crossterm 0.28 with Elm-inspired
//! Model / Msg / update / view pattern.

pub mod app;
pub mod clipboard;
pub mod line_screen;
pub mod completion;
pub mod components;
pub mod fuzzy;
pub mod detect;
pub mod keymap;
pub mod render;
pub mod selection;
pub mod terminal;
pub mod theme;

// Re-export key types
pub use app::{AppMode, Cmd, Dialog, DialogAction, DialogButton, Message, Model, Msg};
pub use detect::{detect_terminal_background_theme, detect_theme_from_env, TerminalTheme};
pub use theme::Theme;
pub use components::{
    ArgumentCompletionsFn, Completer, CompletionCommand, CompletionItem, CompletionRequest,
    CompletionTrigger, DiffView, Editor, EditorMode, Input, Markdown, MarkdownTheme, SelectList, TextComponent,
};
pub use keymap::{Action, KeyBind, Keymap};
pub use selection::{Granularity, SelPoint, SelSpace, Selection};
pub use terminal::{ShutdownGuard, Terminal};

/// Utility: render markdown text to styled lines through the vendored
/// grok-build markdown pipeline (pulldown-cmark + syntect + width-aware wrap)
/// with the TS original dark theme palette.
pub fn render_markdown(text: &str) -> Vec<ratatui::text::Line<'static>> {
    xai_grok_markdown::render_markdown_ratatui(
        text,
        components::markdown::style_from_theme(&Theme::default()),
        true,
        Some(components::markdown::default_syntect()),
    )
    .0
}
