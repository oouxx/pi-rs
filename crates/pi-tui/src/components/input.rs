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
        // TS `segment()` only merges markers whose id is still in the registry
        // (`segmentWithMarkers(text, …, validPasteIds())`), so a marker left
        // without an entry is ordinary text and backspace deletes one grapheme.
        if let Some((target_id, marker_len)) = marker_before_cursor(before)
            .filter(|(id, _)| self.pastes.contains_key(id))
        {
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

    /// Delete forward (TS `handleForwardDelete`): the grapheme at the cursor,
    /// which the marker-aware segmentation makes a whole paste marker when its
    /// id is valid. TS leaves the registry entry in place (and does not
    /// renumber) — only backspace drops the entry.
    pub fn delete(&mut self) {
        let cursor = self.buf.cursor_byte();
        let marker = parse_paste_marker(&self.buf.text()[cursor..])
            .filter(|(id, _)| self.pastes.contains_key(id))
            .map(|(_, marker_len)| marker_len);
        if let Some(marker_len) = marker {
            let mut new_text = self.buf.text().to_string();
            new_text.replace_range(cursor..cursor + marker_len, "");
            self.buf = EditBuffer::from_parts(new_text, cursor);
            return;
        }
        let _ = self.buf.apply(EditCommand::DeleteGraphemeForward);
    }

    /// Move left by one grapheme (vendored kernel: CJK/emoji-cluster safe).
    /// A valid paste marker is one grapheme (TS `moveCursor` uses the
    /// marker-aware `segment(text, "grapheme")`).
    pub fn move_left(&mut self) {
        let cursor = self.buf.cursor_byte();
        if let Some((_, marker_len)) = marker_before_cursor(&self.buf[..cursor])
            .filter(|(id, _)| self.pastes.contains_key(id))
        {
            let _ = self.buf.set_cursor_byte(cursor - marker_len);
            return;
        }
        let _ = self.buf.apply(EditCommand::MoveGraphemeLeft);
    }

    /// Move right by one grapheme (vendored kernel); a valid paste marker is
    /// one grapheme (TS `moveCursor`).
    pub fn move_right(&mut self) {
        let cursor = self.buf.cursor_byte();
        let marker = parse_paste_marker(&self.buf.text()[cursor..])
            .filter(|(id, _)| self.pastes.contains_key(id))
            .map(|(_, marker_len)| marker_len);
        if let Some(marker_len) = marker {
            let _ = self.buf.set_cursor_byte(cursor + marker_len);
            return;
        }
        let _ = self.buf.apply(EditCommand::MoveGraphemeRight);
    }
    pub fn move_home(&mut self) { let _ = self.buf.set_cursor_byte(0); }
    pub fn move_end(&mut self) {
        let end = self.buf.text().len();
        let _ = self.buf.set_cursor_byte(end);
    }
    pub fn clear(&mut self) { self.buf = EditBuffer::new(); self.clear_pastes(); }

    /// TS `setText`: replaces the text **and** drops the paste registry, because
    /// the new text is unrelated to the old one (programmatic set, restore,
    /// extension-driven editor text).
    pub fn set_value(&mut self, value: &str) { self.buf = EditBuffer::from_text(value); self.clear_pastes(); }

    /// Replace the text while keeping the paste registry — for edits derived
    /// from the current text that still carry its paste markers (TS applies
    /// completions by mutating `state.lines` in place, so the registry
    /// survives and a later `getExpandedText` still resolves the markers).
    pub fn set_value_preserving_pastes(&mut self, value: &str) {
        self.buf = EditBuffer::from_text(value);
    }
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
///
/// Marker forms (TS `/\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]/`):
/// `[paste #N]`, `[paste #N +N lines]` or `[paste #N N chars]`. The match is
/// bounded — the marker may be followed by more text in the same line, which
/// is the common case once anything is typed after a paste.
fn parse_paste_marker(s: &str) -> Option<(u64, usize)> {
    let rest = s.strip_prefix("[paste #")?;
    let id_end = rest.find(|c: char| !c.is_ascii_digit())?;
    let id: u64 = rest[..id_end].parse().ok()?;
    let suffix = &rest[id_end..];
    // Everything after the closing `]` of this marker.
    let after_marker = if let Some(after) = suffix.strip_prefix(']') {
        after
    } else if let Some(after) = suffix.strip_prefix(" +") {
        let digits_end = after.find(|c: char| !c.is_ascii_digit())?;
        after[digits_end..].strip_prefix(" lines]")?
    } else {
        let after = suffix.strip_prefix(' ')?;
        let digits_end = after.find(|c: char| !c.is_ascii_digit())?;
        after[digits_end..].strip_prefix(" chars]")?
    };
    let marker_len = s.len() - after_marker.len();
    Some((id, marker_len))
}

/// If `before` ends with a paste marker, return `(paste_id, marker_byte_len)`.
///
/// The marker must end **exactly** at the cursor: TS asks whether the last
/// grapheme before the cursor is a marker, so a marker followed by more text is
/// not "before the cursor" even though it parses on its own.
fn marker_before_cursor(before: &str) -> Option<(u64, usize)> {
    let start = before.rfind("[paste #")?;
    let (id, marker_len) = parse_paste_marker(&before[start..])?;
    if start + marker_len != before.len() {
        return None;
    }
    Some((id, marker_len))
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

    /// 光标左右移动把有效 marker 当一个字素（TS `moveCursor` 用 marker-aware
    /// 的 `segment`），悬空 marker 则按普通文本逐字移动。
    #[test]
    fn cursor_moves_treat_a_valid_marker_as_one_grapheme() {
        let mut input = Input::new();
        let big = "x".repeat(20_000);
        input.handle_paste(&big);
        input.insert_str(" tail");

        let marker_len = "[paste #1 20000 chars]".len();
        // 行首一次右移即跨过整个 marker（不是 21 次）。
        input.move_home();
        input.move_right();
        assert_eq!(input.cursor_pos(), marker_len, "一次跨过整个 marker");
        // 再过一格是 marker 后的第一个字符。
        input.move_right();
        assert_eq!(input.cursor_pos(), marker_len + 1, "之后按字素移动");
        // 左移同理：回到 marker 末尾，再一次跨回行首。
        input.move_left();
        assert_eq!(input.cursor_pos(), marker_len);
        input.move_left();
        assert_eq!(input.cursor_pos(), 0, "一次跨回 marker 起点");

        // 悬空 marker：逐字移动。
        let mut dangling = Input::new();
        dangling.set_value("[paste #1 20000 chars]");
        dangling.move_home();
        dangling.move_right();
        assert_eq!(dangling.cursor_pos(), 1, "悬空 marker 只移动一个字素");
    }

    /// 悬空 marker（文本还在、表项已无）按普通文本处理：TS 的
    /// `segmentWithMarkers` 只合并**有效 id**，所以退格删一个字符、
    /// 前向删除也只删一个字符。
    #[test]
    fn dangling_paste_marker_is_plain_text() {
        let mut input = Input::new();
        // `set_value` 是 TS `setText` 语义：清表，但文本里的 marker 留下。
        input.set_value("[paste #1 20000 chars]");
        assert_eq!(input.expanded_value(), "[paste #1 20000 chars]");

        input.backspace();
        assert_eq!(
            input.value(),
            "[paste #1 20000 chars",
            "悬空 marker 的退格只删一个字符"
        );

        input.move_home();
        input.delete();
        assert_eq!(
            input.value(),
            "paste #1 20000 chars",
            "悬空 marker 的前向删除也只删一个字符"
        );
    }

    /// 有效 marker：退格整块删 + 删表项 + 重编号；前向删除整块删但保留表项
    /// （TS `handleBackspace` / `handleForwardDelete` 的非对称语义）。
    #[test]
    fn valid_paste_marker_is_atomic_for_both_deletions() {
        let mut input = Input::new();
        let big = "x".repeat(20_000);
        input.handle_paste(&big);
        input.insert_str(" tail");

        // 前向删除：光标移到行首（marker 之前），整块删掉 marker。
        input.move_home();
        input.delete();
        assert_eq!(input.value(), " tail", "前向删除整块移除 marker");
        assert_eq!(
            input.expanded_value(),
            " tail",
            "表项保留但 marker 文本已删，展开无影响"
        );

        // 退格：重建后整体删掉 marker 并清表项。
        let mut input = Input::new();
        input.handle_paste(&big);
        input.backspace();
        assert_eq!(input.value(), "");
        assert_eq!(input.expanded_value(), "", "退格后表项已移除");
    }

    /// marker 后面还有文本时也必须展开（TS 的 marker 正则是**有界**的：
    /// `/\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]/`）。曾经的实现用
    /// `ends_with(" chars]")` 校验**整段剩余字符串的结尾**，于是只要粘贴后
    /// 再输入任何内容，marker 就被原样当成普通文本提交。
    #[test]
    fn paste_marker_expands_when_followed_by_text() {
        let mut input = Input::new();
        let big = "x".repeat(20_000);
        input.handle_paste(&big);
        input.insert_str(" tail");
        assert_eq!(input.value(), "[paste #1 20000 chars] tail");
        assert_eq!(
            input.expanded_value(),
            format!("{big} tail"),
            "marker 后有文本时也要展开"
        );

        // 多行形式同理，且在中间（前后都有文本）。
        let mut multi = Input::new();
        let lines = (0..11).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        multi.insert_str("before ");
        multi.handle_paste(&lines);
        multi.insert_str(" after");
        assert_eq!(
            multi.expanded_value(),
            format!("before {lines} after")
        );
    }

    /// 保留粘贴表的文本替换（补全应用路径用）：marker 仍能展开。
    #[test]
    fn set_value_preserving_pastes_keeps_the_registry() {
        let mut input = Input::new();
        let big = "x".repeat(20_000);
        input.handle_paste(&big);
        let value = input.value().to_string();
        assert_eq!(value, "[paste #1 20000 chars]");
        input.set_value_preserving_pastes(&format!("{value} tail"));
        assert_eq!(
            input.expanded_value(),
            format!("{big} tail"),
            "marker 必须仍能展开"
        );
        // 对照：set_value 是 TS setText 语义，会清表（marker 变成字面文本）。
        input.set_value("[paste #1 20000 chars] tail");
        assert_eq!(input.expanded_value(), "[paste #1 20000 chars] tail");
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
        // 光标移到第一个 marker 之后：marker 是一个字素，一次右移即跨过
        // （TS `moveCursor` 用 marker-aware 的 `segment`）。
        input.move_home();
        input.move_right();
        assert_eq!(
            input.cursor_pos(),
            "[paste #1 +11 lines]".len(),
            "一次跨过第一个 marker"
        );
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