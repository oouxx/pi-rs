//! Selectable list widget.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

/// A selectable list with keyboard navigation.
pub struct SelectList {
    items: Vec<String>,
    pub state: ListState,
    title: String,
}

impl SelectList {
    pub fn new(items: Vec<String>) -> Self {
        let mut state = ListState::default();
        state.select(if items.is_empty() { None } else { Some(0) });
        Self { items, state, title: String::new() }
    }

    pub fn with_title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }

    pub fn selected_index(&self) -> Option<usize> { self.state.selected() }
    pub fn selected_item(&self) -> Option<&str> {
        self.state.selected().and_then(|i| self.items.get(i)).map(|s| s.as_str())
    }

    pub fn handle_key(&mut self, key: &KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press { return; }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.state.selected().unwrap_or(0);
                if i > 0 { self.state.select(Some(i - 1)); }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.state.selected().unwrap_or(0);
                if i + 1 < self.items.len() { self.state.select(Some(i + 1)); }
            }
            KeyCode::Home => self.state.select(Some(0)),
            KeyCode::End => self.state.select(Some(self.items.len().saturating_sub(1))),
            KeyCode::PageUp => {
                let i = self.state.selected().unwrap_or(0);
                self.state.select(Some(i.saturating_sub(5)));
            }
            KeyCode::PageDown => {
                let i = self.state.selected().unwrap_or(0);
                self.state.select(Some((i + 5).min(self.items.len().saturating_sub(1))));
            }
            _ => {}
        }
    }

    pub fn render_to_frame(&self, frame: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self.items.iter().map(|item| {
            ListItem::new(Text::raw(item.clone())).style(Style::default())
        }).collect();
        let block = Block::default()
            .borders(Borders::ALL).border_type(BorderType::Rounded)
            .title(if self.title.is_empty() { " Select " } else { &self.title });
        frame.render_widget(Clear, area);
        let list = List::new(items).block(block)
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");
        // Clone state for rendering
        let mut state = ListState::default();
        state.select(self.state.selected());
        frame.render_stateful_widget(list, area, &mut state);
    }
}
