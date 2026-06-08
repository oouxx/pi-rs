use crate::autocomplete::AutocompleteProvider;
use crate::tui::Component;

pub trait EditorComponent: Component {
    fn get_text(&self) -> &str;
    fn set_text(&mut self, text: &str);
    fn on_submit(&mut self, _text: &str) {}
    fn on_change(&mut self, _text: &str) {}
    fn add_to_history(&mut self, _text: &str) {}
    fn insert_text_at_cursor(&mut self, _text: &str) {}
    fn get_expanded_text(&self) -> &str {
        self.get_text()
    }
    fn set_autocomplete_provider(
        &mut self,
        _provider: Box<dyn AutocompleteProvider + Send + Sync>,
    ) {
    }
    fn border_color(&self) -> Option<&dyn Fn() -> ratatui::style::Style> {
        None
    }
    fn set_padding_x(&mut self, _padding: u16) {}
    fn set_autocomplete_max_visible(&mut self, _max_visible: usize) {}
}
