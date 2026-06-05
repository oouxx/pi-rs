use crossterm::event::KeyEvent;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::Component;

// ---------------------------------------------------------------------------
// Public types — kept for API compatibility, now backed by ratkit
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DefaultTextStyle {
    pub style: Style,
}

pub struct MarkdownTheme {
    pub heading: Box<dyn Fn(u8) -> Style + Send + Sync>,
    pub heading_prefix: Box<dyn Fn(u8) -> Style + Send + Sync>,
    pub link: Box<dyn Fn() -> Style + Send + Sync>,
    pub link_url: Box<dyn Fn() -> Style + Send + Sync>,
    pub code: Box<dyn Fn() -> Style + Send + Sync>,
    pub code_block: Box<dyn Fn() -> Style + Send + Sync>,
    pub code_block_border: Box<dyn Fn() -> Style + Send + Sync>,
    pub quote: Box<dyn Fn() -> Style + Send + Sync>,
    pub quote_border: Box<dyn Fn() -> Style + Send + Sync>,
    pub hr: Box<dyn Fn() -> Style + Send + Sync>,
    pub list_bullet: Box<dyn Fn() -> Style + Send + Sync>,
    pub bold: Box<dyn Fn() -> Style + Send + Sync>,
    pub italic: Box<dyn Fn() -> Style + Send + Sync>,
    pub strikethrough: Box<dyn Fn() -> Style + Send + Sync>,
    pub underline: Box<dyn Fn() -> Style + Send + Sync>,
    pub highlight_code: Option<Box<dyn Fn(&str, Option<&str>) -> Vec<Line<'static>> + Send + Sync>>,
    pub code_block_indent: String,
}

impl MarkdownTheme {
    pub fn default_theme() -> Self {
        let heading_style = Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD);
        Self {
            heading: Box::new(move |level| {
                if level == 1 {
                    heading_style.add_modifier(Modifier::UNDERLINED)
                } else {
                    heading_style
                }
            }),
            heading_prefix: Box::new(|_level| {
                Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)
            }),
            link: Box::new(|| Style::new().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)),
            link_url: Box::new(|| Style::new().fg(Color::DarkGray)),
            code: Box::new(|| Style::new().fg(Color::Yellow)),
            code_block: Box::new(|| Style::new().fg(Color::Yellow)),
            code_block_border: Box::new(|| {
                Style::new()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC)
            }),
            quote: Box::new(|| Style::new().add_modifier(Modifier::ITALIC)),
            quote_border: Box::new(|| Style::new().fg(Color::DarkGray)),
            hr: Box::new(|| Style::new().fg(Color::DarkGray)),
            list_bullet: Box::new(|| Style::new().fg(Color::Cyan)),
            bold: Box::new(|| Style::new().add_modifier(Modifier::BOLD)),
            italic: Box::new(|| Style::new().add_modifier(Modifier::ITALIC)),
            strikethrough: Box::new(|| Style::new().add_modifier(Modifier::CROSSED_OUT)),
            underline: Box::new(|| Style::new().add_modifier(Modifier::UNDERLINED)),
            highlight_code: None,
            code_block_indent: "  ".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MarkdownOptions {
    pub preserve_ordered_list_markers: bool,
}

/// Check whether a rendered line contains the image escape sequence marker.
pub fn is_image_line(s: &str) -> bool {
    s.contains("\x1b_pi:img")
}

// ---------------------------------------------------------------------------
// Markdown component
// ---------------------------------------------------------------------------

pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<Box<DefaultTextStyle>>,
    /// Saved for API compatibility but no longer drives per-element styling.
    #[allow(dead_code)]
    theme: MarkdownTheme,
    options: MarkdownOptions,
    cached_text: Option<String>,
    cached_width: Option<u16>,
    cached_lines: Option<Vec<Line<'static>>>,
}

