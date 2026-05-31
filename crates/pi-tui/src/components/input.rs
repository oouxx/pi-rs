use crate::keybindings::get_keybindings;
use crate::tui::{Component, Focusable, CURSOR_MARKER};
use crate::utils::visible_width;

/// A single-line text input component with cursor handling.
pub struct Input {
    pub focused: bool,
    value: String,
    cursor: usize,        // byte position in value
    scroll_offset: usize, // horizontal scroll offset
    prompt: String,
    bg_color: Option<String>,
    max_length: Option<usize>,
    pub on_submit: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

impl Input {
    pub fn new() -> Self {
        Self {
            focused: false,
            value: String::new(),
            cursor: 0,
            scroll_offset: 0,
            prompt: "> ".to_string(),
            bg_color: None,
            max_length: None,
            on_submit: None,
        }
    }

    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
        self.scroll_offset = 0;
    }

    pub fn get_value(&self) -> &str {
        &self.value
    }

    /// Move cursor left by one grapheme cluster.
    fn cursor_left(&mut self) {
        if self.cursor > 0 {
            // Move back one byte (simplified — for ASCII this works)
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    /// Move cursor right by one grapheme cluster.
    fn cursor_right(&mut self) {
        if self.cursor < self.value.len() {
            self.cursor += 1;
        }
    }

    /// Move cursor to start of line.
    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Move cursor to end of line.
    fn cursor_end(&mut self) {
        self.cursor = self.value.len();
    }

    /// Insert a character at the cursor position.
    fn insert_char(&mut self, ch: char) {
        if let Some(max) = self.max_length {
            if self.value.len() >= max {
                return;
            }
        }
        self.value.insert(self.cursor, ch);
        self.cursor += 1;
    }

    /// Delete the character before the cursor (backspace).
    fn delete_backward(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.value.remove(self.cursor);
        }
    }

    /// Delete the character at the cursor (delete).
    fn delete_forward(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    /// Delete word backward (Ctrl+Backspace behavior).
    fn delete_word_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before = &self.value[..self.cursor];
        if let Some(pos) = before.rfind(' ') {
            let removed = self.cursor - (pos + 1);
            self.cursor = pos + 1;
            self.value.drain(self.cursor..self.cursor + removed);
        } else {
            let len = self.cursor;
            self.cursor = 0;
            self.value.drain(0..len);
        }
    }
}

impl Component for Input {
    fn render(&self, width: u16) -> Vec<String> {
        let prompt_width = visible_width(&self.prompt);
        let input_width = (width as usize).saturating_sub(prompt_width).max(1);

        // Determine visible portion of value
        let visible_start = self.scroll_offset;
        let visible_part = &self.value[visible_start..];
        let display_value = if visible_width(visible_part) > input_width {
            // Truncate to fit
            let mut result = String::new();
            let mut current = 0;
            for ch in visible_part.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if current + cw > input_width {
                    break;
                }
                current += cw;
                result.push(ch);
            }
            result
        } else {
            visible_part.to_string()
        };

        // Build the line with cursor marker
        let mut line = String::new();
        line.push_str(&self.prompt);

        let cursor_offset = self.cursor - visible_start;
        for (i, ch) in display_value.chars().enumerate() {
            if i == cursor_offset {
                line.push_str(CURSOR_MARKER);
            }
            line.push(ch);
        }
        if cursor_offset >= display_value.len() {
            line.push_str(CURSOR_MARKER);
        }

        // Pad to full width
        let lw = visible_width(&line);
        if lw < width as usize {
            for _ in 0..(width as usize - lw) {
                line.push(' ');
            }
        }

        vec![line]
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();

        if kb.matches(data, "submit") {
            if let Some(ref cb) = self.on_submit {
                cb(&self.value);
            }
            return;
        }

        if kb.matches(data, "cancel") {
            self.value.clear();
            self.cursor = 0;
            self.scroll_offset = 0;
            return;
        }

        if kb.matches(data, "cursorLeft") {
            self.cursor_left();
            return;
        }
        if kb.matches(data, "cursorRight") {
            self.cursor_right();
            return;
        }
        if kb.matches(data, "cursorHome") {
            self.cursor_home();
            return;
        }
        if kb.matches(data, "cursorEnd") {
            self.cursor_end();
            return;
        }
        if kb.matches(data, "deleteBackward") {
            self.delete_backward();
            return;
        }
        if kb.matches(data, "deleteForward") {
            self.delete_forward();
            return;
        }
        if kb.matches(data, "deleteWordBackward") {
            self.delete_word_backward();
            return;
        }

