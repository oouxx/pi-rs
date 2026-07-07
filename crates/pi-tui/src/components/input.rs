//! Single-line text input with cursor tracking.

use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;

/// A single-line text input field.
pub struct Input {
    buffer: String,
    cursor: usize,
}

impl Input {
    pub fn new() -> Self {
        Self { buffer: String::new(), cursor: 0 }
    }

    pub fn value(&self) -> &str { &self.buffer }
    pub fn cursor_pos(&self) -> usize { self.cursor }

    /// Cursor position in display columns (CJK = 2, ASCII = 1).
    pub fn cursor_display_col(&self) -> u16 {
        let prefix = &self.buffer[..self.cursor];
        let width = unicode_width::UnicodeWidthStr::width(prefix);
        width as u16
    }

    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8(); // advance by UTF-8 byte length
    }

    pub fn insert_str(&mut self, s: &str) {
        self.buffer.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 { self.cursor -= 1; self.buffer.remove(self.cursor); }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.buffer.len() { self.buffer.remove(self.cursor); }
    }

    pub fn move_left(&mut self) { self.cursor = self.cursor.saturating_sub(1); }
    pub fn move_right(&mut self) { self.cursor = self.cursor.min(self.buffer.len()); }
    pub fn move_home(&mut self) { self.cursor = 0; }
    pub fn move_end(&mut self) { self.cursor = self.buffer.len(); }
    pub fn clear(&mut self) { self.buffer.clear(); self.cursor = 0; }
    pub fn set_value(&mut self, value: &str) { self.buffer = value.to_string(); self.cursor = self.buffer.len(); }
}

impl Default for Input {
    fn default() -> Self { Self::new() }
}
