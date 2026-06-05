use ratatui::text::Line;

use crate::tui::Component;

pub struct Spacer {
    lines: usize,
}

impl Spacer {
    pub fn new(lines: usize) -> Self {
        Self { lines }
    }

    pub fn set_lines(&mut self, lines: usize) {
        self.lines = lines;
    }
}

impl Component for Spacer {
    fn render(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(vec![]); self.lines]
    }
}
