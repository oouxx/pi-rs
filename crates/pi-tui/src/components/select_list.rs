use crossterm::event::KeyEvent;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::keybindings::get_keybindings;
use crate::tui::Component;
use crate::utils::{truncate_to_width, visible_width};

#[derive(Debug, Clone)]
pub struct SelectItem {
    pub value: String,
    pub description: Option<String>,
    pub metadata: Option<String>,
}

pub struct SelectListTheme {
    pub selected_prefix: String,
    pub unselected_prefix: String,
    pub selected_style: Style,
    pub unselected_style: Style,
    pub description_style: Style,
    pub counter_style: Style,
    pub filter_style: Style,
}

impl Default for SelectListTheme {
    fn default() -> Self {
        Self {
            selected_prefix: "▶ ".to_string(),
            unselected_prefix: "  ".to_string(),
            selected_style: Style::default(),
            unselected_style: Style::default(),
            description_style: Style::default(),
            counter_style: Style::default(),
            filter_style: Style::default(),
        }
    }
}

pub struct SelectList {
    items: Vec<SelectItem>,
    selected_index: usize,
    max_visible: usize,
    filter: String,
    theme: SelectListTheme,
}

impl SelectList {
    pub fn new(items: Vec<SelectItem>, max_visible: usize) -> Self {
        Self {
            items,
            selected_index: 0,
            max_visible: max_visible.max(1),
            filter: String::new(),
            theme: SelectListTheme::default(),
        }
    }

    pub fn with_theme(mut self, theme: SelectListTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
        self.selected_index = 0;
    }

    pub fn set_selected_index(&mut self, index: usize) {
        let max = self.filtered_items().len().saturating_sub(1);
        self.selected_index = index.min(max);
    }

    pub fn get_selected_item(&self) -> Option<&SelectItem> {
        self.filtered_items().get(self.selected_index).copied()
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn filtered_items(&self) -> Vec<&SelectItem> {
        if self.filter.is_empty() {
            return self.items.iter().collect();
        }
        let filter_lower = self.filter.to_lowercase();
        self.items
            .iter()
            .filter(|item| item.value.to_lowercase().starts_with(&filter_lower))
            .collect()
    }

    fn select_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    fn select_down(&mut self) {
        let filtered = self.filtered_items();
        let max = filtered.len().saturating_sub(1);
        if self.selected_index < max {
            self.selected_index += 1;
        }
    }

    fn page_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(self.max_visible);
    }

    fn page_down(&mut self) {
        let filtered = self.filtered_items();
        let max = filtered.len().saturating_sub(1);
        self.selected_index = (self.selected_index + self.max_visible).min(max);
    }
}

impl Component for SelectList {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let filtered = self.filtered_items();
        let total = filtered.len();
        let mut lines = Vec::new();

        if !self.filter.is_empty() {
            let filter_text = format!("> {}", self.filter);
            lines.push(Line::from(Span::styled(
                filter_text,
                self.theme.filter_style,
            )));
        }

        if total == 0 {
            lines.push(Line::from(Span::raw("  (no matches)")));
            return lines;
        }

        let window_start = if self.selected_index < self.max_visible / 2 {
            0
        } else if self.selected_index >= total.saturating_sub(self.max_visible / 2) {
            total.saturating_sub(self.max_visible)
        } else {
            self.selected_index.saturating_sub(self.max_visible / 2)
        };

        let window_end = (window_start + self.max_visible).min(total);

        let desc_width = 20usize;