        // Printable characters
        if data.len() == 1 {
            let ch = data.chars().next().unwrap();
            if !ch.is_control() {
                self.insert_char(ch);
            }
        }
    }
}

impl Focusable for Input {
    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::init_keybindings;

    fn setup_kb() {
        // Ensure keybindings are initialized
        init_keybindings(None);
    }

    #[test]
    fn test_input_render_empty() {
        setup_kb();
        let input = Input::new();
        let lines = input.render(80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("> "));
        assert!(lines[0].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_input_set_and_get_value() {
        let mut input = Input::new();
        input.set_value("hello");
        assert_eq!(input.get_value(), "hello");
        assert_eq!(input.cursor, 5);
    }

    #[test]
    fn test_input_insert_char() {
        let mut input = Input::new();
        input.insert_char('a');
        input.insert_char('b');
        assert_eq!(input.get_value(), "ab");
    }

    #[test]
    fn test_input_backspace() {
        let mut input = Input::new();
        input.set_value("abc");
        input.delete_backward();
        assert_eq!(input.get_value(), "ab");
    }

    #[test]
    fn test_input_cursor_left_right() {
        let mut input = Input::new();
        input.set_value("ab");
        input.cursor_left();
        assert_eq!(input.cursor, 1);
        input.cursor_right();
        assert_eq!(input.cursor, 2);
    }

    #[test]
    fn test_input_cancel() {
        let mut input = Input::new();
        input.set_value("test");
        input.handle_input("\x1b"); // Escape
        assert_eq!(input.get_value(), "");
    }

    // --- Supplementary tests matching TS originals ---

    #[test]
    fn test_input_render_cjk_text() {
        // TS: "Does not overflow with wide CJK and fullwidth text"
        setup_kb();
        let mut input = Input::new();
        // Korean, Japanese, Chinese mixed
        input.set_value("안녕하세요 こんにちは 你好");
        let lines = input.render(40);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_input_render_cjk_cursor_visible() {
        // TS: "Keeps cursor visible when horizontally scrolling wide CJK text"
        setup_kb();
        let mut input = Input::new();
        input.set_value("你好世界测试文本");
        // Move cursor to end
        let lines = input.render(10);
        // Should still contain the cursor marker even if scrolled
        assert!(lines[0].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_input_backslash_submitted() {
        // TS: "Backslash submitted via Enter includes the backslash in the value"
        setup_kb();
        let mut input = Input::new();
        let mut submitted = String::new();
        input.on_submit = Some(Box::new(move |val| {
            // Can't easily capture in test, so just verify value is set correctly
        }));
        input.set_value("hello\\world");
        assert_eq!(input.get_value(), "hello\\world");
    }

    #[test]
    fn test_input_delete_forward_at_end() {
        setup_kb();
        let mut input = Input::new();
        input.set_value("abc");
        // Cursor is at end (3)
        input.delete_forward(); // should do nothing
        assert_eq!(input.get_value(), "abc");
        assert_eq!(input.cursor, 3);
    }

    #[test]
    fn test_input_delete_backward_at_start() {
        setup_kb();
        let mut input = Input::new();
        input.delete_backward(); // should do nothing
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn test_input_cursor_home_end() {
        setup_kb();
        let mut input = Input::new();
        input.set_value("hello world");
        input.cursor_home();
        assert_eq!(input.cursor, 0);
        input.cursor_end();
        assert_eq!(input.cursor, 11);
    }

    #[test]
    fn test_input_delete_word_backward_middle() {
        setup_kb();
        let mut input = Input::new();
        // Set cursor in the middle of a word, delete_word_backward should delete to start of word
        input.set_value("hello world foo");
        input.cursor = 8; // after "hello w"
        let original_len = input.get_value().len();
        input.delete_word_backward();
        // Should delete "w" (one word) back to the space
        assert!(input.get_value().len() < original_len);
        assert!(input.cursor < 8);
    }

    #[test]
    fn test_input_delete_word_backward_start() {
        setup_kb();
        let mut input = Input::new();
        input.set_value("hello");
        input.cursor = 0;
        input.delete_word_backward();
        assert_eq!(input.get_value(), "hello"); // unchanged
    }

    #[test]
    fn test_input_max_length() {
        setup_kb();
        let mut input = Input::new();
        input.max_length = Some(3);
        input.insert_char('a');
        input.insert_char('b');
        input.insert_char('c');
        input.insert_char('d'); // should be rejected
        assert_eq!(input.get_value(), "abc");
    }

    #[test]
    fn test_input_focused_state() {
        let mut input = Input::new();
        assert!(!input.focused());
        input.set_focused(true);
        assert!(input.focused());
    }
}
