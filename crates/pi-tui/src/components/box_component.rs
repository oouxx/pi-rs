use crate::tui::Component;

/// A container component that applies padding and optional background color
/// around its child components.
pub struct BoxComponent {
    padding_x: u16,
    padding_y: u16,
    bg_color: Option<String>, // ANSI background color escape
    children: Vec<Box<dyn Component>>,
}

impl BoxComponent {
    pub fn new() -> Self {
        Self {
            padding_x: 1,
            padding_y: 0,
            bg_color: None,
            children: Vec::new(),
        }
    }

    pub fn with_padding(mut self, padding_x: u16, padding_y: u16) -> Self {
        self.padding_x = padding_x;
        self.padding_y = padding_y;
        self
    }

    pub fn with_bg(mut self, ansi_bg: impl Into<String>) -> Self {
        self.bg_color = Some(ansi_bg.into());
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
    fn render(&self, width: u16) -> Vec<String> {
        let inner_width = width.saturating_sub(self.padding_x * 2).max(1);

        // Render children
        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.render(inner_width));
        }

        if lines.is_empty() {
            lines.push(String::new());
        }

        // Apply horizontal padding
        let pad = " ".repeat(self.padding_x as usize);
        for line in &mut lines {
            *line = format!("{}{}{}", pad, line, pad);
        }

        // Apply background
        if let Some(ref bg) = self.bg_color {
            let reset = "\x1b[0m";
            for line in &mut lines {
                *line = format!("{}{}{}", bg, line, reset);
            }
        }

        // Vertical padding
        let empty = if let Some(ref bg) = self.bg_color {
            let reset = "\x1b[0m";
            let padded = " ".repeat(width as usize);
            format!("{}{}{}", bg, padded, reset)
        } else {
            " ".repeat(width as usize)
        };

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

    fn handle_input(&mut self, data: &str) {
        for child in &mut self.children {
            child.handle_input(data);
        }
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}