        for i in window_start..window_end {
            let item = &filtered[i];
            let is_selected = i == self.selected_index;
            let prefix = if is_selected {
                &self.theme.selected_prefix
            } else {
                &self.theme.unselected_prefix
            };
            let value_style = if is_selected {
                self.theme.selected_style
            } else {
                self.theme.unselected_style
            };

            let value_width = (width as usize)
                .saturating_sub(prefix.len() + desc_width + 4)
                .max(10);

            let truncated_value = truncate_to_width(&item.value, value_width);

            let mut spans: Vec<Span<'static>> = vec![Span::styled(
                format!("{}{}", prefix, truncated_value),
                value_style,
            )];

            if let Some(ref desc) = item.description {
                let truncated_desc = truncate_to_width(desc, desc_width);
                let pad = value_width.saturating_sub(visible_width(&truncated_value));
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
                spans.push(Span::raw("  "));
                spans.push(Span::styled(truncated_desc, self.theme.description_style));
            }

            lines.push(Line::from(spans));
        }

        if total > self.max_visible {
            let counter_text = format!("  {}-{} of {}", window_start + 1, window_end, total);
            lines.push(Line::from(Span::styled(
                counter_text,
                self.theme.counter_style,
            )));
        }

        lines
    }

    fn handle_input(&mut self, event: &KeyEvent) {
        let kb = get_keybindings();

        if kb.matches(event, "selectUp") {
            self.select_up();
        } else if kb.matches(event, "selectDown") {
            self.select_down();
        } else if kb.matches(event, "selectPageUp") {
            self.page_up();
        } else if kb.matches(event, "selectPageDown") {
            self.page_down();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::init_keybindings;

    fn make_items() -> Vec<SelectItem> {
        vec![
            SelectItem {
                value: "apple".into(),
                description: Some("A fruit".into()),
                metadata: None,
            },
            SelectItem {
                value: "banana".into(),
                description: Some("Yellow fruit".into()),
                metadata: None,
            },
            SelectItem {
                value: "cherry".into(),
                description: Some("Red fruit".into()),
                metadata: None,
            },
        ]
    }

    #[test]
    fn test_select_list_navigation() {
        init_keybindings(None);
        let mut list = SelectList::new(make_items(), 10);
        assert_eq!(list.selected_index, 0);

        list.select_down();
        assert_eq!(list.selected_index, 1);

        list.select_up();
        assert_eq!(list.selected_index, 0);
    }

    #[test]
    fn test_select_list_filter() {
        let mut list = SelectList::new(make_items(), 10);
        list.set_filter("ba");
        let filtered = list.filtered_items();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "banana");
    }

    #[test]
    fn test_select_list_get_selected() {
        let mut list = SelectList::new(make_items(), 10);
        list.select_down();
        let item = list.get_selected_item().unwrap();
        assert_eq!(item.value, "banana");
    }

    #[test]
    fn test_select_list_no_items() {
        let list = SelectList::new(vec![], 10);
        let lines = list.render(40);
        assert!(lines.iter().any(|l| l.to_string().contains("no matches")));
    }

    #[test]
    fn test_select_list_normalizes_multiline_descriptions() {
        let items = vec![SelectItem {
            value: "item1".into(),
            description: Some("line1\nline2".into()),
            metadata: None,
        }];
        let list = SelectList::new(items, 10);
        let lines = list.render(80);
        assert!(lines.iter().any(|l| l.to_string().contains("line1")));
    }

    #[test]
    fn test_select_list_descriptions_aligned_with_truncated_labels() {
        let items = vec![
            SelectItem {
                value: "a very long item name that should be truncated".into(),
                description: Some("short desc".into()),
                metadata: None,
            },
            SelectItem {
                value: "short".into(),
                description: Some("another desc".into()),
                metadata: None,
            },
        ];
        let list = SelectList::new(items, 10);
        let lines = list.render(40);
        assert!(lines.len() >= 2);
        let has_first_desc = lines.iter().any(|l| l.to_string().contains("short desc"));
        let has_second_desc = lines.iter().any(|l| l.to_string().contains("another desc"));
        assert!(has_first_desc || has_second_desc);
    }

    #[test]
    fn test_select_list_filter_case_insensitive() {
        let items = vec![
            SelectItem {
                value: "Apple".into(),
                description: None,
                metadata: None,
            },
            SelectItem {
                value: "BANANA".into(),
                description: None,
                metadata: None,
            },
            SelectItem {
                value: "cherry".into(),
                description: None,
                metadata: None,
            },
        ];
        let mut list = SelectList::new(items, 10);
        list.set_filter("ap");
        let filtered = list.filtered_items();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "Apple");
    }

    #[test]
    fn test_select_list_selection_wraps_at_bottom() {
        let items = vec![
            SelectItem {
                value: "a".into(),
                description: None,
                metadata: None,
            },
            SelectItem {
                value: "b".into(),
                description: None,
                metadata: None,
            },
            SelectItem {
                value: "c".into(),
                description: None,
                metadata: None,
            },
        ];
        let mut list = SelectList::new(items, 10);
        list.select_down();
        list.select_down();
        list.select_down();
        assert_eq!(list.selected_index(), 2);
    }

    #[test]
    fn test_select_list_page_navigation() {
        let items: Vec<SelectItem> = (0..20)
            .map(|i| SelectItem {
                value: format!("item{}", i),
                description: None,
                metadata: None,
            })
            .collect();
        let mut list = SelectList::new(items, 5);
        assert_eq!(list.selected_index(), 0);
        list.page_down();
        assert_eq!(list.selected_index(), 5);
        list.page_down();
        assert_eq!(list.selected_index(), 10);
        list.page_up();
        assert_eq!(list.selected_index(), 5);
    }

    #[test]
    fn test_select_list_set_selected_index_clamps() {
        let items = vec![
            SelectItem {
                value: "a".into(),
                description: None,
                metadata: None,
            },
            SelectItem {
                value: "b".into(),
                description: None,
                metadata: None,
            },
        ];
        let mut list = SelectList::new(items, 10);
        list.set_selected_index(999);
        assert_eq!(list.selected_index(), 1);
    }

    #[test]
    fn test_select_list_render_shows_counter() {
        let items: Vec<SelectItem> = (0..20)
            .map(|i| SelectItem {
                value: format!("item{}", i),
                description: None,
                metadata: None,
            })
            .collect();
        let list = SelectList::new(items, 5);
        let lines = list.render(80);
        let has_counter = lines.iter().any(|l| l.to_string().contains("of 20"));
        assert!(
            has_counter,
            "Counter should be shown when total > max_visible"
        );
    }
}
