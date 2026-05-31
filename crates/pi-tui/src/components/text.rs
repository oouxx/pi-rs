use crate::tui::Component;
use crate::utils::wrap_text_with_ansi;

/// A plain text component with ANSI-aware word wrapping.
pub struct TextComponent {
    text: String,
    padding_x: u16,
    padding_y: u16,
    cached_output: Option<Vec<String>>,
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
    fn render(&self, width: u16) -> Vec<String> {
        // Check cache
        if let (Some(ref cached), Some(cached_w)) = (&self.cached_output, self.cached_width) {
            if cached_w == width {
                return cached.clone();
            }
        }

        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let mut lines = wrap_text_with_ansi(&self.text, content_width as usize);

        if lines.is_empty() {
            lines.push(String::new());
        }

        // Apply horizontal padding
        let pad = " ".repeat(self.padding_x as usize);
        for line in &mut lines {
            let full = format!("{}{}{}", pad, line, pad);
            *line = full;
        }

        // Apply vertical padding
        let mut result = Vec::new();
        let empty_line = " ".repeat(width as usize);
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }
        result.append(&mut lines);
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        // We can't update the cache here because we're in an immutable reference
        // The cache just won't be used
        result
    }

    fn invalidate(&mut self) {
        self.cached_output = None;
        self.cached_width = None;
    }
}
