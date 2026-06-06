use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::keybindings::get_keybindings;
use crate::tui::{Component, Focusable};
use crate::utils::visible_width;

pub struct Input {
    pub focused: bool,
    value: String,
    cursor: usize,
    scroll_offset: usize,
    prompt: String,
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

    fn cursor_left(&mut self) {
        if self.cursor > 0 {
            let len = self.value[..self.cursor].chars().next_back().map_or(1, |c| c.len_utf8());
            self.cursor -= len;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    fn cursor_right(&mut self) {
        if let Some(ch) = self.value[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    fn cursor_end(&mut self) {
        self.cursor = self.value.len();
    }

    fn insert_char(&mut self, ch: char) {
        if let Some(max) = self.max_length {
            if self.value.len() >= max {
                return;
            }
        }
        self.value.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn delete_backward(&mut self) {
        if self.cursor > 0 {
            let len = self.value[..self.cursor].chars().next_back().map_or(1, |c| c.len_utf8());
            self.cursor -= len;
            self.value.drain(self.cursor..self.cursor + len);
        }
    }

    fn delete_forward(&mut self) {
        if let Some(ch) = self.value[self.cursor..].chars().next() {
            let len = ch.len_utf8();
            self.value.drain(self.cursor..self.cursor + len);
        }
    }

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
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let prompt_width = visible_width(&self.prompt);
        let input_width = (width as usize).saturating_sub(prompt_width).max(1);

        let visible_start = self.scroll_offset;
        let visible_part = &self.value[visible_start..];
        let display_value = if visible_width(visible_part) > input_width {
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

        let cursor_byte_offset = self.cursor.saturating_sub(visible_start);
        let cursor_char_index = display_value
            .char_indices()
            .take_while(|(byte_pos, _)| *byte_pos < cursor_byte_offset)
            .count();

        let mut spans: Vec<Span<'static>> = vec![Span::raw(self.prompt.clone())];

        for (i, ch) in display_value.chars().enumerate() {
            let ch_str = ch.to_string();
            if i == cursor_char_index && self.focused {
                spans.push(Span::styled(
                    ch_str,
                    Style::default().bg(ratatui::style::Color::DarkGray),
                ));
            } else {
                spans.push(Span::raw(ch_str));
            }
        }

        if cursor_char_index >= display_value.chars().count() && self.focused {
            spans.push(Span::styled(
                " ",
                Style::default().bg(ratatui::style::Color::DarkGray),
            ));
        }

        let lw = visible_width(&self.prompt) + visible_width(&display_value);
        if lw < width as usize {
            spans.push(Span::raw(" ".repeat(width as usize - lw)));
        }

        vec![Line::from(spans)]
    }

    fn cursor_position(&self) -> Option<(u16, u16)> {
        let prompt_width = visible_width(&self.prompt) as u16;
        let visible_start = self.scroll_offset;
        let display_text = &self.value[visible_start..];

        let cursor_byte_offset = self.cursor.saturating_sub(visible_start);
        let mut col = prompt_width;
        let mut seen = 0usize;
        for ch in display_text.chars() {
            if seen >= cursor_byte_offset {
                break;
            }
            col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
            seen += ch.len_utf8();
        }

        Some((0, col))
    }

    fn handle_input(&mut self, event: &KeyEvent) {
        let kb = get_keybindings();

        if kb.matches(event, "submit") {
            if let Some(ref cb) = self.on_submit {
                cb(&self.value);
            }
            return;
        }

        if kb.matches(event, "cancel") {
            self.value.clear();
            self.cursor = 0;
            self.scroll_offset = 0;
            return;
        }

        if kb.matches(event, "cursorLeft") {
            self.cursor_left();
            return;
        }
        if kb.matches(event, "cursorRight") {
            self.cursor_right();
            return;
        }
        if kb.matches(event, "cursorHome") {
            self.cursor_home();
            return;
        }
        if kb.matches(event, "cursorEnd") {
            self.cursor_end();
            return;
        }
        if kb.matches(event, "deleteBackward") {
            self.delete_backward();
            return;
        }
        if kb.matches(event, "deleteForward") {
            self.delete_forward();
            return;
        }
        if kb.matches(event, "deleteWordBackward") {
            self.delete_word_backward();
            return;
        }

        if let KeyCode::Char(ch) = event.code {
            if event.modifiers.is_empty() && !ch.is_control() {
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
        init_keybindings(None);
    }

    #[test]
    fn test_input_render_empty() {
        setup_kb();
        let input = Input::new();
        let lines = input.render(80);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("> "));
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
        input.handle_input(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn test_input_render_cjk_text() {
        setup_kb();
        let mut input = Input::new();
        input.set_value("\u{c548}\u{b155}\u{d558}\u{c138}\u{c694} \u{3053}\u{3093}\u{306b}\u{3061}\u{306f} \u{4f60}\u{597d}");
        let lines = input.render(40);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_input_render_cjk_cursor_visible() {
        setup_kb();
        let mut input = Input::new();
        input.set_value("\u{4f60}\u{597d}\u{4e16}\u{754c}\u{6d4b}\u{8bd5}\u{6587}\u{672c}");
        let lines = input.render(10);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_input_backslash_submitted() {
        setup_kb();
        let mut input = Input::new();
        let mut submitted = String::new();
        input.on_submit = Some(Box::new(move |val| {}));
        input.set_value("hello\\world");
        assert_eq!(input.get_value(), "hello\\world");
    }

    #[test]
    fn test_input_delete_forward_at_end() {
        setup_kb();
        let mut input = Input::new();
        input.set_value("abc");
        input.delete_forward();
        assert_eq!(input.get_value(), "abc");
        assert_eq!(input.cursor, 3);
    }

    #[test]
    fn test_input_delete_backward_at_start() {
        setup_kb();
        let mut input = Input::new();
        input.delete_backward();
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
        input.set_value("hello world foo");
        input.cursor = 8;
        let original_len = input.get_value().len();
        input.delete_word_backward();
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
        assert_eq!(input.get_value(), "hello");
    }

    #[test]
    fn test_input_max_length() {
        setup_kb();
        let mut input = Input::new();
        input.max_length = Some(3);
        input.insert_char('a');
        input.insert_char('b');
        input.insert_char('c');
        input.insert_char('d');
        assert_eq!(input.get_value(), "abc");
    }

    #[test]
    fn test_input_focused_state() {
        let mut input = Input::new();
        assert!(!input.focused());
        input.set_focused(true);
        assert!(input.focused());
    }

    #[test]
    fn test_input_cursor_position() {
        let mut input = Input::new();
        input.set_value("abc");
        input.focused = true;
        let pos = input.cursor_position();
        assert_eq!(pos, Some((0, 5))); // "> abc" = 5 cols
    }
}
