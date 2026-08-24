//! Autocomplete engine for `/` commands and `@` file paths with fuzzy matching.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
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
        scored.sort_by_key(|(_, exact)| std::cmp::Reverse(*exact));

        self.results = scored.into_iter().map(|(i, _)| i).collect();
        // The menu stays open even with zero matches (TS SelectList shows
        // the "No matching commands" row) — it closes on Esc/space/select.
        self.selected = 0;
        self.visible = true;
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

    /// Render the completion list TS-style (the original `SelectList` used
    /// for the editor autocomplete): plain inline rows — no border, no
    /// title, no background. The selected row carries a `→ ` accent prefix
    /// and accent text; the description sits muted in an aligned second
    /// column; a `(n/m)` scroll row appears when the list scrolls; an open
    /// menu with no matches shows `No matching commands`.
    pub fn render_rows(&self, frame: &mut Frame, area: Rect, t: &crate::theme::Theme) {
        if !self.visible || area.height == 0 {
            return;
        }
        let candidates = match self.trigger {
            Some(CompletionTrigger::Slash) => &self.commands,
            Some(CompletionTrigger::AtFile) => &self.files,
            None => return,
        };
        let mut row = |y: u16, spans: Vec<Span<'static>>| {
            if y < area.y + area.height {
                frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, y, area.width, 1));
            }
        };

        if self.results.is_empty() {
            row(area.y, vec![Span::styled("  No matching commands", Style::new().fg(t.muted))]);
            return;
        }

        // TS SelectList: primary column = widest label + gap, capped at 32.
        let max_visible = 5usize;
        let total = self.results.len();
        let widest = candidates
            .iter()
            .map(|c| unicode_width::UnicodeWidthStr::width(c.label.as_str()) + 2)
            .max()
            .unwrap_or(0);
        let primary_col = widest.clamp(1, 32);
        // Selection-centered window (TS `selected - floor(maxVisible/2)`).
        let start = self
            .selected
            .saturating_sub(max_visible / 2)
            .min(total.saturating_sub(max_visible));
        let end = (start + max_visible).min(total);

        for (i, &idx) in self.results.iter().enumerate().skip(start).take(end - start) {
            let item = &candidates[idx];
            let selected = i == self.selected;
            let prefix = if selected { "\u{2192} " } else { "  " };
            let label_w = unicode_width::UnicodeWidthStr::width(item.label.as_str());
            let max_label = primary_col.saturating_sub(2).max(1);
            let label = if label_w > max_label {
                crate::app::truncate_to_width(&item.label, max_label)
            } else {
                item.label.clone()
            };
            let label_w = unicode_width::UnicodeWidthStr::width(label.as_str());
            let spacing = " ".repeat(primary_col.saturating_sub(label_w).max(1));
            let mut spans = vec![
                Span::styled(prefix, Style::new().fg(t.accent)),
                Span::styled(label, Style::new().fg(if selected { t.accent } else { t.text })),
            ];
            if !item.description.is_empty() {
                let desc_start = 2 + primary_col;
                let desc_w = (area.width as usize).saturating_sub(desc_start).saturating_sub(2);
                if desc_w > 10 {
                    let desc = crate::app::truncate_to_width(&item.description, desc_w);
                    spans.push(Span::styled(format!("{spacing}{desc}"), Style::new().fg(t.muted)));
                }
            }
            row(area.y + i as u16, spans);
        }

        // Scroll indicator (TS `  (n/m)` in muted).
        if start > 0 || end < total {
            let info = format!("  ({}/{})", self.selected + 1, total);
            row(area.y + (end - start) as u16, vec![Span::styled(info, Style::new().fg(t.muted))]);
        }
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
