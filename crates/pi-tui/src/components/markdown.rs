//! Markdown component — thin wrapper over the vendored grok-build streaming
//! markdown pipeline (`xai-grok-markdown`).
//!
//! The pipeline is owned by `crates/vendor/xai-grok-markdown` (pulldown-cmark
//! parsing + syntect highlighting + checkpoint-based streaming); this module
//! keeps the component API stable (`Markdown::new` / `append_text` / `render`)
//! and adds width-aware wrapping for the logical lines the pipeline produces.

use ratatui::text::Line;
use xai_grok_markdown::{MarkdownStyle, StreamingMarkdownRenderer, Syntect};

use crate::render::wrap::word_wrap_lines_with_joiners;

/// Theme type for markdown rendering — grok-build's style configuration.
/// A `MarkdownStyle` holds semantic styles (heading / code / table …) which
/// the pipeline maps onto ratatui styles. [`MarkdownTheme::default`] resolves
/// to the TS original dark palette (see [`pi_dark_style`]).
pub type MarkdownTheme = MarkdownStyle;

/// Tokyo Night `.tmTheme` shipped with the vendored pipeline
/// (grok-build uses the same theme file for the TUI).
const TOKYO_NIGHT_THEME: &[u8] =
    include_bytes!("../../../vendor/xai-grok-markdown/assets/tokyo-night.tmTheme");

/// The TS original dark theme markdown palette (`dark.json` md tokens),
/// mapped onto the vendored grok pipeline's `MarkdownStyle`.
///
/// Reference tokens: heading `#f0c674`, link `#81a2be`, linkUrl `#666666`,
/// code `#8abeb7`, codeBlock `#b5bd68`, quote `#808080`, hr `#808080`,
/// listBullet `#8abeb7`, text `#d4d4d4`. Code blocks carry no background
/// (the TS `Markdown` component colors code lines with `mdCodeBlock` only).
pub fn pi_dark_style() -> MarkdownStyle {
    use anstyle::{Color, RgbColor, Style as AnStyle};

    let rgb = |hex: u32| {
        RgbColor(
            ((hex >> 16) & 0xff) as u8,
            ((hex >> 8) & 0xff) as u8,
            (hex & 0xff) as u8,
        )
    };
    let fg = |hex: u32| AnStyle::new().fg_color(Some(Color::Rgb(rgb(hex))));
    let hidden = AnStyle::new().hidden();

    MarkdownStyle {
        heading_inner: [fg(0xf0c674).bold(); 6],
        heading_outer: [hidden; 6],
        strong_inner: fg(0xd4d4d4).bold(),
        strong_outer: hidden,
        emphasis_inner: fg(0xd4d4d4).italic(),
        emphasis_outer: hidden,
        strikethrough_inner: fg(0x808080).strikethrough(),
        strikethrough_outer: hidden,
        inline_code_inner: fg(0x8abeb7),
        inline_code_outer: hidden,
        blockquote_outer: fg(0x808080).italic(),
        task_checked: fg(0xb5bd68),
        task_unchecked: fg(0x808080),
        list_item: fg(0x8abeb7),
        rule: fg(0x808080),
        link_outer: hidden,
        link_text: fg(0x81a2be).underline(),
        link_url: fg(0x666666),
        link_title: fg(0x666666),
        code_outer: hidden,
        code_language: hidden,
        code_untagged: fg(0xb5bd68),
        code_background: fg(0xb5bd68),
        table_outer: fg(0x8abeb7).bold(),
        text: fg(0xd4d4d4),
        math: fg(0x81a2be),
    }
}

/// Tokyo Night markdown style — the previous default, kept for reference
/// (grok-build's `tokyonight` markdown palette).
pub fn tokyo_night_style() -> MarkdownStyle {
    use anstyle::{Color, RgbColor, Style as AnStyle};

    let rgb = |hex: u32| {
        RgbColor(
            ((hex >> 16) & 0xff) as u8,
            ((hex >> 8) & 0xff) as u8,
            (hex & 0xff) as u8,
        )
    };
    let fg = |hex: u32| AnStyle::new().fg_color(Some(Color::Rgb(rgb(hex))));
    let bg = |hex: u32| AnStyle::new().bg_color(Some(Color::Rgb(rgb(hex))));
    let hidden = AnStyle::new().hidden();

    MarkdownStyle {
        heading_inner: [fg(0x7aa2f7).bold(); 6],
        heading_outer: [hidden; 6],
        strong_inner: fg(0xc0caf5).bold(),
        strong_outer: hidden,
        emphasis_inner: fg(0xc0caf5).italic(),
        emphasis_outer: hidden,
        strikethrough_inner: fg(0x565f89).strikethrough(),
        strikethrough_outer: hidden,
        inline_code_inner: fg(0x9ece6a)
            .bg_color(Some(Color::Rgb(rgb(0x1a1b26)))),
        inline_code_outer: hidden,
        blockquote_outer: fg(0xbb9af7),
        task_checked: fg(0x9ece6a),
        task_unchecked: fg(0x565f89),
        list_item: fg(0x7aa2f7),
        rule: fg(0x363b4f),
        link_outer: hidden,
        link_text: fg(0x7dcbf8).underline(),
        link_url: fg(0x565f89),
        link_title: fg(0x565f89),
        code_outer: hidden,
        code_language: hidden,
        code_untagged: fg(0x9ece6a),
        code_background: bg(0x16161e),
        table_outer: fg(0x7aa2f7).bold(),
        text: fg(0xc0caf5),
        math: fg(0xbb9af7),
    }
}

