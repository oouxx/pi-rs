//! Selectable list widget.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState};
use ratatui::Frame;

/// A selectable list with keyboard navigation.
pub struct SelectList {
    items: Vec<String>,
    pub state: ListState,
    title: String,
    max_visible: usize,
}

impl SelectList {
    pub fn new(items: Vec<String>) -> Self {
        let mut state = ListState::default();
        state.select(if items.is_empty() { None } else { Some(0) });
        Self {
            items,
            state,
            title: String::new(),
            max_visible: 5,
        }
    }

    /// TS `SelectList(items, maxVisible)`: at most `max` rows are shown,
    /// centered on the selection. Defaults to 5.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max.max(1);
        self
    }

    pub fn with_title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }
    pub fn selected_item(&self) -> Option<&str> {
        self.state
            .selected()
            .and_then(|i| self.items.get(i))
            .map(String::as_str)
    }

    pub fn handle_key(&mut self, key: &KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }
        if self.items.is_empty() {
            return;
        }
        // v0.87.1 `SelectList.handleInput`: Up/Down wrap at the ends; it does
        // not bind Home/End/j/k or page keys.
        match key.code {
            KeyCode::Up => {
                let i = self.state.selected().unwrap_or(0);
                let next = if i == 0 { self.items.len() - 1 } else { i - 1 };
                self.state.select(Some(next));
            }
            KeyCode::Down => {
                let i = self.state.selected().unwrap_or(0);
                let next = if i + 1 >= self.items.len() { 0 } else { i + 1 };
                self.state.select(Some(next));
            }
            _ => {}
        }
    }

    pub fn render_to_frame(&self, frame: &mut Frame, area: Rect, t: &crate::theme::Theme) {
        // TS `getVisibleRange`: keep the selection centered within
        // `maxVisible` rows (plus the two border rows).
        let visible_height = (self.max_visible as u16).saturating_add(2);
        let area = Rect {
            height: area.height.min(visible_height),
            ..area
        };
        let total = self.items.len();
        let selected = self.state.selected().unwrap_or(0);
        let start = if total > self.max_visible {
            (selected.saturating_sub(self.max_visible / 2)).min(total - self.max_visible)
        } else {
            0
        };
        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|item| ListItem::new(Text::raw(item.clone())).style(Style::default()))
            .collect();
        // TS select-list theme: accent prefix/text for the selection, muted
        // border, selectedBg highlight.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.border_muted))
            .title_style(Style::new().fg(t.accent))
            .title(if self.title.is_empty() {
                " Select "
            } else {
                &self.title
            });
        frame.render_widget(Clear, area);
        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .fg(t.accent)
                    .bg(t.selected_bg)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("\u{2192} ");
        // Clone state for rendering
        let mut state = ListState::default();
        state.select(self.state.selected());
        *state.offset_mut() = start;
        frame.render_stateful_widget(list, area, &mut state);
    }
}
