//! Markdown rendering with code block syntax highlighting.
//!
//! Uses `ratatui-markdown` with `HighlightHooks` for tree-sitter based
//! syntax highlighting of code blocks.

use std::sync::Arc;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui_markdown::highlight::{HighlightHooks, TreeSitterHighlighter};
use ratatui_markdown::markdown::{MarkdownRenderer, RenderHooks};
use ratatui_markdown::theme::DefaultTheme;

/// Theme configuration for markdown rendering.
pub struct MarkdownTheme;

impl MarkdownTheme {
    pub fn default() -> Self { Self }
}

/// Rendered markdown content with syntax-highlighted code blocks.
pub struct Markdown {
    text: String,
    rendered: Vec<Line<'static>>,
    dirty: bool,
}

impl Markdown {
    /// Parse and render markdown source.
    /// The `width` is the available character width for text wrapping.
    pub fn new(source: &str, width: usize) -> Self {
        let mut md = Self {
            text: source.to_string(),
            rendered: Vec::new(),
            dirty: true,
        };
        md.render_internal(width);
        md
    }

    /// Append streaming text and mark for re-render.
    pub fn append_text(&mut self, delta: &str) {
        self.text.push_str(delta);
        self.dirty = true;
    }

    pub fn text(&self) -> &str { &self.text }

    /// Get rendered lines (re-renders if dirty).
    pub fn render(&mut self, width: usize) -> &[Line<'static>] {
        if self.dirty || self.rendered.is_empty() {
            self.render_internal(width);
        }
        &self.rendered
    }

    fn render_internal(&mut self, width: usize) {
        let mut renderer = MarkdownRenderer::new(width.max(20));

        // Wire up syntax highlighting
        let highlighter = Arc::new(TreeSitterHighlighter::new());
        let hooks = HighlightHooks::new(highlighter, width.max(20));
        renderer = renderer.with_render_hooks(Box::new(hooks));

        let blocks = renderer.parse(&self.text);
        let theme = DefaultTheme;
        let lines = renderer.render(&blocks, &theme);

        // Post-process: apply line numbering and styling for code blocks
        self.rendered = lines;
        self.dirty = false;
    }
}
