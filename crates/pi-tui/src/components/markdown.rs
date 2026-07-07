//! Markdown rendering - placeholder for ratatui-markdown integration.

use ratatui::text::Line;

/// Theme configuration for markdown rendering.
pub struct MarkdownTheme;
impl MarkdownTheme {
    pub fn default() -> Self { Self }
}

/// Rendered markdown content (stub - uses raw text with ratatui-markdown).
pub struct Markdown {
    text: String,
}

impl Markdown {
    pub fn new(source: &str, _theme: &MarkdownTheme) -> Self {
        Self { text: source.to_string() }
    }

    pub fn append_text(&mut self, delta: &str) {
        self.text.push_str(delta);
    }

    pub fn render(&self, _theme: &MarkdownTheme) -> Vec<Line<'static>> {
        // Use ratatui-markdown if available; fallback to raw text
        crate::render_markdown(&self.text)
    }

    pub fn text(&self) -> &str { &self.text }
}
