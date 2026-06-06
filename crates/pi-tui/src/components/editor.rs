use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;
use std::sync::OnceLock;

use regex_lite::Regex;

use crate::autocomplete::{AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions};
use crate::components::select_list::{SelectItem, SelectList, SelectListTheme};
use crate::keybindings::get_keybindings;
use crate::kill_ring::KillRing;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::{Component, Focusable};
use crate::undo_stack::UndoStack;
use crate::utils::{is_whitespace_char, visible_width};
use crate::word_navigation::{find_word_backward, find_word_forward, WordNavigationOptions};

fn paste_marker_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[paste #(\d+)( (\+(\d+) lines|(\d+) chars))?\]").unwrap())
}

fn paste_marker_single() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\[paste #(\d+)( (\+(\d+) lines|(\d+) chars))?\]$").unwrap())
}

fn is_paste_marker(segment: &str) -> bool {
    segment.len() >= 10 && paste_marker_single().is_match(segment)
}

fn segment_with_markers<'a>(text: &'a str, valid_ids: &[u64]) -> Vec<&'a str> {
    if valid_ids.is_empty() || !text.contains("[paste #") {
        return text.graphemes(true).collect();
    }

    let markers: Vec<(usize, usize)> = paste_marker_regex()
        .find_iter(text)
        .filter_map(|m| {
            let id_str = m.as_str();
            let id_start = "[paste #".len();
            let id_end = id_str[id_start..].find(|c: char| !c.is_ascii_digit()).unwrap_or(id_str.len() - id_start);
            let id: u64 = id_str[id_start..id_start + id_end].parse().ok()?;
            if valid_ids.contains(&id) {
                Some((m.start(), m.end()))
            } else {
                None
            }
        })
        .collect();

    if markers.is_empty() {
        return text.graphemes(true).collect();
    }

    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let mut result: Vec<&str> = Vec::new();
    let mut marker_idx = 0;
    let mut i = 0;

    while i < graphemes.len() {
        let grapheme = graphemes[i];
        let byte_start = grapheme.as_ptr() as usize - text.as_ptr() as usize;

        if marker_idx < markers.len() && byte_start >= markers[marker_idx].1 {
            marker_idx += 1;
        }

        if marker_idx < markers.len() {
            let (m_start, m_end) = markers[marker_idx];
            if byte_start >= m_start && byte_start < m_end {
                if byte_start == m_start {
                    result.push(&text[m_start..m_end]);
                }
                i += 1;
                while i < graphemes.len() {
                    let next_start = graphemes[i].as_ptr() as usize - text.as_ptr() as usize;
                    if next_start >= m_end {
                        break;
                    }
                    i += 1;
                }
                marker_idx += 1;
                continue;
            }
        }
        result.push(grapheme);
        i += 1;
    }
    result
}

#[derive(Debug, Clone)]
pub struct TextChunk {
    pub text: String,
    pub start_index: usize,
    pub end_index: usize,
}

pub fn word_wrap_line(line: &str, max_width: usize, pre_segmented: Option<&[&str]>) -> Vec<TextChunk> {
    if line.is_empty() || max_width == 0 {
        return vec![TextChunk {
            text: String::new(),
            start_index: 0,
            end_index: 0,
        }];
    }

    let line_width = visible_width(line);
    if line_width <= max_width {
        return vec![TextChunk {
            text: line.to_string(),
            start_index: 0,
            end_index: line.len(),
        }];
    }

    let segments: Vec<&str> = match pre_segmented {
        Some(s) => s.to_vec(),
        None => line.graphemes(true).collect(),
    };

    let mut chunks: Vec<TextChunk> = Vec::new();
    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    let mut wrap_opp_index: Option<(usize, usize)> = None;

    for i in 0..segments.len() {
        let grapheme = segments[i];
        let g_width = visible_width(grapheme);
        let char_index = grapheme.as_ptr() as usize - line.as_ptr() as usize;
        let is_ws = !is_paste_marker(grapheme) && grapheme.chars().next().map_or(false, |c| c == ' ' || c == '\t');

        if current_width + g_width > max_width {
            if let Some((opp_idx, opp_width)) = wrap_opp_index {
                if current_width - opp_width + g_width <= max_width {
                    chunks.push(TextChunk {
                        text: line[chunk_start..opp_idx].to_string(),
                        start_index: chunk_start,
                        end_index: opp_idx,
                    });
                    chunk_start = opp_idx;
                    current_width -= opp_width;
                }
            } else if chunk_start < char_index {
                chunks.push(TextChunk {
                    text: line[chunk_start..char_index].to_string(),
                    start_index: chunk_start,
                    end_index: char_index,
                });
                chunk_start = char_index;
                current_width = 0;
            }
            wrap_opp_index = None;
        }

        if g_width > max_width {
            let sub_chunks = word_wrap_line(grapheme, max_width, None);
            for j in 0..sub_chunks.len().saturating_sub(1) {
                let sc = &sub_chunks[j];
                chunks.push(TextChunk {
                    text: sc.text.clone(),
                    start_index: char_index + sc.start_index,
                    end_index: char_index + sc.end_index,
                });
            }
            if let Some(last) = sub_chunks.last() {
                chunk_start = char_index + last.start_index;
                current_width = visible_width(&last.text);
            }
            wrap_opp_index = None;
            continue;
        }

        current_width += g_width;

        let next = segments.get(i + 1).copied();
        if is_ws && next.map_or(false, |n| is_paste_marker(n) || n.chars().next().map_or(true, |c| c != ' ' && c != '\t')) {
            let next_start = next.map(|n| n.as_ptr() as usize - line.as_ptr() as usize).unwrap_or(line.len());
            wrap_opp_index = Some((next_start, current_width));
        }
    }

    chunks.push(TextChunk {
        text: line[chunk_start..].to_string(),
        start_index: chunk_start,
        end_index: line.len(),
    });

    chunks
}

#[derive(Debug, Clone)]
struct EditorState {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,
}

#[derive(Debug, Clone)]
struct LayoutLine {
    text: String,
    has_cursor: bool,
    cursor_pos: Option<usize>,
}

pub struct EditorTheme {
    pub border_color: Box<dyn Fn() -> Style + Send + Sync>,
    pub select_list: SelectListTheme,
}

pub struct EditorOptions {
    pub padding_x: usize,
    pub autocomplete_max_visible: usize,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            padding_x: 0,
            autocomplete_max_visible: 5,
        }
    }
}

