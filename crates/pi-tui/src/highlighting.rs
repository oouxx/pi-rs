use std::sync::Arc;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// A syntax highlighter that uses syntect (Sublime Text syntax definitions).
///
/// Wraps a `SyntaxSet` (language definitions) and a `Theme` (color scheme)
/// to highlight code for display in ratatui components.
///
/// Implements `Send + Sync` so it can be shared across threads or captured
/// in closures for use as `MarkdownTheme::highlight_code`.
///
/// # Example
///
/// ```ignore
/// use pi_tui::highlighting::SyntaxHighlighter;
/// use pi_tui::components::MarkdownTheme;
///
/// let highlighter = SyntaxHighlighter::new();
/// let mut theme = MarkdownTheme::default_theme();
/// theme.highlight_code = Some(highlighter.into_highlight_fn());
/// ```
pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme: Theme,
}

impl SyntaxHighlighter {
    /// Create a new highlighter with all default syntax definitions
    /// and the built-in `base16-ocean.dark` theme.
    pub fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set
            .themes
            .get("base16-ocean.dark")
            .cloned()
            .unwrap_or_else(|| {
                // Fallback: if the theme name doesn't exist, use the first available theme
                theme_set
                    .themes
                    .into_values()
                    .next()
                    .unwrap_or_else(|| Theme::default())
            });
        Self { syntax_set, theme }
    }

    /// Create a new highlighter with a specific built-in theme name.
    ///
    /// Common built-in themes: `base16-ocean.dark`, `Solarized (dark)`,
    /// `Solarized (light)`, `InspiredGitHub`, `base16-eighties.dark`, etc.
    ///
    /// Returns `None` if the theme name is not found.
    pub fn with_theme(theme_name: &str) -> Option<Self> {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        theme_set.themes.get(theme_name).map(|theme| Self {
            syntax_set,
            theme: theme.clone(),
        })
    }

    /// Create a new highlighter with a custom syntect `Theme`.
    pub fn with_custom_theme(theme: Theme) -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        Self { syntax_set, theme }
    }

    /// Highlight a block of code and return ratatui lines with styled spans.
    ///
    /// * `code` — The source code text to highlight.
    /// * `lang` — An optional language hint (e.g. `"rust"`, `"python"`, `"json"`).
    ///   If `None` or empty, falls back to plain text.
    ///
    /// Language lookup tries token-based matching first, then extension-based,
    /// then name-based, and falls back to plain text if nothing matches.
    pub fn highlight(&self, code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
        let syntax = lang
            .and_then(|l| {
                if l.is_empty() {
                    return None;
                }
                self.syntax_set
                    .find_syntax_by_token(l)
                    .or_else(|| self.syntax_set.find_syntax_by_extension(l))
                    .or_else(|| self.syntax_set.find_syntax_by_name(l))
            })
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let mut lines: Vec<Line<'static>> = Vec::new();

        for line in LinesWithEndings::from(code) {
            // LinesWithEndings preserves the trailing newline character(s).
            // We pass the full line (with ending) to highlight_line so syntect
            // can track state across lines, but we don't display the newline.
            match highlighter.highlight_line(line, &self.syntax_set) {
                Ok(ranges) => {
                    if ranges.is_empty() {
                        lines.push(Line::from(vec![]));
                    } else {
                        let spans: Vec<Span<'static>> = ranges
                            .iter()
                            .map(|(style, text)| {
                                Span::styled(text.to_string(), syntect_style_to_ratatui(style))
                            })
                            .collect();
                        lines.push(Line::from(spans));
                    }
                }
                Err(_) => {
                    // On error, output the raw line as plain text
                    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                    if trimmed.is_empty() {
                        lines.push(Line::from(vec![]));
                    } else {
                        lines.push(Line::from(Span::raw(trimmed.to_string())));
                    }
                }
            }
        }

        lines
    }

    /// Convert this highlighter into a closure suitable for
    /// `MarkdownTheme::highlight_code`.
    ///
    /// The returned closure captures `Self` (via `Arc`) and implements
    /// `Send + Sync`.
    pub fn into_highlight_fn(
        self,
    ) -> Box<dyn Fn(&str, Option<&str>) -> Vec<Line<'static>> + Send + Sync> {
        let this = Arc::new(self);
        Box::new(move |code, lang| this.highlight(code, lang))
    }

    /// Create a highlighter closure in one step (convenience).
    ///
    /// Equivalent to `SyntaxHighlighter::new().into_highlight_fn()`.
    pub fn default_highlight_fn(
    ) -> Box<dyn Fn(&str, Option<&str>) -> Vec<Line<'static>> + Send + Sync> {
        Self::new().into_highlight_fn()
    }
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a syntect `Style` to a ratatui `Style`.
fn syntect_style_to_ratatui(syntect_style: &syntect::highlighting::Style) -> Style {
    let mut style = Style::default();

    let fg = syntect_style.foreground;
    style = style.fg(Color::Rgb(fg.r, fg.g, fg.b));

    let bg = syntect_style.background;
    if bg.a > 0 {
        style = style.bg(Color::Rgb(bg.r, bg.g, bg.b));
    }

    if syntect_style.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if syntect_style.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if syntect_style.font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }

    style
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlighter_creation() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("fn main() {}", Some("rust"));
        assert!(!lines.is_empty(), "Should produce at least one line");
        assert!(
            !lines[0].spans.is_empty(),
            "Syntax-highlighted line should have styled spans"
        );
    }

    #[test]
    fn test_unknown_language_fallback() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("some text", Some("nonexistent_lang_xyz"));
        // Falls back to plain text — still produces a line
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans.len(), 1);
        assert_eq!(lines[0].spans[0].content, "some text");
    }

    #[test]
    fn test_no_language_plain_text() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("plain text", None);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.len() >= 1);
    }

    #[test]
    fn test_empty_code() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("", Some("rust"));
        assert!(lines.is_empty(), "Empty code should produce no lines");
    }

    #[test]
    fn test_multi_line_code() {
        let hl = SyntaxHighlighter::new();
        let code = "fn main() {\n    println!(\"hi\");\n}\n";
        let lines = hl.highlight(code, Some("rust"));
        // LinesWithEndings yields 3 items ("fn main() {\n", "    println!(\"hi\");\n", "}\n")
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_empty_line_in_middle() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("line1\n\nline3", Some("text"));
        // LinesWithEndings yields: "line1\n", "\n", "line3"
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_all_lines_have_styled_spans() {
        let hl = SyntaxHighlighter::new();
        let code = "fn main() {\n    println!(\"hello\");\n}";
        let lines = hl.highlight(code, Some("rust"));
        for (i, line) in lines.iter().enumerate() {
            assert!(
                !line.spans.is_empty(),
                "Line {} should have at least one span",
                i
            );
        }
    }

    #[test]
    fn test_closure_integration() {
        let hl = SyntaxHighlighter::new();
        let highlight_fn = hl.into_highlight_fn();
        let lines = highlight_fn("let x = 42;", Some("rust"));
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].spans.is_empty());
    }

    #[test]
    fn test_default_highlight_fn() {
        let highlight_fn = SyntaxHighlighter::default_highlight_fn();
        let lines = highlight_fn("console.log('hello');", Some("javascript"));
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].spans.is_empty());
    }

    #[test]
    fn test_with_theme() {
        let hl = SyntaxHighlighter::with_theme("Solarized (dark)");
        assert!(hl.is_some(), "Solarized (dark) theme should exist");
        if let Some(hl) = hl {
            let lines = hl.highlight("let x = 1;", Some("rust"));
            assert!(!lines.is_empty());
        }
    }

    #[test]
    fn test_with_theme_unknown() {
        let hl = SyntaxHighlighter::with_theme("nonexistent_theme_xyz");
        assert!(
            hl.is_none(),
            "Unknown theme should return None"
        );
    }

    #[test]
    fn test_highlight_json() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("{\n  \"key\": \"value\"\n}", Some("json"));
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert!(!line.spans.is_empty(), "JSON highlighted lines should have spans");
        }
    }

    #[test]
    fn test_highlight_python() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("def hello():\n    pass", Some("python"));
        assert_eq!(lines.len(), 2);
        for line in &lines {
            assert!(!line.spans.is_empty());
        }
    }

    #[test]
    fn test_multiline_with_trailing_newline_ends_empty() {
        let hl = SyntaxHighlighter::new();
        let lines = hl.highlight("a\nb\n", Some("rust"));
        // LinesWithEndings yields "a\n", "b\n" → 2 lines (newline doesn't create extra empty line)
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_syntect_style_to_ratatui_foreground() {
        let s = syntect::highlighting::Style {
            foreground: syntect::highlighting::Color { r: 255, g: 0, b: 0, a: 255 },
            background: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 0 },
            font_style: FontStyle::empty(),
        };
        let ratatui_style = syntect_style_to_ratatui(&s);
        assert_eq!(ratatui_style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(ratatui_style.bg, None);
    }

    #[test]
    fn test_syntect_style_to_ratatui_background() {
        let s = syntect::highlighting::Style {
            foreground: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 },
            background: syntect::highlighting::Color { r: 10, g: 20, b: 30, a: 255 },
            font_style: FontStyle::empty(),
        };
        let ratatui_style = syntect_style_to_ratatui(&s);
        assert_eq!(ratatui_style.bg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn test_syntect_style_to_ratatui_bold() {
        let s = syntect::highlighting::Style {
            foreground: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 },
            background: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 0 },
            font_style: FontStyle::BOLD,
        };
        let ratatui_style = syntect_style_to_ratatui(&s);
        assert!(ratatui_style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_syntect_style_to_ratatui_italic() {
        let s = syntect::highlighting::Style {
            foreground: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 },
            background: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 0 },
            font_style: FontStyle::ITALIC,
        };
        let ratatui_style = syntect_style_to_ratatui(&s);
        assert!(ratatui_style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_syntect_style_to_ratatui_underline() {
        let s = syntect::highlighting::Style {
            foreground: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 },
            background: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 0 },
            font_style: FontStyle::UNDERLINE,
        };
        let ratatui_style = syntect_style_to_ratatui(&s);
        assert!(ratatui_style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn test_syntect_style_to_ratatui_combined() {
        let s = syntect::highlighting::Style {
            foreground: syntect::highlighting::Color { r: 255, g: 128, b: 0, a: 255 },
            background: syntect::highlighting::Color { r: 0, g: 0, b: 0, a: 255 },
            font_style: FontStyle::BOLD | FontStyle::ITALIC,
        };
        let ratatui_style = syntect_style_to_ratatui(&s);
        assert_eq!(ratatui_style.fg, Some(Color::Rgb(255, 128, 0)));
        assert_eq!(ratatui_style.bg, Some(Color::Rgb(0, 0, 0)));
        assert!(ratatui_style.add_modifier.contains(Modifier::BOLD));
        assert!(ratatui_style.add_modifier.contains(Modifier::ITALIC));
    }
}
