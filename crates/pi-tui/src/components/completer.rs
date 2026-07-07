//! Autocomplete engine for `/` commands and `@` file paths with fuzzy matching.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

/// What triggered the current completion popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTrigger {
    Slash,
    AtFile,
}

/// A single completion candidate.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub description: String,
    /// Text to insert when selected.
    pub insert_text: String,
}

/// Autocomplete engine state.
pub struct Completer {
    pub commands: Vec<CompletionItem>,
    pub files: Vec<CompletionItem>,
    pub trigger: Option<CompletionTrigger>,
    /// Indices into the active candidate list, sorted by relevance.
    pub results: Vec<usize>,
    pub selected: usize,
    pub query: String,
    pub visible: bool,
}

impl Completer {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            files: Vec::new(),
            trigger: None,
            results: Vec::new(),
            selected: 0,
            query: String::new(),
            visible: false,
        }
    }

    pub fn set_commands(&mut self, cmds: Vec<CompletionItem>) { self.commands = cmds; }
    pub fn set_files(&mut self, files: Vec<CompletionItem>) { self.files = files; }

    /// Activate completion with fuzzy matching via prefix comparison.
    pub fn activate(&mut self, trigger: CompletionTrigger, query: &str) {
        let candidates = match trigger {
            CompletionTrigger::Slash => &self.commands,
            CompletionTrigger::AtFile => &self.files,
        };

        self.trigger = Some(trigger);
        self.query = query.to_string();
        let q_lower = query.to_lowercase();

        // Simple prefix + contains matching
        let mut scored: Vec<(usize, bool)> = candidates
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let label_lower = item.label.to_lowercase();
                q_lower.is_empty()
                    || label_lower.starts_with(&q_lower)
                    || label_lower.contains(&q_lower)
            })
            .map(|(i, item)| {
                let label_lower = item.label.to_lowercase();
                let exact_prefix = label_lower.starts_with(&q_lower);
                (i, exact_prefix)
            })
            .collect();

        // Prefix matches first
        scored.sort_by(|a, b| b.1.cmp(&a.1));

        self.results = scored.into_iter().map(|(i, _)| i).collect();
        self.selected = 0;
        self.visible = !self.results.is_empty();
    }

    pub fn deactivate(&mut self) {
        self.visible = false;
        self.trigger = None;
        self.query.clear();
        self.results.clear();
    }

    pub fn next(&mut self) {
        if self.visible && !self.results.is_empty() {
            self.selected = (self.selected + 1) % self.results.len();
        }
    }

    pub fn prev(&mut self) {
        if self.visible && !self.results.is_empty() {
            self.selected = if self.selected == 0 { self.results.len() - 1 } else { self.selected - 1 };
        }
    }

    pub fn selected_insert(&self) -> Option<String> {
        self.visible.then(|| {
            self.results.get(self.selected).and_then(|&idx| {
                let candidates = match self.trigger? {
                    CompletionTrigger::Slash => &self.commands,
                    CompletionTrigger::AtFile => &self.files,
                };
                candidates.get(idx).map(|c| c.insert_text.clone())
            })
        }).flatten()
    }

    /// Render the completion popup above the cursor position.
    pub fn render(&self, frame: &mut Frame, cursor_x: u16, cursor_y: u16) {
        if !self.visible { return; }

        let candidates = match self.trigger {
            Some(CompletionTrigger::Slash) => &self.commands,
            Some(CompletionTrigger::AtFile) => &self.files,
            None => return,
        };

        let count = self.results.len().min(8);
        if count == 0 { return; }

        let width = 45u16;
        let height = (count as u16).min(10) + 2;
        let area = Rect::new(cursor_x.saturating_sub(2), cursor_y.saturating_sub(height + 1), width, height);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(Color::Cyan))
            .style(Style::new().bg(Color::Black))
            .title( match self.trigger {
                Some(CompletionTrigger::Slash) => " Commands ",
                Some(CompletionTrigger::AtFile) => " Files ",
                None => " ",
            });

        frame.render_widget(Clear, area);

        let items: Vec<ListItem> = self.results.iter().take(count).map(|&idx| {
            if let Some(item) = candidates.get(idx) {
                let desc = if item.description.is_empty() { String::new() } else { format!(" — {}", item.description) };
                let style = match self.trigger {
                    Some(CompletionTrigger::Slash) => Style::new().fg(Color::Cyan),
                    Some(CompletionTrigger::AtFile) => Style::new().fg(Color::Yellow),
                    None => Style::default(),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(item.label.clone(), style),
                    Span::styled(desc, Style::new().fg(Color::DarkGray)),
                ]))
            } else {
                ListItem::new(Line::from(Span::raw("")))
            }
        }).collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD));

        let mut state = ListState::default();
        state.select(Some(self.selected.min(count.saturating_sub(1))));
        frame.render_stateful_widget(list, area, &mut state);
    }

    pub fn should_activate(c: char) -> Option<CompletionTrigger> {
        match c {
            '/' => Some(CompletionTrigger::Slash),
            '@' => Some(CompletionTrigger::AtFile),
            _ => None,
        }
    }
}

impl Default for Completer {
    fn default() -> Self { Self::new() }
}