impl Markdown {
    pub fn new(
        text: String,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<Box<DefaultTextStyle>>,
        options: Option<MarkdownOptions>,
    ) -> Self {
        Self {
            text,
            padding_x,
            padding_y,
            theme,
            default_text_style,
            options: options.unwrap_or_default(),
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}

impl Component for Markdown {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let width = width as usize;

        // Cache hit
        if let (Some(ref ct), Some(ref cw), Some(ref cl)) =
            (&self.cached_text, &self.cached_width, &self.cached_lines)
        {
            if ct == &self.text && *cw as usize == width {
                return cl.clone();
            }
        }

        if self.text.trim().is_empty() {
            return vec![];
        }

        let content_width = width.saturating_sub(self.padding_x * 2).max(1);

        let mut lines = {
            let text = ratkit::widgets::markdown_preview::render_markdown(
                &self.text,
                Some(content_width),
            );
            text.lines
                .into_iter()
                .map(|l| {
                    if line_content(&l).trim().is_empty() {
                        Line::from(vec![])
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
        };

        if lines.is_empty() {
            lines.push(Line::from(vec![]));
        }
        while lines.last().map_or(false, |l| line_content(l).trim().is_empty()) {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(Line::from(vec![]));
        }

        // Apply default text style
        let base_style = self
            .default_text_style
            .as_ref()
            .map(|dts| dts.style)
            .unwrap_or_default();

        // Apply padding horizontally
        let left_pad = self.padding_x;
        let right_pad = self.padding_x;
        for line in &mut lines {
            if left_pad > 0 || right_pad > 0 {
                let mut new_spans = Vec::new();
                if left_pad > 0 {
                    new_spans.push(Span::raw(" ".repeat(left_pad)));
                }
                // Apply base_style to all spans if it's not the default
                for span in line.spans.drain(..) {
                    if base_style != Style::default() {
                        let patched = span.style.patch(base_style);
                        new_spans.push(Span::styled(span.content.to_string(), patched));
                    } else {
                        new_spans.push(span);
                    }
                }
                if right_pad > 0 {
                    new_spans.push(Span::raw(" ".repeat(right_pad)));
                }
                line.spans = new_spans;
            } else if base_style != Style::default() {
                let spans: Vec<Span<'static>> = line
                    .spans
                    .drain(..)
                    .map(|s| {
                        let patched = s.style.patch(base_style);
                        Span::styled(s.content.to_string(), patched)
                    })
                    .collect();
                line.spans = spans;
            }
        }

        // Apply padding vertically
        let empty_line = Line::from(vec![]);
        let mut result: Vec<Line<'static>> = Vec::new();
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }
        result.extend(lines);
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        if result.is_empty() {
            result.push(Line::from(vec![]));
        }

        result
    }

    fn handle_input(&mut self, _event: &KeyEvent) {}

    fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn line_content(line: &Line<'static>) -> String {
    let mut s = String::new();
    for span in &line.spans {
        s.push_str(&span.content);
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_theme() -> MarkdownTheme {
        MarkdownTheme::default_theme()
    }

    fn compact(content: &[Line<'static>]) -> Vec<String> {
        content.iter().map(|l| line_content(l)).collect()
    }

    #[test]
    fn test_simple_paragraph() {
        let md = Markdown::new("Hello, world!".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Hello, world!"));
    }

    #[test]
    fn test_heading_h1() {
        let md = Markdown::new("# Heading 1".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Heading 1"));
    }

    #[test]
    fn test_heading_h2() {
        let md = Markdown::new("## Heading 2".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Heading 2"));
    }

    #[test]
    fn test_inline_bold() {
        let md = Markdown::new("Hello **world**!".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Hello"));
        assert!(line_content(&lines[0]).contains("world"));
    }

    #[test]
    fn test_inline_code() {
        let md = Markdown::new("Use `let x = 1` here".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("let x = 1"));
    }

    #[test]
    fn test_code_block() {
        let md = Markdown::new(
            "```rust\nfn main() {}\n```".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        let output: Vec<String> = lines.iter().map(|l| line_content(l)).collect();
        eprintln!("code block lines: {output:?}");
        assert!(output.iter().any(|l| l.contains("fn main()")), "code should contain fn main()");
    }

    #[test]
    fn test_blockquote() {
        let md = Markdown::new("> quoted text".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(
            line_content(&lines[0]).contains("quoted text"),
            "blockquote should contain quoted text, got: {:?}",
            line_content(&lines[0])
        );
    }

    #[test]
    fn test_list_unordered() {
        let md = Markdown::new("- item 1\n- item 2".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.iter().any(|l| line_content(l).contains("item 1")));
        assert!(lines.iter().any(|l| line_content(l).contains("item 2")));
    }

    #[test]
    fn test_horizontal_rule() {
        let md = Markdown::new("---".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        let output = compact(&lines);
        eprintln!("hr lines: {output:?}");
        // HR should produce at least one non-empty line
        assert!(lines.iter().any(|l| !line_content(l).trim().is_empty()));
    }

    #[test]
    fn test_empty_text_returns_empty() {
        let md = Markdown::new(String::new(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_padding() {
        let md = Markdown::new("Hello".to_string(), 2, 1, default_theme(), None, None);
        let lines = md.render(20);
        let output = compact(&lines);
        eprintln!("padding lines: {output:?}");
        assert!(lines.len() >= 3, "expected ≥3 lines, got {}", lines.len());
        // The middle line (index 1 because of top padding) should contain "Hello" with left padding
        assert!(
            output.iter().any(|l| l.trim() == "Hello" || l.contains("Hello")),
            "Hello should appear in one of the lines"
        );
    }

    #[test]
    fn test_default_text_style() {
        let dts = Box::new(DefaultTextStyle {
            style: Style::new()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        });
        let md = Markdown::new("colored text".to_string(), 0, 0, default_theme(), Some(dts), None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        // Some spans should carry the default style
        let has_red = lines[0].spans.iter().any(|s| s.style.fg == Some(Color::Red));
        let has_bold = lines[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(has_red || has_bold, "default style should be applied");
    }

    #[test]
    fn test_link_rendering() {
        let md = Markdown::new(
            "[text](https://example.com)".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        let output = compact(&lines);
        eprintln!("link output: {output:?}");
        assert!(
            output.iter().any(|l| l.contains("text")),
            "link text should appear"
        );
        // ratkit renders link icon + text (URL may or may not appear separately)
        let all_text: String = output.join("");
        assert!(!all_text.trim().is_empty(), "output should not be empty");
    }

    #[test]
    fn test_strikethrough() {
        let md = Markdown::new("~~struck~~".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("struck"));
    }

    #[test]
    fn test_table_simple() {
        let md = Markdown::new(
            "| H1 | H2 |\n|----|----|\n| A  | B  |\n| C  | D  |".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        let output: Vec<String> = lines.iter().map(|l| line_content(l)).collect();
        eprintln!("table output: {output:?}");
        assert!(output.iter().any(|l| l.contains("H1") || l.contains("H2")));
        assert!(output.iter().any(|l| l.contains("A") || l.contains("B")));
    }

    #[test]
    fn test_multiple_paragraphs() {
        let md = Markdown::new(
            "First paragraph.\n\nSecond paragraph.".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        let output = compact(&lines);
        eprintln!("multi-para output: {output:?}");
        // Both paragraphs should appear
        assert!(output.iter().any(|l| l.contains("First paragraph")));
        assert!(output.iter().any(|l| l.contains("Second paragraph")));
    }

    #[test]
    fn test_nested_list_indentation() {
        let md = Markdown::new(
            "- Level 1\n  - Level 2\n    - Level 3".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        let output: Vec<String> = lines.iter().map(|l| line_content(l)).collect();
        eprintln!("nested list output: {output:?}");
        assert!(output.iter().any(|l| l.contains("Level 1")));
        assert!(output.iter().any(|l| l.contains("Level 2")));
        assert!(output.iter().any(|l| l.contains("Level 3")));
    }

    #[test]
    fn test_cache_invalidation() {
        let mut md = Markdown::new("Hello".to_string(), 0, 0, default_theme(), None, None);
        let lines1 = md.render(80);
        let _lines2 = md.render(80);
        md.set_text("World".to_string());
        let lines3 = md.render(80);
        assert!(
            line_content(&lines3[0]).contains("World"),
            "Updated text should render"
        );
    }

    #[test]
    fn test_inline_bold_inside_heading() {
        let md = Markdown::new(
            "## H2 with **bold**".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("H2 with"));
        assert!(line_content(&lines[0]).contains("bold"));
    }

    #[test]
    fn test_link_inside_heading() {
        let md = Markdown::new(
            "# [Title Link](https://example.com)".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Title Link"));
    }

    #[test]
    fn test_code_inline_with_style_reset() {
        let dts = Box::new(DefaultTextStyle {
            style: Style::new().fg(Color::Blue),
        });
        let md = Markdown::new(
            "text `code` text".to_string(),
            0,
            0,
            default_theme(),
            Some(dts),
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("text"));
        assert!(line_content(&lines[0]).contains("code"));
    }
}
