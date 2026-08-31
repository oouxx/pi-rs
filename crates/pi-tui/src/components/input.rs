//! Single-line text input with cursor tracking.
//!
//! All editing (grapheme/word movement, deletions, readline chords) is
//! delegated to the vendored editor kernel: `xai_ratatui_textarea::EditBuffer`
//! and `classify_key_event`. This component only adds the TS-compatible
//! paste-marker handling on top.

use std::collections::HashMap;

use xai_ratatui_textarea::{EditBuffer, EditCommand, classify_key_event};

/// A single-line text input field.
pub struct Input {
    /// Line text + cursor. Edited through the vendored [`EditBuffer`] so
    /// every movement/deletion shares its grapheme and word-boundary
    /// semantics (same kernel as the multi-line editor).
    buf: EditBuffer,
    /// Large-paste storage (TS editor `pastes` map): paste id -> full text.
    /// A large paste (>10 lines or >1000 chars) is stored here and the
    /// editor shows a compact `[paste #N +N lines]` marker instead.
    pastes: HashMap<u64, String>,
    /// Next paste id (TS `pasteCounter`).
    paste_counter: u64,
}

impl Input {
    pub fn new() -> Self {
        Self { buf: EditBuffer::new(), pastes: HashMap::new(), paste_counter: 0 }
    }

    pub fn value(&self) -> &str { self.buf.text() }
    pub fn cursor_pos(&self) -> usize { self.buf.cursor_byte() }

    /// Cursor position in display columns (CJK = 2, ASCII = 1).
    pub fn cursor_display_col(&self) -> u16 {
        let width = unicode_width::UnicodeWidthStr::width(&self.buf[..self.buf.cursor_byte()]);
        width as u16
    }

    /// Route a Ctrl/Alt-modified chord through the vendored readline keymap
    /// (`classify_key_event`): Ctrl+A/E line start/end, Ctrl+B/F grapheme
    /// moves, Alt+B/F word moves, Ctrl+W/U/K/H/D deletions,
    /// Alt+Backspace/Alt+D word deletions, Ctrl/Alt+Arrow word moves — all
    /// classified and executed by the vendored editor kernel, nothing
    /// implemented here. Unmapped chords are consumed silently so modified
    /// letters are never inserted literally (e.g. Ctrl+F used to type "f").
    ///
    /// Returns `true` when the key was consumed.
    pub fn handle_readline_key(&mut self, key: &crossterm::event::KeyEvent) -> bool {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return true; // Enhanced-protocol release events: consume silently.
        }
        match classify_key_event(key) {
            Some(command) => {
                let _ = self.buf.apply(command);
                true
            }
            // Chords with no readline meaning (Ctrl+G, Ctrl+O, …): swallowed
            // by the caller — never fall through to literal insertion.
            None => true,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let mut encoded = [0u8; 4];
        let _ = self.buf.insert_str(c.encode_utf8(&mut encoded));
    }

    pub fn insert_str(&mut self, s: &str) {
        let _ = self.buf.insert_str(s);
    }

    /// TS `handlePaste`: normalize line endings + expand tabs, filter
    /// non-printable chars, prepend a space when pasting a file path after a
    /// word char, and collapse large pastes (>10 lines or >1000 chars) into
    /// a `[paste #N +N lines]` / `[paste #N N chars]` marker stored in
    /// [`Self::pastes`].
    pub fn handle_paste(&mut self, text: &str) {
        // 1. Normalize line endings + expand tabs (TS `normalizeText`).
        let mut normalized = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");
        // 2. Filter out non-printable chars except newline (TS `charCode >= 32`).
        normalized = normalized
            .chars()
            .filter(|c| *c == '\n' || (*c as u32) >= 32)
            .collect();
        // 3. File-path space prepending (TS: starts with / ~ . and the char
        //    before the cursor is a word char).
        if normalized.starts_with('/') || normalized.starts_with('~') || normalized.starts_with('.') {
            let char_before = self.buf[..self.buf.cursor_byte()].chars().next_back();
            if let Some(c) = char_before {
                if c.is_ascii_alphanumeric() || c == '_' {
                    normalized = format!(" {normalized}");
                }
            }
        }
        // 4. Large-paste marker (TS: >10 lines or >1000 chars).
        let pasted_lines = normalized.split('\n').count();
        let total_chars = normalized.chars().count();
        if pasted_lines > 10 || total_chars > 1000 {
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.insert(paste_id, normalized);
            let marker = if pasted_lines > 10 {
                format!("[paste #{paste_id} +{pasted_lines} lines]")
            } else {
                format!("[paste #{paste_id} {total_chars} chars]")
            };
            self.insert_str(&marker);
            return;
        }
        self.insert_str(&normalized);
    }

