//! Markdown component — thin wrapper over the vendored grok-build streaming
//! markdown pipeline (`xai-grok-markdown`).
//!
//! The pipeline is owned by `crates/vendor/xai-grok-markdown` (pulldown-cmark
//! parsing + syntect highlighting + checkpoint-based streaming); this module
//! keeps the component API stable (`Markdown::new` / `append_text` / `render`)
//! and adds width-aware wrapping for the logical lines the pipeline produces.
//!
//! Both the markdown palette ([`MarkdownStyle`]) and the code-block syntax
//! palette (a `syntect` theme) are derived from the active [`Theme`], so a
//! `/theme dark` → `/theme light` switch re-colours existing blocks (TS
//! drives every markdown element from the one global `theme` instance).

use ratatui::style::Color as RataColor;
use ratatui::text::Line;
use xai_grok_markdown::{MarkdownStyle, StreamingMarkdownRenderer, Syntect};

use crate::render::wrap::word_wrap_lines_with_joiners;
use crate::theme::Theme;

/// Theme type for markdown rendering — grok-build's style configuration.
/// A `MarkdownStyle` holds semantic styles (heading / code / table …) which
/// the pipeline maps onto ratatui styles. [`MarkdownTheme::default`] resolves
/// to the TS original dark palette (see [`style_from_theme`]).
pub type MarkdownTheme = MarkdownStyle;

/// Tokyo Night `.tmTheme` shipped with the vendored pipeline. Only used as a
/// bootstrap so [`Syntect::new`] can load its syntax set; the parser's theme
/// field is immediately replaced by [`syntect_theme`] (the active [`Theme`]'s
/// `syntax*` tokens). It is never used as the rendered syntax palette.
const TOKYO_NIGHT_THEME: &[u8] =
    include_bytes!("../../../vendor/xai-grok-markdown/assets/tokyo-night.tmTheme");

/// Build an `anstyle` style with `color` as the foreground.
fn anstyle_fg(color: RataColor) -> anstyle::Style {
    match color {
        RataColor::Rgb(r, g, b) => {
            anstyle::Style::new().fg_color(Some(anstyle::Color::Rgb(anstyle::RgbColor(r, g, b))))
        }
        _ => anstyle::Style::new(),
    }
}

/// Map the active [`Theme`]'s semantic tokens onto the vendored pipeline's
/// [`MarkdownStyle`]. Mirrors the TS `getMarkdownTheme()` wiring: heading →
/// `mdHeading`, link → `mdLink`, code → `mdCode`, code block → `mdCodeBlock`,
/// quote → `mdQuote`, rule → `mdHr`, list bullet → `mdListBullet`, body text →
/// `text`. Code blocks carry no background (TS colours code lines with
/// `mdCodeBlock` only).
#[must_use]
pub fn style_from_theme(t: &Theme) -> MarkdownStyle {
    let hidden = anstyle::Style::new().hidden();
    MarkdownStyle {
        heading_inner: [anstyle_fg(t.md_heading).bold(); 6],
        heading_outer: [hidden; 6],
        strong_inner: anstyle_fg(t.text).bold(),
        strong_outer: hidden,
        emphasis_inner: anstyle_fg(t.text).italic(),
        emphasis_outer: hidden,
        strikethrough_inner: anstyle_fg(t.muted).strikethrough(),
        strikethrough_outer: hidden,
        inline_code_inner: anstyle_fg(t.md_code),
        inline_code_outer: hidden,
        blockquote_outer: anstyle_fg(t.md_quote).italic(),
        task_checked: anstyle_fg(t.success),
        task_unchecked: anstyle_fg(t.muted),
        list_item: anstyle_fg(t.md_list_bullet),
        rule: anstyle_fg(t.md_hr),
        link_outer: hidden,
        link_text: anstyle_fg(t.md_link).underline(),
        link_url: anstyle_fg(t.md_link_url),
        link_title: anstyle_fg(t.md_link_url),
        code_outer: hidden,
        code_language: hidden,
        code_untagged: anstyle_fg(t.md_code_block),
        code_background: anstyle_fg(t.md_code_block),
        table_outer: anstyle_fg(t.accent).bold(),
        text: anstyle_fg(t.text),
        math: anstyle_fg(t.md_link),
    }
}