pub struct Editor {
    state: EditorState,
    pub focused: bool,
    request_render: Option<Box<dyn Fn() + Send + Sync>>,
    theme: EditorTheme,
    padding_x: usize,
    last_width: usize,
    scroll_offset: usize,
    autocomplete_provider: Option<Box<dyn AutocompleteProvider + Send + Sync>>,
    autocomplete_list: Option<SelectList>,
    autocomplete_state: Option<AutocompleteState>,
    autocomplete_prefix: String,
    autocomplete_max_visible: usize,
    terminal_rows: u16,
    pastes: Vec<(u64, String)>,
    paste_counter: u64,
    history: Vec<String>,
    history_index: isize,
    kill_ring: KillRing,
    last_action: Option<LastAction>,
    undo_stack: UndoStack<EditorState>,
    jump_mode: Option<JumpDirection>,
    preferred_visual_col: Option<usize>,
    snapped_from_cursor_col: Option<usize>,
    pub on_submit: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pub on_change: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pub disable_submit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutocompleteState {
    Regular,
    Force,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JumpDirection {
    Forward,
    Backward,
}

impl Editor {
    pub fn new(
        request_render: Option<Box<dyn Fn() + Send + Sync>>,
        theme: EditorTheme,
        options: Option<EditorOptions>,
        terminal_rows: u16,
    ) -> Self {
        let opts = options.unwrap_or_default();
        Self {
            state: EditorState {
                lines: vec![String::new()],
                cursor_line: 0,
                cursor_col: 0,
            },
            focused: false,
            request_render,
            theme,
            padding_x: opts.padding_x,
            last_width: 80,
            scroll_offset: 0,
            autocomplete_provider: None,
            autocomplete_list: None,
            autocomplete_state: None,
            autocomplete_prefix: String::new(),
            autocomplete_max_visible: opts.autocomplete_max_visible.max(3).min(20),
            terminal_rows,
            pastes: Vec::new(),
            paste_counter: 0,
            history: Vec::new(),
            history_index: -1,
            kill_ring: KillRing::new(),
            last_action: None,
            undo_stack: UndoStack::new(),
            jump_mode: None,
            preferred_visual_col: None,
            snapped_from_cursor_col: None,
            on_submit: None,
            on_change: None,
            disable_submit: false,
        }
    }

    pub fn set_padding_x(&mut self, padding: usize) {
        self.padding_x = padding;
        if let Some(ref cb) = self.request_render {
            cb();
        }
    }

    pub fn set_autocomplete_provider(&mut self, provider: Box<dyn AutocompleteProvider + Send + Sync>) {
        self.cancel_autocomplete();
        self.autocomplete_provider = Some(provider);
    }

    pub fn add_to_history(&mut self, text: &str) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        if self.history.first().map_or(false, |h| h == &trimmed) {
            return;
        }
        self.history.insert(0, trimmed);
        if self.history.len() > 100 {
            self.history.pop();
        }
    }

    pub fn get_text(&self) -> String {
        self.state.lines.join("\n")
    }

    pub fn get_expanded_text(&self) -> String {
        let text = self.state.lines.join("\n");
        let mut result = text;
        for (paste_id, paste_content) in &self.pastes {
            let pattern = format!("[paste #{}", paste_id);
            if let Some(start) = result.find(&pattern) {
                if let Some(end) = result[start..].find(']') {
                    let before = &result[..start];
                    let after = &result[start + end + 1..];
                    result = format!("{}{}{}", before, paste_content, after);
                }
            }
        }
        result
    }

    pub fn get_lines(&self) -> Vec<String> {
        self.state.lines.clone()
    }

    pub fn get_cursor(&self) -> (usize, usize) {
        (self.state.cursor_line, self.state.cursor_col)
    }

    pub fn set_text(&mut self, text: &str) {
        self.cancel_autocomplete();
        self.last_action = None;
        self.history_index = -1;
        let normalized = Self::normalize_text(text);
        if self.get_text() != normalized {
            self.push_undo_snapshot();
        }
        self.set_text_internal(&normalized);
    }

    pub fn insert_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.cancel_autocomplete();
        self.push_undo_snapshot();
        self.last_action = None;
        self.history_index = -1;
        self.insert_text_at_cursor_internal(text);
    }

    fn set_text_internal(&mut self, text: &str) {
        let lines: Vec<String> = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        self.state.lines = lines;
        self.state.cursor_line = self.state.lines.len().saturating_sub(1);
        let last_line_len = self.state.lines.last().map(|l| l.len()).unwrap_or(0);
        self.set_cursor_col(last_line_len);
        self.scroll_offset = 0;
        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn insert_text_at_cursor_internal(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let normalized = Self::normalize_text(text);
        let inserted_lines: Vec<&str> = normalized.split('\n').collect();

        let current_line = self.state.lines[self.state.cursor_line].clone();
        let before = &current_line[..self.state.cursor_col];
        let after = &current_line[self.state.cursor_col..];

        if inserted_lines.len() == 1 {
            self.state.lines[self.state.cursor_line] = format!("{}{}{}", before, normalized, after);
            self.set_cursor_col(self.state.cursor_col + normalized.len());
        } else {
            let mut new_lines: Vec<String> = Vec::new();
            new_lines.extend_from_slice(&self.state.lines[..self.state.cursor_line]);
            new_lines.push(format!("{}{}", before, inserted_lines[0]));
            for mid in inserted_lines[1..inserted_lines.len() - 1].iter() {
                new_lines.push(mid.to_string());
            }
            new_lines.push(format!(
                "{}{}",
                inserted_lines[inserted_lines.len() - 1],
                after
            ));
            new_lines.extend_from_slice(&self.state.lines[self.state.cursor_line + 1..]);
            self.state.lines = new_lines;
            self.state.cursor_line += inserted_lines.len() - 1;
            self.set_cursor_col(inserted_lines.last().unwrap().len());
        }

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn is_editor_empty(&self) -> bool {
        self.state.lines.len() == 1 && self.state.lines[0].is_empty()
    }

    fn is_on_first_visual_line(&self) -> bool {
        let vl = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&vl) == 0
    }

    fn is_on_last_visual_line(&self) -> bool {
        let vl = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&vl) >= vl.len().saturating_sub(1)
    }

    fn navigate_history(&mut self, direction: isize) {
        self.last_action = None;
        if self.history.is_empty() {
            return;
        }
        let new_index = self.history_index - direction;
        if new_index < -1 || new_index >= self.history.len() as isize {
            return;
        }
        if self.history_index == -1 && new_index >= 0 {
            self.push_undo_snapshot();
        }
        self.history_index = new_index;
        if self.history_index == -1 {
            self.set_text_internal("");
        } else {
            let text = self.history[self.history_index as usize].clone();
            self.set_text_internal(&text);
        }
    }

    fn valid_paste_ids(&self) -> Vec<u64> {
        self.pastes.iter().map(|(id, _)| *id).collect()
    }

    fn segment_graphemes<'a>(&'a self, text: &'a str) -> Vec<&'a str> {
        let ids = self.valid_paste_ids();
        segment_with_markers(text, &ids)
    }