    /// TS `getExpandedText`: the buffer with paste markers expanded back to
    /// their stored full text. Used when submitting the input.
    pub fn expanded_value(&self) -> String {
        if self.pastes.is_empty() {
            return self.buf.text().to_string();
        }
        let mut result = String::with_capacity(self.buf.text().len());
        let mut i = 0usize;
        let bytes = self.buf.text().as_bytes();
        while i < bytes.len() {
            if bytes[i..].starts_with(b"[paste #") {
                if let Some((id, marker_len)) = parse_paste_marker(&self.buf.text()[i..]) {
                    if let Some(content) = self.pastes.get(&id) {
                        result.push_str(content);
                        i += marker_len;
                        continue;
                    }
                }
            }
            let ch = self.buf.text()[i..].chars().next().unwrap_or('\u{FFFD}');
            result.push(ch);
            i += ch.len_utf8();
        }
        result
    }

    /// Clear the paste registry (TS `submitValue` / `setText` clear it).
    pub fn clear_pastes(&mut self) {
        self.pastes.clear();
        self.paste_counter = 0;
    }

    /// Backspace over the previous *grapheme* (vendored kernel semantics,
    /// CJK/emoji-cluster safe). If the text before the cursor ends with a
    /// paste marker, the whole marker is deleted along with its stored paste
    /// (TS `handleBackspace`).
    pub fn backspace(&mut self) {
        let cursor = self.buf.cursor_byte();
        if cursor == 0 {
            return;
        }
        let before = &self.buf[..cursor];
        if let Some((target_id, marker_len)) = marker_before_cursor(before) {
            // Drop the paste, decrement the counter, shift higher ids down
            // and renumber markers (TS `handleBackspace`).
            self.pastes.remove(&target_id);
            self.paste_counter = self.paste_counter.saturating_sub(1);
            let mut higher: Vec<u64> =
                self.pastes.keys().copied().filter(|id| *id > target_id).collect();
            higher.sort_unstable();
            for id in higher {
                if let Some(content) = self.pastes.remove(&id) {
                    self.pastes.insert(id - 1, content);
                }
            }
            let old_text = self.buf.text().to_string();
            let mut new_text = old_text.clone();
            new_text.replace_range(cursor - marker_len..cursor, "");
            let renumbered = renumber_markers_after(&new_text, target_id);
            self.buf = EditBuffer::from_parts(renumbered, cursor - marker_len);
            return;
        }
        let _ = self.buf.apply(EditCommand::DeleteGraphemeBackward);
    }

    pub fn delete(&mut self) {
        let _ = self.buf.apply(EditCommand::DeleteGraphemeForward);
    }

    /// Move left by one grapheme (vendored kernel: CJK/emoji-cluster safe).
    pub fn move_left(&mut self) {
        let _ = self.buf.apply(EditCommand::MoveGraphemeLeft);
    }

