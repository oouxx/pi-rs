use crate::tui::Component;

/// A spacer component that outputs empty lines.
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
    fn render(&self, _width: u16) -> Vec<String> {
        vec![String::new(); self.lines]
    }
}