/// Rendered markdown content with syntax-highlighted code blocks.
pub struct Markdown {
    renderer: StreamingMarkdownRenderer,
    syntect: Syntect,
    dirty: bool,
    wrapped: Vec<Line<'static>>,
    /// Soft-wrap joiners (one per wrapped row, from grok-build's
    /// `word_wrap_lines_with_joiners`) — the exact substring skipped at each
    /// wrap boundary, for copy/selection fidelity once a scrollback layer
    /// exists. `None` rows are hard breaks.
    joiners: Vec<Option<String>>,
    wrap_width: usize,
}

impl Markdown {
    /// Parse and render markdown source.
    /// The `width` is the available character width for text wrapping.
    pub fn new(source: &str, width: usize) -> Self {
        let mut renderer = StreamingMarkdownRenderer::new(pi_dark_style(), true);
        renderer.push(source);
        let syntect = Syntect::new(TOKYO_NIGHT_THEME);
        let mut md = Self {
            renderer,
            syntect,
            dirty: true,
            wrapped: Vec::new(),
            joiners: Vec::new(),
            wrap_width: 0,
        };
        let _ = md.render(width);
        md
    }

    /// Append streaming text and mark for re-render.
    pub fn append_text(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.renderer.push(delta);
        self.dirty = true;
    }

    pub fn text(&self) -> &str {
        self.renderer.source()
    }

    /// Get rendered lines (re-renders if dirty).
    pub fn render(&mut self, width: usize) -> &[Line<'static>] {
        // Guard against degenerate widths (0 would make wrapping misbehave).
        let width = width.max(1);
        if self.dirty || self.wrap_width != width {
            self.renderer.render(Some(&self.syntect));
            let logical = self.renderer.view().lines.to_vec();
            let (wrapped, joiners) = word_wrap_lines_with_joiners(logical, width);
            self.wrapped = wrapped;
            self.joiners = joiners;
            self.wrap_width = width;
            self.dirty = false;
        }
        &self.wrapped
    }

    /// Soft-wrap joiners parallel to the last [`Self::render`] result.
    pub fn joiners(&self) -> &[Option<String>] {
        &self.joiners
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// A markdown doc exercising headings, inline code, fenced code, tables
    /// and wide (CJK) characters — the pipeline must render it without
    /// panicking and with the expected line structure.
    const SAMPLE_MD: &str = "# Title\n\nSome **bold** and `inline` text with 漢字.\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

    #[test]
    fn renders_structured_markdown_without_panic() {
        let mut md = Markdown::new(SAMPLE_MD, 80);
        let lines = md.render(80);
        assert!(!lines.is_empty(), "must produce lines");
        // The title must be present in some line's plain text.
        let plain: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(plain.contains("Title"), "heading text rendered: {plain}");
        assert!(plain.contains("fn main()"), "code block rendered: {plain}");
        // Table borders: header row is rendered with │ separators.
        assert!(plain.contains('│'), "table borders rendered: {plain}");
    }

    #[test]
    fn streaming_append_updates_render() {
        let mut md = Markdown::new("Hello", 80);
        let before = md.render(80).to_vec();
        assert!(before.iter().any(|l| l.to_string().contains("Hello")));

        md.append_text(" world");
        let after = md.render(80);
        let plain: String = after
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(plain.contains("Hello world"), "appended delta visible: {plain}");
    }

    #[test]
    fn wrapping_respects_display_width_and_wide_glyphs() {
        // "漢字" is 2 columns per glyph; at width 6 the two CJK chars must
        // start a new row rather than being split mid-glyph.
        let mut md = Markdown::new("abc 漢字", 6);
        let lines = md.render(6);        let rows: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        // Every physical row fits the width budget.
        for row in &rows {
            assert!(
                row.width() <= 6,
                "row {row:?} exceeds width (measured {})",
                row.width()
            );
        }
        // No characters may be lost or duplicated across the wrap.
        assert_eq!(rows.concat(), "abc 漢字", "chars preserved: {rows:?}");
        eprintln!("DEBUG rows: {rows:?}");
    }

    #[test]
    fn grok_wrap_respects_width_and_joiners() {
        // The vendored grok wrap is width-aware (CJK = 2 columns) and returns
        // joiners: the exact substring skipped at each soft-wrap boundary.
        let line = Line::from(ratatui::text::Span::raw("abc 漢字"));
        let (rows, joiners) =
            crate::render::wrap::word_wrap_line_with_joiners(&line, 6);
        for row in &rows {
            assert!(row.to_string().width() <= 6, "row {row:?} too wide");
        }
        // Joiners: first row hard break (None), continuation rows carry the
        // skipped whitespace so re-joining restores the original text.
        let rejoined: String = rows.iter().map(|l| l.to_string()).collect();
        assert_eq!(rejoined, "abc 漢字", "chars preserved across wrap");
        assert!(joiners[0].is_none(), "first row has no joiner");
    }

    #[test]
    fn width_one_does_not_split_wide_glyph() {
        // At width 1 a 2-column CJK glyph cannot fit; it must start its own
        // (necessarily over-wide) row rather than being split or panicking.
        let mut md = Markdown::new("漢", 1);
        let lines = md.render(1);
        let rows: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect();
        assert_eq!(rows, vec!["漢"], "single over-wide glyph row: {rows:?}");
    }

    #[test]
    fn empty_and_whitespace_input_are_safe() {
        let mut md = Markdown::new("", 80);
        assert_eq!(md.render(80).len(), 0, "empty input renders nothing");
        md.append_text("\n\n");
        let _ = md.render(80); // must not panic
        assert!(md.text().len() >= 2);
    }
}