    fn normalize_text(text: &str) -> String {
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ")
    }

    fn insert_character(&mut self, ch: char, skip_undo_coalescing: bool) {
        self.history_index = -1;

        if !skip_undo_coalescing {
            if is_whitespace_char(ch) || self.last_action != Some(LastAction::TypeWord) {
                self.push_undo_snapshot();
            }
            self.last_action = Some(LastAction::TypeWord);
        }

        let line = self.state.lines[self.state.cursor_line].clone();
        let before = &line[..self.state.cursor_col];
        let after = &line[self.state.cursor_col..];
        self.state.lines[self.state.cursor_line] = format!("{}{}{}", before, ch, after);
        self.set_cursor_col(self.state.cursor_col + ch.len_utf8());

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn handle_paste(&mut self, pasted_text: &str) {
        self.cancel_autocomplete();
        self.history_index = -1;
        self.last_action = None;
        self.push_undo_snapshot();

        let decoded = pasted_text;
        let clean = Self::normalize_text(decoded);
        let mut filtered: String = clean
            .chars()
            .filter(|&c| c == '\n' || c as u32 >= 32)
            .collect();

        if filtered.starts_with('/') || filtered.starts_with('~') || filtered.starts_with('.') {
            let line = &self.state.lines[self.state.cursor_line];
            if self.state.cursor_col > 0 {
                let prev = line[..self.state.cursor_col].chars().next_back().unwrap_or(' ');
                if prev.is_alphanumeric() {
                    filtered.insert(0, ' ');
                }
            }
        }

        let pasted_lines: Vec<&str> = filtered.split('\n').collect();
        let num_pasted_lines = pasted_lines.len();
        let filtered_len = filtered.len();
        if num_pasted_lines > 10 || filtered_len > 1000 {
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.push((paste_id, filtered));
            let marker = if num_pasted_lines > 10 {
                format!("[paste #{} +{} lines]", paste_id, num_pasted_lines)
            } else {
                format!("[paste #{} {} chars]", paste_id, filtered_len)
            };
            self.insert_text_at_cursor_internal(&marker);
            return;
        }

        self.insert_text_at_cursor_internal(&filtered);
    }

    fn add_new_line(&mut self) {
        self.cancel_autocomplete();
        self.history_index = -1;
        self.last_action = None;
        self.push_undo_snapshot();

        let line = self.state.lines[self.state.cursor_line].clone();
        let before = line[..self.state.cursor_col].to_string();
        let after = line[self.state.cursor_col..].to_string();

        self.state.lines[self.state.cursor_line] = before;
        self.state.lines.insert(self.state.cursor_line + 1, after);
        self.state.cursor_line += 1;
        self.set_cursor_col(0);

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn handle_backspace(&mut self) {
        self.history_index = -1;
        self.last_action = None;

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();
            let line = self.state.lines[self.state.cursor_line].clone();
            let graphemes: Vec<&str> = self.segment_graphemes(&line[..self.state.cursor_col]);
            let last = graphemes.last().copied().unwrap_or("");
            let len = if last.is_empty() { 1 } else { last.len() };

            let before = line[..self.state.cursor_col.saturating_sub(len)].to_string();
            let after = line[self.state.cursor_col..].to_string();
            self.state.lines[self.state.cursor_line] = before.clone() + &after;
            self.set_cursor_col(before.len());
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();
            let prev = self.state.lines[self.state.cursor_line - 1].clone();
            let curr = self.state.lines[self.state.cursor_line].clone();
            self.state.lines[self.state.cursor_line - 1] = prev.clone() + &curr;
            self.state.lines.remove(self.state.cursor_line);
            self.state.cursor_line -= 1;
            self.set_cursor_col(prev.len());
        }

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn handle_forward_delete(&mut self) {
        self.history_index = -1;
        self.last_action = None;

        let line = self.state.lines[self.state.cursor_line].clone();
        if self.state.cursor_col < line.len() {
            self.push_undo_snapshot();
            let after = &line[self.state.cursor_col..];
            let graphemes: Vec<&str> = self.segment_graphemes(after);
            let first = graphemes.first().copied().unwrap_or("");
            let len = if first.is_empty() { 1 } else { first.len() };

            let before = line[..self.state.cursor_col].to_string();
            let rest = line[self.state.cursor_col + len..].to_string();
            self.state.lines[self.state.cursor_line] = before + &rest;
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();
            let next = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = line + &next;
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn delete_to_line_end(&mut self) {
        self.history_index = -1;
        let line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col < line.len() {
            self.push_undo_snapshot();
            let deleted = &line[self.state.cursor_col..];
            self.kill_ring.push(deleted.to_string(), true, self.last_action == Some(LastAction::Kill));
            self.last_action = Some(LastAction::Kill);
            self.state.lines[self.state.cursor_line] = line[..self.state.cursor_col].to_string();
        } else if self.state.cursor_line < self.state.lines.len() - 1 {
            self.push_undo_snapshot();
            self.kill_ring.push("\n".to_string(), true, self.last_action == Some(LastAction::Kill));
            self.last_action = Some(LastAction::Kill);
            let next = self.state.lines[self.state.cursor_line + 1].clone();
            self.state.lines[self.state.cursor_line] = line + &next;
            self.state.lines.remove(self.state.cursor_line + 1);
        }

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn delete_to_line_start(&mut self) {
        self.history_index = -1;
        let line = self.state.lines[self.state.cursor_line].clone();

        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();
            let deleted = &line[..self.state.cursor_col];
            self.kill_ring.push(deleted.to_string(), true, self.last_action == Some(LastAction::Kill));
            self.last_action = Some(LastAction::Kill);

            self.state.lines[self.state.cursor_line] = line[self.state.cursor_col..].to_string();
        }

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn yank(&mut self) {
        if self.kill_ring.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        if let Some(text) = self.kill_ring.peek().map(|s| s.to_string()) {
            self.insert_yanked_text(&text);
        }
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo_snapshot();
        self.delete_yanked_text();
        self.kill_ring.rotate();
        if let Some(text) = self.kill_ring.peek().map(|s| s.to_string()) {
            self.insert_yanked_text(&text);
        }
        self.last_action = Some(LastAction::Yank);
    }

    fn insert_yanked_text(&mut self, text: &str) {
        self.history_index = -1;
        let lines: Vec<&str> = text.split('\n').collect();

        if lines.len() == 1 {
            let line = self.state.lines[self.state.cursor_line].clone();
            let before = line[..self.state.cursor_col].to_string();
            let after = line[self.state.cursor_col..].to_string();
            self.state.lines[self.state.cursor_line] = format!("{}{}{}", before, text, after);
            self.set_cursor_col(self.state.cursor_col + text.len());
        } else {
            let line = self.state.lines[self.state.cursor_line].clone();
            let before = line[..self.state.cursor_col].to_string();
            let after = line[self.state.cursor_col..].to_string();

            self.state.lines[self.state.cursor_line] = format!("{}{}", before, lines[0]);
            for i in 1..lines.len() - 1 {
                self.state.lines
                    .insert(self.state.cursor_line + i, lines[i].to_string());
            }
            let last_idx = self.state.cursor_line + lines.len() - 1;
            self.state.lines
                .insert(last_idx, format!("{}{}", lines[lines.len() - 1], after));
            self.state.cursor_line = last_idx;
            self.set_cursor_col(lines[lines.len() - 1].len());
        }

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn delete_yanked_text(&mut self) {
        let yanked_text = match self.kill_ring.peek() {
            Some(t) => t.to_string(),
            None => return,
        };
        let yank_lines: Vec<&str> = yanked_text.split('\n').collect();

        if yank_lines.len() == 1 {
            let line = self.state.lines[self.state.cursor_line].clone();
            let delete_len = yanked_text.len();
            let before = line[..self.state.cursor_col.saturating_sub(delete_len)].to_string();
            let after = line[self.state.cursor_col..].to_string();
            self.state.lines[self.state.cursor_line] = before.clone() + &after;
            self.set_cursor_col(before.len());
        } else {
            let start_line = self.state.cursor_line.saturating_sub(yank_lines.len() - 1);
            let after = self.state.lines[self.state.cursor_line][self.state.cursor_col..].to_string();
            let before = self.state.lines[start_line][..self.state.lines[start_line]
                .len()
                .saturating_sub(yank_lines[0].len())]
                .to_string();

            for _ in 0..yank_lines.len() {
                self.state.lines.remove(start_line);
            }
            self.state.lines
                .insert(start_line, format!("{}{}", before, after));
            self.state.cursor_line = start_line;
            self.set_cursor_col(before.len());
        }

        if let Some(ref cb) = self.on_change {
            cb(&self.get_text());
        }
    }

    fn move_to_line_start(&mut self) {
        self.last_action = None;
        self.set_cursor_col(0);
    }

    fn move_to_line_end(&mut self) {
        self.last_action = None;
        let len = self.state.lines[self.state.cursor_line].len();
        self.set_cursor_col(len);
    }

    fn move_word_backwards(&mut self) {
        self.last_action = None;
        let line = &self.state.lines[self.state.cursor_line];

        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let prev_len = self.state.lines[self.state.cursor_line].len();
                self.set_cursor_col(prev_len);
            }
            return;
        }

        let result = find_word_backward(line, self.state.cursor_col, &WordNavigationOptions {
            is_atomic_segment: Some(&is_paste_marker),
        });
        self.set_cursor_col(result);
    }

    fn move_word_forwards(&mut self) {
        self.last_action = None;
        let line = &self.state.lines[self.state.cursor_line];

        if self.state.cursor_col >= line.len() {
            if self.state.cursor_line < self.state.lines.len() - 1 {
                self.state.cursor_line += 1;
                self.set_cursor_col(0);
            }
            return;
        }

        let result = find_word_forward(line, self.state.cursor_col, &WordNavigationOptions {
            is_atomic_segment: Some(&is_paste_marker),
        });
        self.set_cursor_col(result);
    }

    fn submit_value(&mut self) {
        self.cancel_autocomplete();
        let result = self.get_expanded_text().trim().to_string();
        self.state = EditorState {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        };
        self.pastes.clear();
        self.paste_counter = 0;
        self.history_index = -1;
        self.scroll_offset = 0;
        self.undo_stack.clear();
        self.last_action = None;

        if let Some(ref cb) = self.on_change {
            cb("");
        }
        if let Some(ref cb) = self.on_submit {
            cb(&result);
        }
    }

    fn set_cursor_col(&mut self, col: usize) {
        self.state.cursor_col = col;
        self.preferred_visual_col = None;
        self.snapped_from_cursor_col = None;
    }

    fn push_undo_snapshot(&mut self) {
        self.undo_stack.push(&self.state);
    }

    fn undo(&mut self) {
        self.history_index = -1;
        if let Some(snapshot) = self.undo_stack.pop() {
            self.state = snapshot;
            self.last_action = None;
            self.preferred_visual_col = None;
            if let Some(ref cb) = self.on_change {
                cb(&self.get_text());
            }
        }
    }

    fn jump_to_char(&mut self, ch: char, direction: JumpDirection) {
        self.last_action = None;
        let is_forward = direction == JumpDirection::Forward;
        let lines = &self.state.lines;

        if is_forward {
            for line_idx in self.state.cursor_line..lines.len() {
                let line = &lines[line_idx];
                let search_from = if line_idx == self.state.cursor_line {
                    self.state.cursor_col + 1
                } else {
                    0
                };
                if let Some(idx) = line[search_from..].find(ch) {
                    let abs_idx = search_from + idx;
                    self.state.cursor_line = line_idx;
                    self.set_cursor_col(abs_idx);
                    return;
                }
            }
        } else {
            for line_idx in (0..=self.state.cursor_line).rev() {
                let line = &lines[line_idx];
                let search_from = if line_idx == self.state.cursor_line {
                    if self.state.cursor_col == 0 {
                        continue;
                    }
                    self.state.cursor_col - 1
                } else {
                    line.len()
                };
                if let Some(idx) = line[..search_from].rfind(ch) {
                    self.state.cursor_line = line_idx;
                    self.set_cursor_col(idx);
                    return;
                }
            }
        }
    }

    fn page_scroll(&mut self, direction: isize) {
        self.last_action = None;
        let page_size = (self.terminal_rows as f64 * 0.3).max(5.0) as usize;
        let vl = self.build_visual_line_map(self.last_width);
        let current_vl = self.find_current_visual_line(&vl);
        let target = (current_vl as isize + direction * page_size as isize)
            .max(0)
            .min(vl.len().saturating_sub(1) as isize) as usize;
        self.move_to_visual_line(&vl, current_vl, target);
    }

    fn build_visual_line_map(&self, width: usize) -> Vec<VisualLine> {
        let mut result: Vec<VisualLine> = Vec::new();

        for (i, line) in self.state.lines.iter().enumerate() {
            let lw = visible_width(line);
            if line.is_empty() {
                result.push(VisualLine {
                    logical_line: i,
                    start_col: 0,
                    length: 0,
                });
            } else if lw <= width {
                result.push(VisualLine {
                    logical_line: i,
                    start_col: 0,
                    length: line.len(),
                });
            } else {
                let segs: Vec<&str> = self.segment_graphemes(line);
                let chunks = word_wrap_line(line, width, Some(&segs));
                for chunk in &chunks {
                    result.push(VisualLine {
                        logical_line: i,
                        start_col: chunk.start_index,
                        length: chunk.end_index - chunk.start_index,
                    });
                }
            }
        }
        result
    }

    fn find_visual_line_at(&self, visual_lines: &[VisualLine], line: usize, col: usize) -> usize {
        for (i, vl) in visual_lines.iter().enumerate() {
            if vl.logical_line != line {
                continue;
            }
            let offset = col.saturating_sub(vl.start_col);
            let is_last = i == visual_lines.len() - 1
                || visual_lines[i + 1].logical_line != vl.logical_line;
            if offset <= vl.length || (is_last && offset == vl.length) {
                return i;
            }
        }
        visual_lines.len().saturating_sub(1)
    }

    fn find_current_visual_line(&self, visual_lines: &[VisualLine]) -> usize {
        self.find_visual_line_at(visual_lines, self.state.cursor_line, self.state.cursor_col)
    }

    fn move_to_visual_line(&mut self, visual_lines: &[VisualLine], current_vl: usize, target_vl: usize) {
        let current = &visual_lines[current_vl];
        let target = &visual_lines[target_vl];

        let current_visual_col = if let Some(snapped) = self.snapped_from_cursor_col {
            let vl_idx = self.find_visual_line_at(visual_lines, current.logical_line, snapped);
            snapped - visual_lines[vl_idx].start_col
        } else {
            self.state.cursor_col.saturating_sub(current.start_col)
        };

        let is_last_source = current_vl == visual_lines.len() - 1
            || visual_lines[current_vl + 1].logical_line != current.logical_line;
        let source_max = if is_last_source {
            current.length
        } else {
            current.length.saturating_sub(1).max(0)
        };

        let is_last_target = target_vl == visual_lines.len() - 1
            || visual_lines[target_vl + 1].logical_line != target.logical_line;
        let target_max = if is_last_target {
            target.length
        } else {
            target.length.saturating_sub(1).max(0)
        };

        let move_to = self.compute_vertical_move_column(current_visual_col, source_max, target_max);

        self.state.cursor_line = target.logical_line;
        let target_col = target.start_col + move_to;
        let logical = &self.state.lines[target.logical_line];
        self.state.cursor_col = target_col.min(logical.len());

        // Snap to atomic segment boundaries
        let segs = self.segment_graphemes(logical);
        for seg in &segs {
            let seg_start = seg.as_ptr() as usize - logical.as_ptr() as usize;
            if seg_start > self.state.cursor_col {
                break;
            }
            if seg.len() <= 1 {
                continue;
            }
            if self.state.cursor_col < seg_start + seg.len() {
                self.snapped_from_cursor_col = Some(self.state.cursor_col);
                self.state.cursor_col = seg_start;
                return;
            }
        }
        self.snapped_from_cursor_col = None;
    }

    fn compute_vertical_move_column(
        &mut self,
        current_visual_col: usize,
        source_max: usize,
        target_max: usize,
    ) -> usize {
        let has_preferred = self.preferred_visual_col.is_some();
        let cursor_in_middle = current_visual_col < source_max;
        let target_too_short = target_max < current_visual_col;

        if !has_preferred || cursor_in_middle {
            if target_too_short {
                self.preferred_visual_col = Some(current_visual_col);
                return target_max;
            }
            self.preferred_visual_col = None;
            return current_visual_col;
        }

        let pref = self.preferred_visual_col.unwrap();
        if target_too_short || target_max < pref {
            return target_max;
        }

        self.preferred_visual_col = None;
        pref
    }

    fn is_slash_menu_allowed(&self) -> bool {
        self.state.cursor_line == 0
    }

    fn is_at_start_of_message(&self) -> bool {
        if !self.is_slash_menu_allowed() {
            return false;
        }
        let line = &self.state.lines[self.state.cursor_line];
        let before = &line[..self.state.cursor_col];
        before.trim().is_empty() || before.trim() == "/"
    }

    fn is_in_slash_command_context(&self, text_before_cursor: &str) -> bool {
        self.is_slash_menu_allowed() && text_before_cursor.trim_start().starts_with('/')
    }

    fn try_trigger_autocomplete(&mut self) {
        if self.autocomplete_provider.is_none() {
            return;
        }
        let line = &self.state.lines[self.state.cursor_line];
        let before = &line[..self.state.cursor_col];
        if let Some(ref provider) = self.autocomplete_provider {
            let result = provider.get_suggestions(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
                false,
            );
            self.apply_autocomplete_result(result);
        }
    }

    fn force_file_autocomplete(&mut self) {
        if self.autocomplete_provider.is_none() {
            return;
        }

        if let Some(ref provider) = self.autocomplete_provider {
            let should = provider.should_trigger_file_completion(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
            );
            if !should {
                return;
            }
            let result = provider.get_suggestions(
                &self.state.lines,
                self.state.cursor_line,
                self.state.cursor_col,
                true,
            );
            self.apply_autocomplete_result(result);
        }
    }

    fn apply_autocomplete_result(&mut self, result: Option<AutocompleteSuggestions>) {
        match result {
            Some(suggestions) if !suggestions.items.is_empty() => {
                let items: Vec<SelectItem> = suggestions
                    .items
                    .iter()
                    .map(|item| SelectItem {
                        value: item.value.clone(),
                        description: item.description.clone(),
                        metadata: None,
                    })
                    .collect();
                let mut list = SelectList::new(items, self.autocomplete_max_visible);
                let best = self.get_best_autocomplete_match_index(&suggestions.items, &suggestions.prefix);
                if best > 0 {
                    list.set_selected_index(best);
                }
                self.autocomplete_list = Some(list);
                self.autocomplete_prefix = suggestions.prefix;
                self.autocomplete_state = Some(AutocompleteState::Regular);
            }
            _ => {
                self.cancel_autocomplete();
            }
        }
    }

    fn get_best_autocomplete_match_index(
        &self,
        items: &[AutocompleteItem],
        prefix: &str,
    ) -> usize {
        if prefix.is_empty() {
            return 0;
        }
        let mut first_prefix = None;
        for (i, item) in items.iter().enumerate() {
            if item.value == prefix {
                return i;
            }
            if first_prefix.is_none() && item.value.starts_with(prefix) {
                first_prefix = Some(i);
            }
        }
        first_prefix.unwrap_or(0)
    }

    fn cancel_autocomplete(&mut self) {
        self.autocomplete_state = None;
        self.autocomplete_list = None;
        self.autocomplete_prefix.clear();
    }

    fn handle_tab_completion(&mut self) {
        let line = &self.state.lines[self.state.cursor_line];
        let before = &line[..self.state.cursor_col];

        if self.is_in_slash_command_context(before) && !before.trim_start().contains(' ') {
            self.try_trigger_autocomplete();
        } else {
            self.force_file_autocomplete();
        }
    }

    fn layout_text(&self, content_width: usize) -> Vec<LayoutLine> {
        if self.state.lines.is_empty()
            || (self.state.lines.len() == 1 && self.state.lines[0].is_empty())
        {
            return vec![LayoutLine {
                text: String::new(),
                has_cursor: true,
                cursor_pos: Some(0),
            }];
        }

        let mut result: Vec<LayoutLine> = Vec::new();

        for (i, line) in self.state.lines.iter().enumerate() {
            let is_current = i == self.state.cursor_line;
            let lw = visible_width(line);

            if lw <= content_width {
                if is_current {
                    result.push(LayoutLine {
                        text: line.clone(),
                        has_cursor: true,
                        cursor_pos: Some(self.state.cursor_col),
                    });
                } else {
                    result.push(LayoutLine {
                        text: line.clone(),
                        has_cursor: false,
                        cursor_pos: None,
                    });
                }
            } else {
                let segs: Vec<&str> = self.segment_graphemes(line);
                let chunks = word_wrap_line(line, content_width, Some(&segs));
                for (ci, chunk) in chunks.iter().enumerate() {
                    let is_last = ci == chunks.len() - 1;
                    let has_cursor_in_chunk = if is_current {
                        if is_last {
                            self.state.cursor_col >= chunk.start_index
                        } else {
                            self.state.cursor_col >= chunk.start_index
                                && self.state.cursor_col < chunk.end_index
                        }
                    } else {
                        false
                    };

                    if has_cursor_in_chunk {
                        let adj = (self.state.cursor_col - chunk.start_index).min(chunk.text.len());
                        result.push(LayoutLine {
                            text: chunk.text.clone(),
                            has_cursor: true,
                            cursor_pos: Some(adj),
                        });
                    } else {
                        result.push(LayoutLine {
                            text: chunk.text.clone(),
                            has_cursor: false,
                            cursor_pos: None,
                        });
                    }
                }
            }
        }
        result
    }
}

#[derive(Debug, Clone, Copy)]
struct VisualLine {
    logical_line: usize,
    start_col: usize,
    length: usize,
}

impl VisualLine {
    fn is_none(&self) -> bool {
        false
    }
}

impl Component for Editor {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let width = width as usize;
        let max_padding = width.saturating_sub(1) / 2;
        let padding_x = self.padding_x.min(max_padding);
        let content_width = (width.saturating_sub(padding_x * 2)).max(1);
        let layout_width = if padding_x > 0 {
            content_width
        } else {
            content_width.saturating_sub(1).max(1)
        };

        let layout_lines = self.layout_text(layout_width);

        let max_visible = (self.terminal_rows as f64 * 0.3).max(5.0) as usize;
        let cursor_line_idx = layout_lines
            .iter()
            .position(|l| l.has_cursor)
            .unwrap_or(0);

        let mut scroll_offset = self.scroll_offset;
        if cursor_line_idx < scroll_offset {
            scroll_offset = cursor_line_idx;
        } else if cursor_line_idx >= scroll_offset + max_visible {
            scroll_offset = cursor_line_idx + 1 - max_visible;
        }
        let max_scroll = layout_lines.len().saturating_sub(max_visible);
        scroll_offset = scroll_offset.min(max_scroll);

        let visible: Vec<&LayoutLine> = layout_lines
            .iter()
            .skip(scroll_offset)
            .take(max_visible)
            .collect();

        let border_style = (self.theme.border_color)();
        let left_pad = " ".repeat(padding_x);
        let right_pad = left_pad.clone();

        let mut result: Vec<Line<'static>> = Vec::new();

        // Top border with scroll indicator
        if scroll_offset > 0 {
            let indicator = format!("─── ↑ {} more ", scroll_offset);
            let remaining = width.saturating_sub(visible_width(&indicator));
            let border_text = format!("{}{}", indicator, "─".repeat(remaining));
            result.push(Line::from(Span::styled(border_text, border_style)));
        } else {
            result.push(Line::from(Span::styled(
                "─".repeat(width),
                border_style,
            )));
        }

        // Emit cursor marker only when focused
        let emit_cursor = self.focused && self.autocomplete_state.is_none();
        let cursor_bg = Style::default().bg(ratatui::style::Color::Gray);

        for layout_line in &visible {
            let line_text = layout_line.text.clone();
            let line_vis = visible_width(&line_text);
            let mut spans: Vec<Span<'static>> = Vec::new();

            if layout_line.has_cursor && emit_cursor {
                if let Some(cpos) = layout_line.cursor_pos {
                    let cpos = cpos.min(line_text.len());
                    let before = &line_text[..cpos];
                    let after = &line_text[cpos..];

                    spans.push(Span::raw(before.to_string()));

                    if !after.is_empty() {
                        let graphemes: Vec<&str> = after.graphemes(true).collect();
                        let first = graphemes.first().copied().unwrap_or("");
                        let rest: String = graphemes
                            .iter()
                            .skip(1)
                            .flat_map(|g| g.chars())
                            .collect();
                        spans.push(Span::styled(first.to_string(), cursor_bg));
                        spans.push(Span::raw(rest));
                    } else {
                        spans.push(Span::styled(" ".to_string(), cursor_bg));
                    }
                } else {
                    spans.push(Span::raw(line_text.clone()));
                }
            } else {
                spans.push(Span::raw(line_text.clone()));
            }

            let pad_count = content_width.saturating_sub(line_vis);
            if pad_count > 0 {
                spans.push(Span::raw(" ".repeat(pad_count)));
            }

            let mut final_spans = vec![Span::raw(left_pad.clone())];
            final_spans.extend(spans);
            final_spans.push(Span::raw(right_pad.clone()));
            result.push(Line::from(final_spans));
        }

        // Bottom border
        let lines_below = layout_lines.len().saturating_sub(scroll_offset + visible.len());
        if lines_below > 0 {
            let indicator = format!("─── ↓ {} more ", lines_below);
            let remaining = width.saturating_sub(visible_width(&indicator));
            let border_text = format!("{}{}", indicator, "─".repeat(remaining.max(0)));
            result.push(Line::from(Span::styled(border_text, border_style)));
        } else {
            result.push(Line::from(Span::styled(
                "─".repeat(width),
                border_style,
            )));
        }

        // Autocomplete overlay
        if self.autocomplete_state.is_some() {
            if let Some(ref list) = self.autocomplete_list {
                let ac_lines = list.render(content_width as u16);
                for ac_line in ac_lines {
                    let ac_text = ac_line.to_string();
                    let lw = visible_width(&ac_text);
                    let lp = " ".repeat(content_width.saturating_sub(lw));
                    let mut ac_spans = vec![Span::raw(left_pad.clone())];
                    ac_spans.extend(ac_line.spans.iter().cloned());
                    ac_spans.push(Span::raw(lp));
                    ac_spans.push(Span::raw(right_pad.clone()));
                    result.push(Line::from(ac_spans));
                }
            }
        }

        result
    }

    fn cursor_position(&self) -> Option<(u16, u16)> {
        if !self.focused || self.autocomplete_state.is_some() {
            return None;
        }

        let width = self.last_width as usize;
        let max_padding = width.saturating_sub(1) / 2;
        let padding_x = self.padding_x.min(max_padding);
        let content_width = (width.saturating_sub(padding_x * 2)).max(1);
        let layout_width = if padding_x > 0 {
            content_width
        } else {
            content_width.saturating_sub(1).max(1)
        };

        let layout_lines = self.layout_text(layout_width);

        let max_visible = (self.terminal_rows as f64 * 0.3).max(5.0) as usize;
        let cursor_line_idx = layout_lines
            .iter()
            .position(|l| l.has_cursor)
            .unwrap_or(0);

        let mut scroll_offset = self.scroll_offset;
        if cursor_line_idx < scroll_offset {
            scroll_offset = cursor_line_idx;
        } else if cursor_line_idx >= scroll_offset + max_visible {
            scroll_offset = cursor_line_idx + 1 - max_visible;
        }
        let max_scroll = layout_lines.len().saturating_sub(max_visible);
        scroll_offset = scroll_offset.min(max_scroll);

        let row = 1u16 + (cursor_line_idx.saturating_sub(scroll_offset)) as u16;

        if let Some(layout_line) = layout_lines.get(cursor_line_idx) {
            if let Some(cpos) = layout_line.cursor_pos {
                let col = padding_x as u16 + visible_width(&layout_line.text[..cpos.min(layout_line.text.len())]) as u16;
                return Some((row, col));
            }
        }

        Some((row, padding_x as u16))
    }

    fn handle_input(&mut self, event: &KeyEvent) {
        let kb = get_keybindings();

        // Jump mode
        if let Some(jump_dir) = self.jump_mode.take() {
            if let KeyCode::Char(ch) = event.code {
                if (ch as u32) >= 32 {
                    self.jump_to_char(ch, jump_dir);
                    return;
                }
            }
        }

        if kb.matches(event, "undo") {
            self.undo();
            return;
        }

        if self.autocomplete_state.is_some() {
            if kb.matches(event, "cancel") {
                self.cancel_autocomplete();
                return;
            }
            if event.code == KeyCode::Up && event.modifiers.is_empty()
                || event.code == KeyCode::Down && event.modifiers.is_empty()
            {
                if let Some(ref mut list) = self.autocomplete_list {
                    list.handle_input(event);
                }
                return;
            }
            if event.code == KeyCode::Tab {
                self.push_undo_snapshot();
                self.last_action = None;
                let selected_item = self.autocomplete_list.as_ref().and_then(|list| list.get_selected_item().cloned());
                if let Some(selected) = selected_item {
                        if let Some(ref provider) = self.autocomplete_provider {
                            let result = provider.apply_completion(
                                &self.state.lines,
                                self.state.cursor_line,
                                self.state.cursor_col,
                                &AutocompleteItem {
                                    value: selected.value.clone(),
                                    label: selected.value.clone(),
                                    description: selected.description.clone(),
                                },
                                &self.autocomplete_prefix,
                            );
                            self.state.lines = result.lines;
                            self.state.cursor_line = result.cursor_line;
                            self.set_cursor_col(result.cursor_col);
                            self.cancel_autocomplete();
                            if let Some(ref cb) = self.on_change {
                                cb(&self.get_text());
                            }
                        }
                }
                return;
            }
        }
        if event.code == KeyCode::Enter && event.modifiers.is_empty() {
            if self.autocomplete_state.is_some() {
                self.push_undo_snapshot();
                self.last_action = None;
                let selected_item = self.autocomplete_list.as_ref().and_then(|list| list.get_selected_item().cloned());
                if let Some(selected) = selected_item {
                        if let Some(ref provider) = self.autocomplete_provider {
                            let result = provider.apply_completion(
                                &self.state.lines,
                                self.state.cursor_line,
                                self.state.cursor_col,
                                &AutocompleteItem {
                                    value: selected.value.clone(),
                                    label: selected.value.clone(),
                                    description: selected.description.clone(),
                                },
                                &self.autocomplete_prefix,
                            );
                            self.state.lines = result.lines;
                            self.state.cursor_line = result.cursor_line;
                            self.set_cursor_col(result.cursor_col);
                            if self.autocomplete_prefix.starts_with('/') {
                                self.cancel_autocomplete();
                            } else {
                                self.cancel_autocomplete();
                                if let Some(ref cb) = self.on_change {
                                    cb(&self.get_text());
                                }
                                return;
                            }
                        }
                }
            }
        }

        // Tab - trigger completion
        if event.code == KeyCode::Tab && self.autocomplete_state.is_none() {
            self.handle_tab_completion();
            return;
        }

        // Delete to line end
        if kb.matches(event, "deleteToLineEnd") {
            self.delete_to_line_end();
            return;
        }

        // Delete actions
        let shift_backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::SHIFT);
        let shift_delete = KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT);
        if kb.matches(event, "deleteBackward")
            || *event == shift_backspace
            || event.code == KeyCode::Backspace && event.modifiers.is_empty()
        {
            self.handle_backspace();
            return;
        }
        if kb.matches(event, "deleteForward")
            || *event == shift_delete
            || event.code == KeyCode::Delete && event.modifiers.is_empty()
        {
            self.handle_forward_delete();
            return;
        }

        // Kill ring
        if event.code == KeyCode::Char('y') && event.modifiers == KeyModifiers::CONTROL {
            self.yank();
            return;
        }
        if event.code == KeyCode::Char('y') && event.modifiers == KeyModifiers::ALT {
            self.yank_pop();
            return;
        }

        // Cursor movement
        if kb.matches(event, "cursorLineStart") || event.code == KeyCode::Home && event.modifiers.is_empty() {
            self.move_to_line_start();
            return;
        }
        if kb.matches(event, "cursorLineEnd") || event.code == KeyCode::End && event.modifiers.is_empty() {
            self.move_to_line_end();
            return;
        }
        if kb.matches(event, "cursorWordLeft") || event.code == KeyCode::Char('b') && event.modifiers == KeyModifiers::ALT {
            self.move_word_backwards();
            return;
        }
        if kb.matches(event, "cursorWordRight") || event.code == KeyCode::Char('f') && event.modifiers == KeyModifiers::ALT {
            self.move_word_forwards();
            return;
        }

        // New line (Shift+Enter, Alt+Enter, etc)
        if event.code == KeyCode::Enter && event.modifiers.is_empty() {
            if !self.disable_submit {
                let line = &self.state.lines[self.state.cursor_line];
                if self.state.cursor_col > 0
                    && line[..self.state.cursor_col].chars().next_back() == Some('\\')
                {
                    self.handle_backspace();
                    self.add_new_line();
                    return;
                }
                self.submit_value();
            } else {
                self.add_new_line();
            }
            return;
        }

        // Ctrl+C - let parent handle
        if event.code == KeyCode::Char('c') && event.modifiers == KeyModifiers::CONTROL {
            return;
        }

        // Arrow navigation
        if event.code == KeyCode::Up && event.modifiers.is_empty() {
            if self.is_editor_empty() {
                self.navigate_history(-1);
            } else if self.history_index > -1 && self.is_on_first_visual_line() {
                self.navigate_history(-1);
            } else if self.is_on_first_visual_line() {
                self.move_to_line_start();
            } else {
                let vl = self.build_visual_line_map(self.last_width);
                let current_vl = self.find_current_visual_line(&vl);
                if current_vl > 0 {
                    self.move_to_visual_line(&vl, current_vl, current_vl - 1);
                }
            }
            return;
        }
        if event.code == KeyCode::Down && event.modifiers.is_empty() {
            if self.history_index > -1 && self.is_on_last_visual_line() {
                self.navigate_history(1);
            } else if self.is_on_last_visual_line() {
                self.move_to_line_end();
            } else {
                let vl = self.build_visual_line_map(self.last_width);
                let current_vl = self.find_current_visual_line(&vl);
                if current_vl + 1 < vl.len() {
                    self.move_to_visual_line(&vl, current_vl, current_vl + 1);
                }
            }
            return;
        }
        if event.code == KeyCode::Right && event.modifiers.is_empty() {
            let line = &self.state.lines[self.state.cursor_line];
            if self.state.cursor_col < line.len() {
                let after = &line[self.state.cursor_col..];
                let graphemes: Vec<&str> = after.graphemes(true).collect();
                let first = graphemes.first().copied().unwrap_or("");
                self.set_cursor_col(self.state.cursor_col + first.len().max(1));
            } else if self.state.cursor_line < self.state.lines.len() - 1 {
                self.state.cursor_line += 1;
                self.set_cursor_col(0);
            }
            return;
        }
        if event.code == KeyCode::Left && event.modifiers.is_empty() {
            if self.state.cursor_col > 0 {
                let before = &self.state.lines[self.state.cursor_line][..self.state.cursor_col];
                let graphemes: Vec<&str> = before.graphemes(true).collect();
                let last = graphemes.last().copied().unwrap_or("");
                self.set_cursor_col(self.state.cursor_col.saturating_sub(last.len().max(1)));
            } else if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let prev = &self.state.lines[self.state.cursor_line];
                self.set_cursor_col(prev.len());
            }
            return;
        }

        // Page up/down
        if event.code == KeyCode::PageUp && event.modifiers.is_empty() {
            self.page_scroll(-1);
            return;
        }
        if event.code == KeyCode::PageDown && event.modifiers.is_empty() {
            self.page_scroll(1);
            return;
        }

        // Printable characters
        if let KeyCode::Char(ch) = event.code {
            if event.modifiers.is_empty() && (ch as u32) >= 32 {
                self.insert_character(ch, false);
                return;
            }
        }
    }
}

