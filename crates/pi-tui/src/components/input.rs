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

    /// Backspace over the previous *character* (CJK-safe: steps back to a
    /// UTF-8 char boundary, never into a continuation byte).
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.buffer[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.buffer.remove(prev);
            self.cursor = prev;
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    /// Move left by one character (char-boundary safe).
    pub fn move_left(&mut self) {
        self.cursor = self.buffer[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    /// Move right by one character (char-boundary safe; previously this did
    /// nothing until the end of the buffer).
    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            if let Some(c) = self.buffer[self.cursor..].chars().next() {
                self.cursor += c.len_utf8();
            }
        }
    }
    pub fn move_home(&mut self) { self.cursor = 0; }
    pub fn move_end(&mut self) { self.cursor = self.buffer.len(); }
    pub fn clear(&mut self) { self.buffer.clear(); self.cursor = 0; }
    pub fn set_value(&mut self, value: &str) { self.buffer = value.to_string(); self.cursor = self.buffer.len(); }
}

impl Default for Input {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_removes_whole_cjk_glyph() {
        let mut input = Input::new();
        input.insert_str("漢字ab");
        input.move_home();
        // Move right over 漢 and 字 (3 bytes each).
        input.move_right();
        input.move_right();
        assert_eq!(input.cursor_pos(), 6, "cursor after 字");
        // Backspace over 字 (3 bytes) — must not panic or leave garbage.
        input.backspace();
        assert_eq!(input.value(), "漢ab", "whole glyph removed");
        assert_eq!(input.cursor_pos(), 3, "cursor at 漢 boundary");
        // Backspace over 漢.
        input.backspace();
        assert_eq!(input.value(), "ab");
        assert_eq!(input.cursor_pos(), 0);
    }

    #[test]
    fn move_left_right_step_over_cjk_glyphs() {
        let mut input = Input::new();
        input.insert_str("漢字x");
        input.move_home();
        input.move_right(); // over 漢 (3 bytes)
        assert_eq!(input.cursor_pos(), 3, "cursor after first glyph");
        input.move_right(); // over 字
        assert_eq!(input.cursor_pos(), 6);
        input.move_right(); // over x
        assert_eq!(input.cursor_pos(), 7);
        input.move_left(); // back over x
        assert_eq!(input.cursor_pos(), 6);
        input.move_left(); // back over 字
        assert_eq!(input.cursor_pos(), 3);
    }

    #[test]
    fn delete_at_cjk_boundary_is_safe() {
        let mut input = Input::new();
        input.insert_str("a漢b");
        input.move_left(); // now after 漢
        input.delete(); // remove b? no: cursor after 漢, delete removes b
        assert_eq!(input.value(), "a漢");
        input.delete(); // now cursor == len, no-op
        assert_eq!(input.value(), "a漢");
    }
}
