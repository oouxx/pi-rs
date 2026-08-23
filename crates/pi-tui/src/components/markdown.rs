//! Markdown component — thin wrapper over the vendored grok-build streaming
//! markdown pipeline (`xai-grok-markdown`).
//!
//! The pipeline is owned by `crates/vendor/xai-grok-markdown` (pulldown-cmark
//! parsing + syntect highlighting + checkpoint-based streaming); this module
//! keeps the component API stable (`Markdown::new` / `append_text` / `render`)
//! and adds width-aware wrapping for the logical lines the pipeline produces.

use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;
use xai_grok_markdown::{MarkdownStyle, StreamingMarkdownRenderer, Syntect};

/// Theme type for markdown rendering — grok-build's style configuration.
/// A `MarkdownStyle` holds semantic styles (heading / code / table …) which
/// the pipeline maps onto ratatui styles. [`MarkdownTheme::default`] resolves
/// to the Tokyo Night palette (see [`tokyo_night_style`]).
pub type MarkdownTheme = MarkdownStyle;

/// Tokyo Night `.tmTheme` shipped with the vendored pipeline
/// (grok-build uses the same theme file for the TUI).
const TOKYO_NIGHT_THEME: &[u8] =
    include_bytes!("../../../vendor/xai-grok-markdown/assets/tokyo-night.tmTheme");

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
    wrap_width: usize,
}

impl Markdown {
    /// Parse and render markdown source.
    /// The `width` is the available character width for text wrapping.
    pub fn new(source: &str, width: usize) -> Self {
        let mut renderer = StreamingMarkdownRenderer::new(tokyo_night_style(), true);
        renderer.push(source);
        let syntect = Syntect::new(TOKYO_NIGHT_THEME);
        let mut md = Self {
            renderer,
            syntect,
            dirty: true,
            wrapped: Vec::new(),
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
            self.wrapped = wrap_lines(logical, width);
            self.wrap_width = width;
            self.dirty = false;
        }
        &self.wrapped
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Wrapping — unicode-width aware line breaking
// ────────────────────────────────────────────────────────────────────────────

/// Wrap a logical line into physical rows that fit `max_width` display
/// columns. Wide glyphs (CJK, emoji) are never split mid-glyph: if a
/// 2-column glyph would overflow, it starts the next row.
///
/// Width accounting is global across spans (a later span must observe the
/// width already consumed by earlier spans on the same row).
///
/// This is a simplified stand-in for grok-build's
/// `word_wrap_line_with_joiners` (which additionally tracks continuation
/// joiners for copy fidelity — not needed for the minimal TUI).
fn wrap_line(line: &Line<'_>, max_width: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut cur_spans: Vec<ratatui::text::Span<'static>> = Vec::new();
    let mut cur_width = 0usize;

    for span in &line.spans {
        let mut buf = String::new();
        let mut buf_width = 0usize;
        for ch in span.content.chars() {
            let w = ch.width().unwrap_or(0);
            if cur_width + buf_width + w > max_width {
                // Commit the pending buffer to the current row, then break
                // the row (the glyph itself moves to the next row).
                if !buf.is_empty() {
                    cur_spans.push(ratatui::text::Span::styled(
                        std::mem::take(&mut buf),
                        span.style,
                    ));
                    cur_width += buf_width;
                    buf_width = 0;
                }
                if cur_width > 0 {
                    rows.push(Line::from(std::mem::take(&mut cur_spans)));
                    cur_width = 0;
                }
            }
            buf.push(ch);
            buf_width += w;
        }
        if !buf.is_empty() {
            cur_spans.push(ratatui::text::Span::styled(buf, span.style));
            cur_width += buf_width;
        }
    }
    if !cur_spans.is_empty() {
        rows.push(Line::from(cur_spans));
    }
    rows
}

/// Wrap a list of logical lines to `max_width`.
fn wrap_lines(lines: Vec<Line<'static>>, max_width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in lines {
        if line.spans.is_empty() {
            out.push(line);
            continue;
        }
        if line.width() <= max_width {
            out.push(line);
            continue;
        }
        out.extend(wrap_line(&line, max_width));
    }
    out
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
    fn wrap_line_direct() {
        let line = Line::from(ratatui::text::Span::raw("abc 漢字"));
        let rows = wrap_line(&line, 6);
        eprintln!("DIRECT rows: {:?}", rows.iter().map(|l| l.to_string()).collect::<Vec<_>>());
        for row in &rows {
            assert!(row.to_string().width() <= 6, "row {row:?} too wide");
        }
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
