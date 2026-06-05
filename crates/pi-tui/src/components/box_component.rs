use crossterm::event::KeyEvent;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::Component;

pub struct BoxComponent {
    padding_x: u16,
    padding_y: u16,
    bg_style: Option<Style>,
    children: Vec<Box<dyn Component>>,
}

impl BoxComponent {
    pub fn new() -> Self {
        Self {
            padding_x: 1,
            padding_y: 0,
            bg_style: None,
            children: Vec::new(),
        }
    }

    pub fn with_padding(mut self, padding_x: u16, padding_y: u16) -> Self {
        self.padding_x = padding_x;
        self.padding_y = padding_y;
        self
    }

    pub fn with_bg(mut self, style: Style) -> Self {
        self.bg_style = Some(style);
        self
    }

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.children.push(component);
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }
}

impl Component for BoxComponent {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let inner_width = width.saturating_sub(self.padding_x * 2).max(1);

        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.render(inner_width));
        }

        if lines.is_empty() {
            lines.push(Line::from(vec![]));
        }

        let pad = " ".repeat(self.padding_x as usize);
        for line in &mut lines {
            let pad_span = Span::raw(pad.clone());
            let mut new_spans = vec![pad_span.clone()];
            new_spans.extend(line.spans.drain(..));
            new_spans.push(pad_span);
            line.spans = new_spans;
        }

        if let Some(bg) = self.bg_style {
            for line in &mut lines {
                for span in &mut line.spans {
                    span.style = span.style.patch(bg);
                }
            }
        }

        let empty_style = self.bg_style.unwrap_or_default();
        let empty = Line::from(Span::styled(" ".repeat(width as usize), empty_style));

        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(empty.clone());
        }
        result.append(&mut lines);
        for _ in 0..self.padding_y {
            result.push(empty.clone());
        }

        result
    }

    fn handle_input(&mut self, event: &KeyEvent) {
        for child in &mut self.children {
            child.handle_input(event);
        }
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}
