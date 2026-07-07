use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::fuzzy::fuzzy_filter;
use crate::keybindings::get_keybindings;
use crate::tui::Component;
use crate::utils::{truncate_to_width, visible_width};

use super::input::Input;

fn wrap_plain_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut line_width = 0;

    for ch in text.chars() {
        if ch == '\n' {
            lines.push(current_line);
            current_line = String::new();
            line_width = 0;
            continue;
        }

        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

        if line_width + ch_width > width {
            if !current_line.is_empty() {
                lines.push(current_line);
            }
            current_line = String::new();
            line_width = 0;
        }

        current_line.push(ch);
        line_width += ch_width;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

pub struct SettingItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub current_value: String,
    pub values: Option<Vec<String>>,
}

pub struct SettingsListTheme {
    pub label: Box<dyn Fn(&str, bool) -> Style + Send + Sync>,
    pub value: Box<dyn Fn(&str, bool) -> Style + Send + Sync>,
    pub description: Box<dyn Fn(&str) -> Style + Send + Sync>,
    pub cursor: String,
    pub hint: Box<dyn Fn(&str) -> Style + Send + Sync>,
}

pub struct SettingsListOptions {
    pub enable_search: bool,
}

impl Default for SettingsListOptions {
    fn default() -> Self {
        Self {
            enable_search: false,
        }
    }
}

pub struct SettingsList {
    items: Vec<SettingItem>,
    filtered_items: Vec<usize>,
    selected_index: usize,
    max_visible: usize,
    theme: SettingsListTheme,
    search_input: Option<Input>,
    search_enabled: bool,
    on_change: Box<dyn Fn(&str, &str) + Send + Sync>,
    on_cancel: Box<dyn Fn() + Send + Sync>,
}

impl SettingsList {
    pub fn new(
        items: Vec<SettingItem>,
        max_visible: usize,
        theme: SettingsListTheme,
        on_change: Box<dyn Fn(&str, &str) + Send + Sync>,
        on_cancel: Box<dyn Fn() + Send + Sync>,
        options: SettingsListOptions,
    ) -> Self {
        let search_enabled = options.enable_search;
        let search_input = if search_enabled {
            Some(Input::new())
        } else {
            None
        };
        let filtered: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            filtered_items: filtered,
            selected_index: 0,
            max_visible: max_visible.max(1),
            theme,
            search_input,
            search_enabled,
            on_change,
            on_cancel,
        }
    }

    pub fn update_value(&mut self, id: &str, new_value: &str) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.current_value = new_value.to_string();
        }
    }

    fn display_items(&self) -> Vec<&SettingItem> {
        if self.search_enabled {
            self.filtered_items
                .iter()
                .map(|&i| &self.items[i])
                .collect()
        } else {
            self.items.iter().collect()
        }
    }

    fn apply_filter(&mut self, query: &str) {
        if query.is_empty() {
            self.filtered_items = (0..self.items.len()).collect();
        } else {
            let labels: Vec<String> = self.items.iter().map(|i| i.label.clone()).collect();
            let filtered_refs = fuzzy_filter(&labels, query, |s| s);
            self.filtered_items = filtered_refs
                .into_iter()
                .filter_map(|label| self.items.iter().position(|item| item.label == **label))
                .collect();
        }
        self.selected_index = 0;
    }
}

impl Component for SettingsList {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if let Some(ref input) = self.search_input {
            lines.extend(input.render(width));
            lines.push(Line::from(vec![]));
        }

