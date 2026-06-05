use crossterm::event::KeyEvent;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::Terminal;

use crate::tui::Component;

pub use ratkit::widgets::ai_chat::{AIChatEvent, Message, MessageRole, MessageStore};

pub struct AIChatComponent {
    inner: ratkit::widgets::ai_chat::AIChat,
    last_event: Option<AIChatEvent>,
}

impl AIChatComponent {
    pub fn new() -> Self {
        Self {
            inner: ratkit::widgets::ai_chat::AIChat::new(),
            last_event: None,
        }
    }

    pub fn take_event(&mut self) -> Option<AIChatEvent> {
        self.last_event.take()
    }

    pub fn add_message(&mut self, message: Message) {
        self.inner.messages_mut().add(message);
    }

    pub fn messages(&self) -> &MessageStore {
        self.inner.messages()
    }

    pub fn messages_mut(&mut self) -> &mut MessageStore {
        self.inner.messages_mut()
    }

    pub fn set_loading(&mut self, loading: bool) {
        self.inner.set_loading(loading);
    }

    pub fn is_loading(&self) -> bool {
        self.inner.is_loading()
    }

    pub fn inner(&self) -> &ratkit::widgets::ai_chat::AIChat {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut ratkit::widgets::ai_chat::AIChat {
        &mut self.inner
    }
}

impl Default for AIChatComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for AIChatComponent {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        if width == 0 {
            return vec![Line::from(vec![])];
        }

        let height = 1000u16;
        let backend = TestBackend::new(width, height);
        let mut terminal =
            Terminal::new(backend).expect("AIChatComponent::render: TestTerminal creation failed");

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, width, frame.area().height);
                self.inner.render(frame, area);
            })
            .ok();

        let buffer = terminal.backend().buffer().clone();
        drop(terminal);

        buffer_to_lines(&buffer, width)
    }

    fn handle_input(&mut self, event: &KeyEvent) {
        let result = self.inner.handle_key(event.code);
        if result != AIChatEvent::None {
            self.last_event = Some(result);
        }
    }
}

fn buffer_to_lines(buf: &Buffer, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut lines = Vec::new();

    for row in buf.content.chunks(buf.area.width as usize) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut i = 0;

        while i < w {
            let cell = &row[i];

            if cell.symbol().is_empty() {
                i += 1;
                continue;
            }

            let style = cell.style();
            let mut text = String::new();
            text.push_str(cell.symbol());
            i += 1;

            while i < w {
                let next = &row[i];
                if next.symbol().is_empty() {
                    i += 1;
                    continue;
                }
                if next.style() != style {
                    break;
                }
                text.push_str(next.symbol());
                i += 1;
            }

            spans.push(Span::styled(text, style));
        }

        while let Some(last) = spans.last() {
            if last.content.trim().is_empty() {
                spans.pop();
            } else {
                break;
            }
        }

        lines.push(Line::from(spans));
    }

    while let Some(last) = lines.last() {
        if last.spans.is_empty() {
            lines.pop();
        } else {
            break;
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(vec![]));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_renders_something() {
        let chat = AIChatComponent::new();
        let lines = chat.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_zero_width_returns_empty_line() {
        let chat = AIChatComponent::new();
        let lines = chat.render(0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.is_empty());
    }

    #[test]
    fn test_add_message_appears_in_render() {
        let mut chat = AIChatComponent::new();
        chat.add_message(Message::user("Hello from test".to_string()));
        let lines = chat.render(80);
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            all_text.contains("Hello from test"),
            "message text should appear in rendered output, got: {all_text:?}"
        );
    }

    #[test]
    fn test_handle_key_submit_produces_event() {
        let mut chat = AIChatComponent::new();

        chat.handle_input(&KeyEvent::new(
            crossterm::event::KeyCode::Char('h'),
            crossterm::event::KeyModifiers::NONE,
        ));
        chat.handle_input(&KeyEvent::new(
            crossterm::event::KeyCode::Char('i'),
            crossterm::event::KeyModifiers::NONE,
        ));

        chat.handle_input(&KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));

        let event = chat.take_event();
        assert_eq!(event, Some(AIChatEvent::MessageSubmitted("hi".to_string())));
    }

    #[test]
    fn test_loading_state() {
        let mut chat = AIChatComponent::new();
        assert!(!chat.is_loading());
        chat.set_loading(true);
        assert!(chat.is_loading());
        let _lines = chat.render(80);
    }
}