    /// Move right by one grapheme (vendored kernel).
    pub fn move_right(&mut self) {
        let _ = self.buf.apply(EditCommand::MoveGraphemeRight);
    }
    pub fn move_home(&mut self) { let _ = self.buf.set_cursor_byte(0); }
    pub fn move_end(&mut self) {
        let end = self.buf.text().len();
        let _ = self.buf.set_cursor_byte(end);
    }
    pub fn clear(&mut self) { self.buf = EditBuffer::new(); self.clear_pastes(); }
    pub fn set_value(&mut self, value: &str) { self.buf = EditBuffer::from_text(value); self.clear_pastes(); }
}

/// Renumber paste markers with id > `target_id` down by one (TS
/// `handleBackspace` renumbers markers after a removed paste).
fn renumber_markers_after(text: &str, target_id: u64) -> String {
    let mut new_buffer = String::with_capacity(text.len());
    let mut i = 0usize;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i..].starts_with(b"[paste #") {
            if let Some((id, marker_len)) = parse_paste_marker(&text[i..]) {
                if id > target_id {
                    let marker = &text[i..i + marker_len];
                    let new_marker = marker.replacen(&format!("#{id}"), &format!("#{}", id - 1), 1);
                    new_buffer.push_str(&new_marker);
                    i += marker_len;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap_or('\u{FFFD}');
        new_buffer.push(ch);
        i += ch.len_utf8();
    }
    new_buffer
}

/// Parse a paste marker at the start of `s`. Returns `(paste_id, byte_len)`.
/// Marker forms: `[paste #N +N lines]` or `[paste #N N chars]`.
fn parse_paste_marker(s: &str) -> Option<(u64, usize)> {
    let rest = s.strip_prefix("[paste #")?;
    let id_end = rest.find(|c: char| !c.is_ascii_digit())?;
    let id: u64 = rest[..id_end].parse().ok()?;
    let suffix = &rest[id_end..];
    let valid = (suffix.starts_with(" +")
        && suffix.ends_with(" lines]"))
        || (suffix.starts_with(' ') && suffix.ends_with(" chars]"));
    if !valid {
        return None;
    }
    let marker_len = "[paste #".len() + id_end + suffix.len();
    Some((id, marker_len))
}

/// If `before` ends with a paste marker, return `(paste_id, marker_byte_len)`.
fn marker_before_cursor(before: &str) -> Option<(u64, usize)> {
    let start = before.rfind("[paste #")?;
    let tail = &before[start..];
    parse_paste_marker(tail)
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

    /// TS `normalizeText`: \r\n and \r -> \n, \t -> 4 spaces.
    #[test]
    fn handle_paste_normalizes_line_endings_and_tabs() {
        let mut input = Input::new();
        input.handle_paste("a\r\nb\rc\td");
        assert_eq!(input.value(), "a\nb\nc    d");
    }

    /// TS `handlePaste`: large pastes (>10 lines or >1000 chars) are
    /// collapsed into a marker, not inserted verbatim.
    #[test]
    fn handle_paste_collapses_large_paste_into_marker() {
        let mut input = Input::new();
        let big = (0..11).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        input.handle_paste(&big);
        assert_eq!(input.value(), "[paste #1 +11 lines]");
        assert_eq!(input.expanded_value(), big);
    }

    /// TS `handlePaste`: >1000 chars but <=10 lines uses the chars marker.
    #[test]
    fn handle_paste_collapses_large_char_paste() {
        let mut input = Input::new();
        let big = "x".repeat(1001);
        input.handle_paste(&big);
        assert_eq!(input.value(), "[paste #1 1001 chars]");
        assert_eq!(input.expanded_value(), big);
    }

    /// TS `handlePaste`: small pastes are inserted verbatim (normalized).
    #[test]
    fn handle_paste_small_paste_inserted_verbatim() {
        let mut input = Input::new();
        input.handle_paste("hello world");
        assert_eq!(input.value(), "hello world");
        assert_eq!(input.expanded_value(), "hello world");
    }

    /// TS `handlePaste`: pasting a file path after a word char prepends a space.
    #[test]
    fn handle_paste_prepends_space_for_path_after_word() {
        let mut input = Input::new();
        input.insert_str("cat");
        input.handle_paste("/tmp/file.txt");
        assert_eq!(input.value(), "cat /tmp/file.txt");
    }

    /// TS `handlePaste`: no space when the char before the cursor is not a
    /// word char.
    #[test]
    fn handle_paste_no_space_for_path_after_space() {
        let mut input = Input::new();
        input.insert_str("cat ");
        input.handle_paste("/tmp/file.txt");
        assert_eq!(input.value(), "cat /tmp/file.txt");
    }

    /// TS `handleBackspace`: backspacing over a paste marker removes the
    /// whole marker and its stored paste.
    #[test]
    fn backspace_over_marker_removes_whole_marker() {
        let mut input = Input::new();
        let big = (0..11).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        input.handle_paste(&big);
        assert_eq!(input.value(), "[paste #1 +11 lines]");
        input.backspace();
        assert_eq!(input.value(), "");
        assert_eq!(input.expanded_value(), "");
    }

    /// TS `handleBackspace`: removing a lower-id marker renumbers higher-id
    /// markers down by one.
    #[test]
    fn backspace_over_marker_renumbers_higher_ids() {
        let mut input = Input::new();
        let big1 = (0..11).map(|i| format!("a{i}")).collect::<Vec<_>>().join("\n");
        let big2 = (0..11).map(|i| format!("b{i}")).collect::<Vec<_>>().join("\n");
        input.handle_paste(&big1);
        input.handle_paste(&big2);
        assert_eq!(input.value(), "[paste #1 +11 lines][paste #2 +11 lines]");
        // Move cursor to the start of the second marker, then backspace over
        // the first marker.
        input.move_home();
        input.move_right(); // over '['
        input.move_right(); // over 'p'
        input.move_right(); // over 'a'
        input.move_right(); // over 's'
        input.move_right(); // over 't'
        input.move_right(); // over 'e'
        input.move_right(); // over ' '
        input.move_right(); // over '#'
        input.move_right(); // over '1'
        input.move_right(); // over ' '
        input.move_right(); // over '+'
        input.move_right(); // over '1'
        input.move_right(); // over '1'
        input.move_right(); // over ' '
        input.move_right(); // over 'l'
        input.move_right(); // over 'i'
        input.move_right(); // over 'n'
        input.move_right(); // over 'e'
        input.move_right(); // over 's'
        input.move_right(); // over ']'
        // Cursor is now right after the first marker. Backspace removes it.
        input.backspace();
        assert_eq!(input.value(), "[paste #1 +11 lines]", "second marker renumbered to #1");
        assert_eq!(input.expanded_value(), big2);
    }

    /// `clear` drops the paste registry (TS `setText`/`submitValue`).
    #[test]
    fn clear_drops_paste_registry() {
        let mut input = Input::new();
        let big = (0..11).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        input.handle_paste(&big);
        assert_eq!(input.value(), "[paste #1 +11 lines]");
        input.clear();
        assert_eq!(input.value(), "");
        assert_eq!(input.expanded_value(), "");
    }

    // ============================================================================
    // Readline chords — routed through the vendored `classify_key_event`
    // ============================================================================

    mod readline {
        use super::*;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        fn ctrl(c: char) -> KeyEvent {
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
        }

        fn alt(c: char) -> KeyEvent {
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
        }

        /// Ctrl+F / Ctrl+B move the cursor; they no longer insert "f"/"b".
        #[test]
        fn ctrl_b_f_move_cursor_without_inserting_letters() {
            let mut input = Input::new();
            input.insert_str("ab");
            input.move_home();
            assert!(input.handle_readline_key(&ctrl('f')));
            assert_eq!(input.cursor_pos(), 1, "Ctrl+F moves right");
            assert!(input.handle_readline_key(&ctrl('b')));
            assert_eq!(input.cursor_pos(), 0, "Ctrl+B moves left");

            // Empty buffer + Ctrl+F: consumed as a chord, nothing inserted.
            let mut empty = Input::new();
            assert!(empty.handle_readline_key(&ctrl('f')));
            assert_eq!(empty.value(), "");
            assert_eq!(empty.cursor_pos(), 0);
        }

        /// Ctrl+A / Ctrl+E jump to line start/end (readline semantics).
        #[test]
        fn ctrl_a_e_are_home_end() {
            let mut input = Input::new();
            input.insert_str("hello");
            assert!(input.handle_readline_key(&ctrl('a')));
            assert_eq!(input.cursor_pos(), 0);
            assert!(input.handle_readline_key(&ctrl('e')));
            assert_eq!(input.cursor_pos(), input.value().len());
        }

        /// Ctrl+K deletes to end of line, Ctrl+U deletes to line start,
        /// Ctrl+W deletes the previous word (whitespace-delimited).
        #[test]
        fn ctrl_k_u_delete_from_cursor() {
            let mut input = Input::new();
            input.insert_str("hello world");
            input.move_left();
            assert!(input.handle_readline_key(&ctrl('k')));
            assert_eq!(input.value(), "hello worl");
            assert!(input.handle_readline_key(&ctrl('u')));
            assert_eq!(input.value(), "");

            let mut input = Input::new();
            input.insert_str("one two three");
            input.move_end();
            assert!(input.handle_readline_key(&ctrl('w')));
            assert_eq!(input.value(), "one two ", "Ctrl+W kills 'three'");
        }

        /// Ctrl+H / Ctrl+D: single-grapheme backward/forward delete.
        #[test]
        fn ctrl_h_d_delete_single_grapheme() {
            let mut input = Input::new();
            input.insert_str("abc");
            input.move_end();
            assert!(input.handle_readline_key(&ctrl('h')));
            assert_eq!(input.value(), "ab");
            input.move_home();
            assert!(input.handle_readline_key(&ctrl('d')));
            assert_eq!(input.value(), "b");
            assert_eq!(input.cursor_pos(), 0);
        }

        /// Alt+B / Alt+F move by words (readline small-word semantics).
        #[test]
        fn alt_b_f_move_by_words() {
            let mut input = Input::new();
            input.insert_str("one two");
            assert!(input.handle_readline_key(&alt('b')));
            assert_eq!(input.cursor_pos(), 4, "Alt+B back to 'two' start");
            assert!(input.handle_readline_key(&alt('b')));
            assert_eq!(input.cursor_pos(), 0);
            assert!(input.handle_readline_key(&alt('f')));
            assert_eq!(input.cursor_pos(), 3, "Alt+F over 'one'");
            assert!(input.handle_readline_key(&alt('f')));
            assert_eq!(input.cursor_pos(), 7, "Alt+F to end");
        }

        /// Unmapped chords (Ctrl+J etc.) are swallowed, never inserted.
        #[test]
        fn unmapped_chords_are_swallowed() {
            let mut input = Input::new();
            assert!(input.handle_readline_key(&ctrl('j')));
            assert_eq!(input.value(), "", "Ctrl+J must not insert 'j'");
            assert!(input.handle_readline_key(&ctrl('g')));
            assert_eq!(input.value(), "", "Ctrl+G must not insert 'g'");
        }

        /// Ctrl+Arrow word moves (terminal enhanced-protocol encodings go
        /// through the same vendored keymap). Emacs/readline semantics: M-f
        /// lands after the word, M-b at its start.
        #[test]
        fn ctrl_arrow_moves_by_words() {
            let mut input = Input::new();
            input.insert_str("one two");
            let left = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
            let right = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
            assert!(input.handle_readline_key(&left));
            assert_eq!(input.cursor_pos(), 4);
            assert!(input.handle_readline_key(&left));
            assert_eq!(input.cursor_pos(), 0);
            assert!(input.handle_readline_key(&right));
            assert_eq!(input.cursor_pos(), 3, "end of 'one'");
            assert!(input.handle_readline_key(&right));
            assert_eq!(input.cursor_pos(), 7, "end of buffer");
        }
    }
}