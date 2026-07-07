use ratatui::text::{Line, Span};

use crate::tui::Component;
use crate::utils::{truncate_to_width, visible_width};

pub struct TruncatedText {
    text: String,
    padding_x: u16,
    padding_y: u16,
}

impl TruncatedText {
    pub fn new(text: impl Into<String>, padding_x: u16, padding_y: u16) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

impl Component for TruncatedText {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let mut result = Vec::new();

        for _ in 0..self.padding_y {
            result.push(Line::from(Span::raw(" ".repeat(width as usize))));
        }

        let available = (width as usize)
            .saturating_sub(self.padding_x as usize * 2)
            .max(1);
        let single_line = self.text.split('\n').next().unwrap_or(&self.text);
        let display = truncate_to_width(single_line, available);
        let pad = " ".repeat(self.padding_x as usize);
        let mut line_text = format!("{}{}{}", pad, display, pad);
        let lw = visible_width(&line_text);
        if lw < width as usize {
            line_text.push_str(&" ".repeat(width as usize - lw));
        }
        result.push(Line::from(Span::raw(line_text)));

        for _ in 0..self.padding_y {
            result.push(Line::from(Span::raw(" ".repeat(width as usize))));
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncated_text_short() {
        let tt = TruncatedText::new("hello", 0, 0);
        let lines = tt.render(80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("hello"));
    }

    #[test]
    fn test_truncated_text_truncated() {
        let tt = TruncatedText::new("hello world this is long", 0, 0);
        let lines = tt.render(10);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().ends_with("..."));
    }

    #[test]
    fn test_truncated_text_padding() {
        let tt = TruncatedText::new("hi", 1, 1);
        let lines = tt.render(80);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].to_string().contains("hi"));
    }

    #[test]
    fn test_truncated_text_multiline_input() {
        let tt = TruncatedText::new("first line\nsecond line", 0, 0);
        let lines = tt.render(80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("first line"));
        assert!(!lines[0].to_string().contains("second line"));
    }

    #[test]
    fn test_truncated_text_empty() {
        let tt = TruncatedText::new("", 0, 0);
        let lines = tt.render(80);
        assert_eq!(lines.len(), 1);
    }
}