/// The TS original dark theme markdown palette (`dark.json` @ v0.82.1).
///
/// Kept as a convenience alias for callers that have no `Theme` at hand;
/// prefer [`style_from_theme`].
#[must_use]
pub fn pi_dark_style() -> MarkdownStyle {
    style_from_theme(&Theme::default())
}

/// Convert a ratatui colour to a `syntect` colour (`a` defaults to opaque).
fn syntect_color(color: RataColor) -> syntect::highlighting::Color {
    match color {
        RataColor::Rgb(r, g, b) => syntect::highlighting::Color { r, g, b, a: 0xff },
        _ => syntect::highlighting::Color {
            r: 0xd4,
            g: 0xd4,
            b: 0xd4,
            a: 0xff,
        },
    }
}

/// Build a `syntect` theme whose scope rules use the active [`Theme`]'s
/// `syntax*` tokens.
///
/// This approximates the TS `buildCliHighlightTheme()` mapping (highlight.js
/// categories → `syntaxKeyword`/`syntaxString`/… ) on syntect's TextMate
/// scope names. Where the two ecosystems disagree the mapping errs toward the
/// token that best matches the scope; see `DEVIATIONS.md`.
fn syntect_theme(t: &Theme) -> syntect::highlighting::Theme {
    use std::str::FromStr;
    use syntect::highlighting::{ScopeSelectors, StyleModifier, ThemeItem, ThemeSettings};

    let rule = |scope: &str, color: RataColor| -> Option<ThemeItem> {
        let selectors = ScopeSelectors::from_str(scope).ok()?;
        Some(ThemeItem {
            scope: selectors,
            style: StyleModifier {
                foreground: Some(syntect_color(color)),
                background: None,
                font_style: None,
            },
        })
    };

    // Order matters for equal-specificity ties (later rules win): put the
    // coarse `keyword`/`punctuation` rules first so the specific operator /
    // type / function selectors below override them where they are deeper.
    let rules: &[(&str, RataColor)] = &[
        ("comment", t.syntax_comment),
        ("keyword", t.syntax_keyword),
        ("storage", t.syntax_keyword),
        ("keyword.operator, punctuation.definition.operator", t.syntax_operator),
        ("string, constant.other.symbol, string.regexp", t.syntax_string),
        (
            "constant.numeric, constant.language, constant.character, constant.other, support.constant",
            t.syntax_number,
        ),
        (
            "entity.name.function, support.function, meta.function-call, variable.function",
            t.syntax_function,
        ),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, support.type, support.class, storage.type",
            t.syntax_type,
        ),
        (
            "variable, variable.parameter, variable.other, support.variable, meta.object-literal.key",
            t.syntax_variable,
        ),
        ("entity.name.tag", t.syntax_punctuation),
        ("punctuation", t.syntax_punctuation),
    ];

    let scopes: Vec<ThemeItem> = rules
        .iter()
        .filter_map(|(scope, color)| rule(scope, *color))
        .collect();

    syntect::highlighting::Theme {
        name: Some(format!("pi-{}", t.name)),
        author: None,
        settings: ThemeSettings {
            foreground: Some(syntect_color(t.text)),
            ..ThemeSettings::default()
        },
        scopes,
    }
}

/// A `Syntect` highlighter whose syntax set comes from the vendored bundle and
/// whose theme is [`syntect_theme`].
fn syntect_for_theme(t: &Theme) -> Syntect {
    let mut syntect = Syntect::new(TOKYO_NIGHT_THEME);
    syntect.theme = syntect_theme(t);
    syntect
}

