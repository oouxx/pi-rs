//! Simple static text display.

/// A simple text component that holds static text.
pub struct TextComponent {
    text: String,
}

impl TextComponent {
    pub fn new(text: &str) -> Self {
        Self { text: text.to_string() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}