impl Focusable for Editor {
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
    fn test_editor_empty() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let editor = Editor::new(None, theme, None, 24);
        assert!(editor.get_text().is_empty());
        assert_eq!(editor.state.cursor_line, 0);
        assert_eq!(editor.state.cursor_col, 0);
    }

    #[test]
    fn test_editor_set_text() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.set_text("hello world");
        assert_eq!(editor.get_text(), "hello world");
    }

    #[test]
    fn test_editor_insert_character() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.insert_character('a', false);
        editor.insert_character('b', false);
        editor.insert_character('c', false);
        assert_eq!(editor.get_text(), "abc");
        assert_eq!(editor.state.cursor_col, 3);
    }

    #[test]
    fn test_editor_backspace() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.set_text("hello");
        editor.state.cursor_col = 5;
        editor.handle_backspace();
        assert_eq!(editor.get_text(), "hell");
        assert_eq!(editor.state.cursor_col, 4);
    }

    #[test]
    fn test_editor_newline() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.set_text("hello");
        editor.state.cursor_col = 3;
        editor.add_new_line();
        assert_eq!(editor.state.lines.len(), 2);
        assert_eq!(editor.state.lines[0], "hel");
        assert_eq!(editor.state.lines[1], "lo");
        assert_eq!(editor.state.cursor_line, 1);
        assert_eq!(editor.state.cursor_col, 0);
    }

    #[test]
    fn test_editor_undo() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.set_text("hello");
        editor.push_undo_snapshot();
        editor.set_text("world");
        editor.undo();
        assert_eq!(editor.get_text(), "hello");
    }

    #[test]
    fn test_editor_history() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.add_to_history("first");
        editor.add_to_history("second");
        assert_eq!(editor.history.len(), 2);
        assert_eq!(editor.history[0], "second");
    }

    #[test]
    fn test_editor_delete_to_line_end() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.set_text("hello world");
        editor.state.cursor_col = 5;
        editor.delete_to_line_end();
        assert_eq!(editor.get_text(), "hello");
    }

    #[test]
    fn test_word_wrap_line_simple() {
        let result = word_wrap_line("hello world", 5, None);
        assert!(result.len() >= 2);
        assert_eq!(result[0].text, "hello");
    }

    #[test]
    fn test_word_wrap_no_wrap_needed() {
        let result = word_wrap_line("hello", 10, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "hello");
    }

    #[test]
    fn test_is_paste_marker() {
        assert!(is_paste_marker("[paste #1 +123 lines]"));
        assert!(is_paste_marker("[paste #2 456 chars]"));
        assert!(!is_paste_marker("regular text"));
    }

    #[test]
    fn test_normalize_text() {
        assert_eq!(Editor::normalize_text("hello\r\nworld"), "hello\nworld");
        assert_eq!(Editor::normalize_text("hello\rworld"), "hello\nworld");
        assert_eq!(Editor::normalize_text("hello\tworld"), "hello    world");
    }

    #[test]
    fn test_editor_move_cursor_left_right() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let mut editor = Editor::new(None, theme, None, 24);
        editor.set_text("hello");

        // Move to end then test left/right
        editor.state.cursor_col = 5;

        // Move left
        editor.handle_input(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(editor.state.cursor_col, 4);

        // Move right
        editor.handle_input(&KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(editor.state.cursor_col, 5);
    }

    #[test]
    fn test_editor_render_empty() {
        setup_kb();
        let theme = EditorTheme {
            border_color: Box::new(|| Style::default()),
            select_list: SelectListTheme::default(),
        };
        let editor = Editor::new(None, theme, None, 24);
        let lines = editor.render(40);
        assert!(!lines.is_empty());
    }
}