/// Shared theme-driven `Syntect` for the default (dark) palette, used by the
/// free-standing [`crate::render_markdown`] helper (one syntax-set load).
#[must_use]
pub fn default_syntect() -> &'static Syntect {
    static SYNTECT: std::sync::OnceLock<Syntect> = std::sync::OnceLock::new();
    SYNTECT.get_or_init(|| syntect_for_theme(&Theme::default()))
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
    /// Parse and render markdown source with the default (dark) theme.
    /// The `width` is the available character width for text wrapping.
    ///
    /// Prefer [`Markdown::with_theme`] so the block follows the active theme.
    pub fn new(source: &str, width: usize) -> Self {
        Self::with_theme(source, width, &Theme::default())
    }

    /// Parse and render markdown source with the given theme. The `width` is
    /// the available character width for text wrapping.
    pub fn with_theme(source: &str, width: usize, theme: &Theme) -> Self {
        let mut renderer = StreamingMarkdownRenderer::new(style_from_theme(theme), true);
        renderer.push(source);
        let mut md = Self {
            renderer,
            syntect: syntect_for_theme(theme),
            dirty: true,
            wrapped: Vec::new(),
            joiners: Vec::new(),
            wrap_width: 0,
        };
        let _ = md.render(width);
        md
    }

    /// Re-colour this block for a new theme. The next [`Self::render`]
    /// rebuilds the output from the retained source (style + syntax caches are
    /// reset), so existing blocks follow `/theme` without re-streaming.
    pub fn set_theme(&mut self, theme: &Theme) {
        self.renderer.set_style(style_from_theme(theme));
        self.syntect.theme = syntect_theme(theme);
        self.dirty = true;
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
    #![allow(clippy::expect_used, clippy::unwrap_used)]
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
        assert!(
            plain.contains("Hello world"),
            "appended delta visible: {plain}"
        );
    }

    #[test]
    fn wrapping_respects_display_width_and_wide_glyphs() {
        // "漢字" is 2 columns per glyph; at width 6 the two CJK chars must
        // start a new row rather than being split mid-glyph.
        let mut md = Markdown::new("abc 漢字", 6);
        let lines = md.render(6);
        let rows: Vec<String> = lines
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
        let (rows, joiners) = crate::render::wrap::word_wrap_line_with_joiners(&line, 6);
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

    /// The rendered heading colour must come from the active theme, so a
    /// `/theme light` switch is visible in already-rendered blocks.
    #[test]
    fn heading_color_follows_theme() {
        let dark = Theme::default();
        let light = Theme::light();
        let mut md = Markdown::with_theme("# Title", 80, &dark);
        assert_eq!(heading_fg(&render_owned(&mut md, 80)), dark.md_heading);
        md.set_theme(&light);
        assert_eq!(heading_fg(&render_owned(&mut md, 80)), light.md_heading);
        assert_ne!(dark.md_heading, light.md_heading, "tokens must differ");
    }

    /// Test helper: clone the rendered lines so the borrow of `md` ends.
    fn render_owned(md: &mut Markdown, width: usize) -> Vec<Line<'static>> {
        md.render(width).to_vec()
    }

    /// The heading span's foreground in a rendered document.
    fn heading_fg(lines: &[Line<'static>]) -> ratatui::style::Color {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.contains("Title"))
            .and_then(|s| s.style.fg)
            .expect("heading span has a foreground")
    }

    /// Light vs dark markdown styles must differ, and the syntax theme must be
    /// rebuilt from the theme's `syntax*` tokens.
    #[test]
    fn syntax_theme_uses_theme_tokens() {
        let dark = syntect_theme(&Theme::default());
        assert_eq!(dark.name.as_deref(), Some("pi-dark"));
        let light = syntect_theme(&Theme::light());
        assert_eq!(light.name.as_deref(), Some("pi-light"));
        // The two themes must not be identical (different token values).
        assert_ne!(dark.scopes.len(), 0);
        assert_ne!(dark.scopes, light.scopes);
    }
}