        let display = self.display_items();
        if display.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No matching settings",
                (self.theme.hint)("  No matching settings"),
            )));
            add_hint_line(&mut lines, width, &self.theme, self.search_enabled);
            return lines;
        }

        let max_label_w = self
            .items
            .iter()
            .map(|i| visible_width(&i.label))
            .max()
            .unwrap_or(0)
            .min(30);

        let start = (self.selected_index as isize - self.max_visible as isize / 2)
            .max(0)
            .min((display.len() as isize - self.max_visible as isize).max(0))
            as usize;
        let end = (start + self.max_visible).min(display.len());

        for i in start..end {
            let item = display[i];
            let selected = i == self.selected_index;
            let prefix = if selected {
                self.theme.cursor.clone()
            } else {
                "  ".to_string()
            };

            let label_padded = format!(
                "{}{}",
                item.label,
                " ".repeat(max_label_w.saturating_sub(visible_width(&item.label)))
            );

            let used = visible_width(&prefix) + max_label_w + visible_width("  ");
            let vmax = (width as usize).saturating_sub(used + 2).max(1);
            let truncated_value = truncate_to_width(&item.current_value, vmax);

            let mut spans: Vec<Span<'static>> = vec![
                Span::raw(prefix),
                Span::styled(
                    label_padded.clone(),
                    (self.theme.label)(&label_padded, selected),
                ),
                Span::raw("  "),
                Span::styled(
                    truncated_value.clone(),
                    (self.theme.value)(&truncated_value, selected),
                ),
            ];

            lines.push(Line::from(spans));
        }

        if start > 0 || end < display.len() {
            let scroll = format!("  ({}/{})", self.selected_index + 1, display.len());
            let truncated = truncate_to_width(&scroll, width.saturating_sub(2) as usize);
            let hint_style = (self.theme.hint)(&truncated);
            lines.push(Line::from(Span::styled(truncated, hint_style)));
        }

        if let Some(item) = display.get(self.selected_index) {
            if let Some(ref desc) = item.description {
                lines.push(Line::from(vec![]));
                let desc_width = (width as usize).saturating_sub(4);
                for wl in wrap_plain_text(desc, desc_width) {
                    let text = format!("  {}", wl);
                    let style = (self.theme.description)(&text);
                    lines.push(Line::from(Span::styled(text, style)));
                }
            }
        }

        add_hint_line(&mut lines, width, &self.theme, self.search_enabled);
        lines
    }

    fn handle_input(&mut self, event: &KeyEvent) {
        if self.search_enabled {
            let old_value = self
                .search_input
                .as_ref()
                .map(|i| i.get_value().to_string());
            if let Some(ref mut input) = self.search_input {
                input.handle_input(event);
            }
            let new_value = self
                .search_input
                .as_ref()
                .map(|i| i.get_value().to_string());
            if old_value.is_some() && new_value.is_some() && old_value != new_value {
                let nv = new_value.unwrap();
                self.apply_filter(&nv);
                return;
            }
        }

        let kb = get_keybindings();
        let display_len = self.display_items().len();
        if display_len == 0 {
            return;
        }

        if kb.matches(event, "selectUp") {
            self.selected_index = if self.selected_index == 0 {
                display_len - 1
            } else {
                self.selected_index - 1
            };
        } else if kb.matches(event, "selectDown") {
            self.selected_index = if self.selected_index >= display_len - 1 {
                0
            } else {
                self.selected_index + 1
            };
        } else if kb.matches(event, "confirm") || event.code == KeyCode::Char(' ') {
            self.activate_selected();
        } else if kb.matches(event, "cancel") {
            (self.on_cancel)();
        }
    }
}

impl SettingsList {
    fn activate_selected(&mut self) {
        let display = self.display_items();
        let item = match display.get(self.selected_index) {
            Some(i) => i,
            None => return,
        };

        if let Some(ref values) = item.values {
            if !values.is_empty() {
                let cur = &item.current_value;
                let ci = values.iter().position(|v| v == cur).unwrap_or(0);
                let ni = (ci + 1) % values.len();
                let nv = values[ni].clone();
                let id = item.id.clone();
                if let Some(target) = self.items.iter_mut().find(|i| i.id == id) {
                    target.current_value = nv.clone();
                }
                (self.on_change)(&id, &nv);
            }
        }
    }
}

