//! Single-line text input with cursor tracking.

use std::collections::HashMap;

/// A single-line text input field.
pub struct Input {
    buffer: String,
    cursor: usize,
    /// Large-paste storage (TS editor `pastes` map): paste id -> full text.
    /// A large paste (>10 lines or >1000 chars) is stored here and the
    /// editor shows a compact `[paste #N +N lines]` marker instead.
    pastes: HashMap<u64, String>,
    /// Next paste id (TS `pasteCounter`).
    paste_counter: u64,
}

impl Input {
    pub fn new() -> Self {
        Self { buffer: String::new(), cursor: 0, pastes: HashMap::new(), paste_counter: 0 }
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
            let char_before = self.buffer[..self.cursor].chars().next_back();
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
            return self.buffer.clone();
        }
        let mut result = String::with_capacity(self.buffer.len());
        let mut i = 0usize;
        let bytes = self.buffer.as_bytes();
        while i < bytes.len() {
            if bytes[i..].starts_with(b"[paste #") {
                if let Some((id, marker_len)) = parse_paste_marker(&self.buffer[i..]) {
                    if let Some(content) = self.pastes.get(&id) {
                        result.push_str(content);
                        i += marker_len;
                        continue;
                    }
                }
            }
            let ch = self.buffer[i..].chars().next().unwrap_or('\u{FFFD}');
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

    /// Backspace over the previous *character* (CJK-safe: steps back to a
    /// UTF-8 char boundary, never into a continuation byte). If the text
    /// before the cursor ends with a paste marker, the whole marker is
    /// removed and its paste entry is dropped (TS `handleBackspace`).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before = &self.buffer[..self.cursor];
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
            self.renumber_markers_after(target_id);
            self.buffer.replace_range(self.cursor - marker_len..self.cursor, "");
            self.cursor -= marker_len;
            return;
        }
        let prev = before
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.buffer.remove(prev);
        self.cursor = prev;
    }

    /// Renumber paste markers with id > `target_id` down by one (TS
    /// `handleBackspace` renumbers markers after a removed paste).
    fn renumber_markers_after(&mut self, target_id: u64) {
        let mut new_buffer = String::with_capacity(self.buffer.len());
        let mut i = 0usize;
        let bytes = self.buffer.as_bytes();
        while i < bytes.len() {
            if bytes[i..].starts_with(b"[paste #") {
                if let Some((id, marker_len)) = parse_paste_marker(&self.buffer[i..]) {
                    if id > target_id {
                        let marker = &self.buffer[i..i + marker_len];
                        let new_marker =
                            marker.replacen(&format!("#{id}"), &format!("#{}", id - 1), 1);
                        new_buffer.push_str(&new_marker);
                        i += marker_len;
                        continue;
                    }
                }
            }
            let ch = self.buffer[i..].chars().next().unwrap_or('\u{FFFD}');
            new_buffer.push(ch);
            i += ch.len_utf8();
        }
        self.buffer = new_buffer;
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
    pub fn clear(&mut self) { self.buffer.clear(); self.cursor = 0; self.clear_pastes(); }
    pub fn set_value(&mut self, value: &str) { self.buffer = value.to_string(); self.cursor = self.buffer.len(); self.clear_pastes(); }
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
}
