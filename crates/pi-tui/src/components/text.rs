use ratatui::text::{Line, Span};

use crate::tui::Component;
use crate::utils::visible_width;

fn wrap_text_plain(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut line_width = 0;

    for ch in text.chars() {
        if ch == '\n' {
            lines.push(current_line);
            current_line = String::new();
            line_width = 0;
            continue;
        }

        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

        if line_width + ch_width > width {
            if !current_line.is_empty() {
                lines.push(current_line);
            }
            current_line = String::new();
            line_width = 0;
        }

        current_line.push(ch);
        line_width += ch_width;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

pub struct TextComponent {
    text: String,
    padding_x: u16,
    padding_y: u16,
    cached_output: Option<Vec<Line<'static>>>,
    cached_width: Option<u16>,
}

impl TextComponent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            padding_x: 0,
            padding_y: 0,
            cached_output: None,
            cached_width: None,
        }
    }

    pub fn with_padding(mut self, padding_x: u16, padding_y: u16) -> Self {
        self.padding_x = padding_x;
        self.padding_y = padding_y;
        self
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate();
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Component for TextComponent {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        if let (Some(ref cached), Some(cached_w)) = (&self.cached_output, self.cached_width) {
            if cached_w == width {
                return cached.clone();
            }
        }

        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let wrapped = wrap_text_plain(&self.text, content_width as usize);

        let pad = " ".repeat(self.padding_x as usize);
        let mut lines: Vec<Line<'static>> = wrapped
            .into_iter()
            .map(|line| {
                let text = format!("{}{}{}", pad, line, pad);
                Line::from(Span::raw(text))
            })
            .collect();

        if lines.is_empty() {
            lines.push(Line::from(vec![]));
        }

        let mut result = Vec::new();
        let empty_line = Line::from(Span::raw(" ".repeat(width as usize)));
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }
        result.append(&mut lines);
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        result
    }

    fn invalidate(&mut self) {
        self.cached_output = None;
        self.cached_width = None;
    }
}