fn add_hint_line(
    lines: &mut Vec<Line<'static>>,
    width: u16,
    theme: &SettingsListTheme,
    search: bool,
) {
    lines.push(Line::from(vec![]));
    let hint = if search {
        "  Type to search \u{b7} Enter/Space to change \u{b7} Esc to cancel"
    } else {
        "  Enter/Space to change \u{b7} Esc to cancel"
    };
    let truncated = truncate_to_width(hint, width as usize);
    let hint_style = (theme.hint)(&truncated);
    lines.push(Line::from(Span::styled(truncated, hint_style)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn make_items() -> Vec<SettingItem> {
        vec![
            SettingItem {
                id: "theme".into(),
                label: "Theme".into(),
                description: Some("Choose color theme".into()),
                current_value: "dark".into(),
                values: Some(vec!["dark".into(), "light".into()]),
            },
            SettingItem {
                id: "font_size".into(),
                label: "Font Size".into(),
                description: None,
                current_value: "14".into(),
                values: None,
            },
            SettingItem {
                id: "language".into(),
                label: "Language".into(),
                description: Some("Display language".into()),
                current_value: "English".into(),
                values: Some(vec!["English".into(), "\u{4e2d}\u{6587}".into()]),
            },
        ]
    }

    fn test_theme() -> SettingsListTheme {
        SettingsListTheme {
            label: Box::new(|_s, _selected| Style::default()),
            value: Box::new(|_s, _selected| Style::default()),
            description: Box::new(|_s| Style::default()),
            cursor: "> ".to_string(),
            hint: Box::new(|_s| Style::default()),
        }
    }

    fn default_options() -> SettingsListOptions {
        SettingsListOptions {
            enable_search: false,
        }
    }

    #[test]
    fn test_settings_list_creation() {
        let sl = SettingsList::new(
            make_items(),
            10,
            test_theme(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            default_options(),
        );
        let lines = sl.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_settings_list_navigation() {
        let mut sl = SettingsList::new(
            make_items(),
            10,
            test_theme(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            default_options(),
        );
        assert_eq!(sl.selected_index, 0);
        sl.handle_input(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(sl.selected_index, 1);
        sl.handle_input(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(sl.selected_index, 0);
    }

    #[test]
    fn test_settings_list_cycle_value() {
        use std::sync::Mutex;
        let changed_id: &'static Mutex<String> = Box::leak(Box::new(Mutex::new(String::new())));
        let changed_val: &'static Mutex<String> = Box::leak(Box::new(Mutex::new(String::new())));
        let mut sl = SettingsList::new(
            make_items(),
            10,
            test_theme(),
            Box::new(|id, val| {
                *changed_id.lock().unwrap() = id.to_string();
                *changed_val.lock().unwrap() = val.to_string();
            }),
            Box::new(|| {}),
            default_options(),
        );
        sl.activate_selected();
        assert_eq!(*changed_id.lock().unwrap(), "theme");
        assert_eq!(*changed_val.lock().unwrap(), "light");
    }

    #[test]
    fn test_settings_list_cancel() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let cancelled: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let mut sl = SettingsList::new(
            make_items(),
            10,
            test_theme(),
            Box::new(|_, _| {}),
            Box::new(|| {
                cancelled.store(true, Ordering::SeqCst);
            }),
            default_options(),
        );
        sl.handle_input(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn test_settings_list_update_value() {
        let mut sl = SettingsList::new(
            make_items(),
            10,
            test_theme(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            default_options(),
        );
        sl.handle_input(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(sl.selected_index, 2);
        sl.handle_input(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(sl.selected_index, 0);
    }

    #[test]
    fn test_settings_list_scroll_indicator() {
        let items: Vec<SettingItem> = (0..20)
            .map(|i| SettingItem {
                id: format!("item{}", i),
                label: format!("Item {}", i),
                description: None,
                current_value: "val".into(),
                values: None,
            })
            .collect();
        let sl = SettingsList::new(
            items,
            5,
            test_theme(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            default_options(),
        );
        let lines = sl.render(80);
        let has_scroll = lines.iter().any(|l| l.to_string().contains('/'));
        assert!(has_scroll);
    }

    #[test]
    fn test_settings_list_navigation_wraparound() {
        let mut sl = SettingsList::new(
            make_items(),
            10,
            test_theme(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            default_options(),
        );
        sl.handle_input(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(sl.selected_index, 2);
        sl.handle_input(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(sl.selected_index, 0);
    }

    #[test]
    fn test_settings_list_empty_description() {
        let items = vec![SettingItem {
            id: "test".into(),
            label: "Test".into(),
            description: None,
            current_value: "val".into(),
            values: None,
        }];
        let sl = SettingsList::new(
            items,
            10,
            test_theme(),
            Box::new(|_, _| {}),
            Box::new(|| {}),
            default_options(),
        );
        let lines = sl.render(80);
        assert!(!lines.is_empty());
    }
}
