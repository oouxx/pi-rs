//! Multi-line text editor — wraps the vendored grok-build textarea
//! (`xai-ratatui-textarea`), which provides vim-style word movement, undo
//! history, selection, bracket handling and a proper wrapped-line viewport.
//!
//! The component keeps the previous `Editor` API so `app.rs` and callers
//! stay unchanged; rendering delegates to the textarea widget.

use xai_ratatui_textarea::{ClipboardProvider, TextArea};

/// System-clipboard backend for the vendored textarea (TS editor reads and
/// writes the real system clipboard — `copyToClipboard` / `readClipboardText`).
#[derive(Debug, Default)]
pub struct SystemClipboard;

impl ClipboardProvider for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        crate::clipboard::read_clipboard_text()
    }

    fn set(&mut self, text: &str) {
        let _ = crate::clipboard::copy_to_clipboard(text);
    }
}

/// Editor input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Insert,
    Normal,
}

/// A multi-line text editor.
pub struct Editor {
    ta: TextArea,
}

impl Editor {
    pub fn new(initial: &str) -> Self {
        let mut ta = TextArea::new();
        ta.set_clipboard_provider(Box::new(SystemClipboard));
        ta.set_text(initial);
        Self { ta }
    }

    pub fn text(&self) -> String {
        self.ta.text().to_string()
    }

    pub fn handle_key(&mut self, key: &crossterm::event::KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }
        self.ta.input(*key);
    }

    /// Cursor row (logical line index within the text, 0-based).
    pub fn cursor_row(&self) -> u16 {
        let cursor = self.ta.cursor();
        self.ta.text()[..cursor].bytes().filter(|&b| b == b'\n').count() as u16
    }

    /// Cursor column within its logical line (byte offset).
    pub fn cursor_col(&self) -> u16 {
        let cursor = self.ta.cursor();
        let text = self.ta.text();
        let line_start = text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        (cursor - line_start) as u16
    }

    pub fn mode(&self) -> &EditorMode {
        &EditorMode::Insert
    }

    /// The underlying textarea (for widget rendering / cursor placement).
    pub fn textarea(&self) -> &TextArea {
        &self.ta
    }
}
