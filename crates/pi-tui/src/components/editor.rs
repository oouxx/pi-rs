//! Multi-line text editor with cursor tracking.

/// Editor input mode.
pub enum EditorMode {
    Insert,
    Normal,
}

/// A multi-line text editor.
pub struct Editor {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    mode: EditorMode,
}

impl Editor {
    pub fn new(initial: &str) -> Self {
        let lines: Vec<String> = if initial.is_empty() {
            vec![String::new()]
        } else {
            initial.lines().map(|l| l.to_string()).collect()
        };
        Self {
            lines,
            cursor_row: 0,
            cursor_col: 0,
            mode: EditorMode::Insert,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn cursor_row(&self) -> u16 { self.cursor_row as u16 }
    pub fn cursor_col(&self) -> u16 { self.cursor_col as u16 }
    pub fn mode(&self) -> &EditorMode { &self.mode }

    pub fn handle_key(&mut self, key: &crossterm::event::KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }
        match &self.mode {
            EditorMode::Insert => self.handle_insert_key(key),
            EditorMode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_insert_key(&mut self, key: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Char(c) => {
                self.lines[self.cursor_row].insert(self.cursor_col, c);
                self.cursor_col += 1;
            }
            KeyCode::Enter => {
                let rest = if self.cursor_col < self.lines[self.cursor_row].len() {
                    self.lines[self.cursor_row].split_off(self.cursor_col)
                } else {
                    String::new()
                };
                self.lines.insert(self.cursor_row + 1, rest);
                self.cursor_row += 1;
                self.cursor_col = 0;
            }
            KeyCode::Backspace => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                    self.lines[self.cursor_row].remove(self.cursor_col);
                } else if self.cursor_row > 0 {
                    let prev_len = self.lines[self.cursor_row - 1].len();
                    let current = self.lines.remove(self.cursor_row);
                    self.lines[self.cursor_row - 1].push_str(&current);
                    self.cursor_row -= 1;
                    self.cursor_col = prev_len;
                }
            }
            KeyCode::Delete => {
                if self.cursor_col < self.lines[self.cursor_row].len() {
                    self.lines[self.cursor_row].remove(self.cursor_col);
                } else if self.cursor_row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cursor_row + 1);
                    self.lines[self.cursor_row].push_str(&next);
                }
            }
            KeyCode::Left => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                }
            }
            KeyCode::Right => {
                if self.cursor_col < self.lines[self.cursor_row].len() {
                    self.cursor_col += 1;
                } else if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = 0;
                }
            }
            KeyCode::Up => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            KeyCode::Down => {
                if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            KeyCode::Home => self.cursor_col = 0,
            KeyCode::End => self.cursor_col = self.lines[self.cursor_row].len(),
            KeyCode::Tab => {
                for _ in 0..4 { self.lines[self.cursor_row].insert(self.cursor_col, ' '); }
                self.cursor_col += 4;
            }
            KeyCode::Esc => self.mode = EditorMode::Normal,
            _ => {}
        }
    }

    fn handle_normal_key(&mut self, key: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Char('i') | KeyCode::Char('I') => self.mode = EditorMode::Insert,
            KeyCode::Char('h') => if self.cursor_col > 0 { self.cursor_col -= 1; },
            KeyCode::Char('l') => if self.cursor_col < self.lines[self.cursor_row].len() { self.cursor_col += 1; },
            KeyCode::Char('j') => if self.cursor_row + 1 < self.lines.len() { self.cursor_row += 1; self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len()); },
            KeyCode::Char('k') => if self.cursor_row > 0 { self.cursor_row -= 1; self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len()); },
            KeyCode::Char('0') => self.cursor_col = 0,
            KeyCode::Char('$') => self.cursor_col = self.lines[self.cursor_row].len(),
            KeyCode::Char('x') => {
                if self.cursor_col < self.lines[self.cursor_row].len() {
                    self.lines[self.cursor_row].remove(self.cursor_col);
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.lines[self.cursor_row].clear();
                self.cursor_col = 0;
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                self.lines.insert(self.cursor_row + 1, String::new());
                self.cursor_row += 1;
                self.cursor_col = 0;
                self.mode = EditorMode::Insert;
            }
            KeyCode::Esc => {} // already in normal mode
            _ => {}
        }
    }
}
