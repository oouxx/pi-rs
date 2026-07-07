//! pi-tui — Terminal UI framework with Elm architecture.
//!
//! Built on ratatui 0.29 + crossterm 0.28 with Elm-inspired
//! Model / Msg / update / view pattern.

pub mod app;
pub mod components;
pub mod terminal;

// Re-export key types
pub use app::{Cmd, Model, Msg};
pub use components::{
    DiffView, Editor, EditorMode, Input, Markdown, MarkdownTheme, SelectList, TextComponent,
};
pub use terminal::{ShutdownGuard, Terminal};

/// Utility: render markdown text to styled lines using ratatui-markdown.
pub fn render_markdown(text: &str) -> Vec<ratatui::text::Line<'static>> {
    use ratatui_markdown::markdown::MarkdownRenderer;
    use ratatui_markdown::theme::DefaultTheme;
    let mut renderer = MarkdownRenderer::new(80);
    let blocks = renderer.parse(text);
    let theme = DefaultTheme;
    renderer.render(&blocks, &theme)
}
