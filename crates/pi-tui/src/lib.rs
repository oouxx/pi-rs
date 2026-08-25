//! pi-tui — Terminal UI framework with Elm architecture.
//!
//! Built on ratatui 0.29 + crossterm 0.28 with Elm-inspired
//! Model / Msg / update / view pattern.

pub mod app;
pub mod completion;
pub mod components;
pub mod fuzzy;
pub mod detect;
pub mod keymap;
pub mod render;
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
pub use terminal::{ShutdownGuard, Terminal};

/// Utility: render markdown text to styled lines through the vendored
/// grok-build markdown pipeline (pulldown-cmark + syntect + width-aware wrap)
/// with the TS original dark theme palette.
pub fn render_markdown(text: &str) -> Vec<ratatui::text::Line<'static>> {
    static SYNTECT: std::sync::OnceLock<xai_grok_markdown::Syntect> = std::sync::OnceLock::new();
    let syntect = SYNTECT.get_or_init(|| {
        xai_grok_markdown::Syntect::new(include_bytes!(
            "../../vendor/xai-grok-markdown/assets/tokyo-night.tmTheme"
        ))
    });
    xai_grok_markdown::render_markdown_ratatui(
        text,
        components::markdown::pi_dark_style(),
        true,
        Some(syntect),
    )
    .0
}
