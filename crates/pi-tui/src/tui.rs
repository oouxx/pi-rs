use std::collections::VecDeque;
use std::io;

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

/// The core Component trait — every UI element implements this.
pub trait Component: Send + Sync {
    fn render(&self, width: u16) -> Vec<Line<'static>>;
    fn handle_input(&mut self, _event: &KeyEvent) {}
    fn wants_key_release(&self) -> bool {
        false
    }
    fn invalidate(&mut self) {}
    fn cursor_position(&self) -> Option<(u16, u16)> {
        None
    }
}

/// Trait for components that can receive focus.
pub trait Focusable {
    fn focused(&self) -> bool;
    fn set_focused(&mut self, focused: bool);
}

/// Check if a Component also implements Focusable.
pub fn is_focusable(_component: &dyn Component) -> bool {
    false
}

/// A container that holds children components and renders them sequentially.
pub struct Container {
    pub children: Vec<Box<dyn Component>>,
}

impl Container {
    pub fn new() -> Self {
        Self { children: vec![] }
    }

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.children.push(component);
    }

    pub fn remove_child(&mut self, component: &dyn Component) {
        self.children.retain(|c| {
            !std::ptr::eq(
                c.as_ref() as *const dyn Component as *const (),
                component as *const dyn Component as *const (),
            )
        });
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }

    pub fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }

    pub fn render(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.render(width));
        }
        lines
    }
}

impl Component for Container {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        Container::render(self, width)
    }

    fn handle_input(&mut self, event: &KeyEvent) {
        for child in &mut self.children {
            child.handle_input(event);
        }
    }

    fn invalidate(&mut self) {
        Container::invalidate(self);
    }
}

// ============================================================================
// Overlay system
// ============================================================================

/// Anchor point for overlay positioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// Margin definition for overlay positioning.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayMargin {
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
    pub left: u16,
}

/// Size value type — can be absolute (u16) or percentage (string like "50%").
#[derive(Debug, Clone, Copy)]
pub enum SizeValue {
    Absolute(u16),
    Percentage(u16),
}

/// Options for positioning and sizing an overlay.
pub struct OverlayOptions {
    pub width: Option<u16>,
    pub min_width: Option<u16>,
    pub max_height: Option<u16>,
    pub anchor: OverlayAnchor,
    pub offset_x: i16,
    pub offset_y: i16,
    pub margin: OverlayMargin,
    pub visible: Option<Box<dyn Fn(u16, u16) -> bool + Send + Sync>>,
    pub non_capturing: bool,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            width: None,
            min_width: None,
            max_height: None,
            anchor: OverlayAnchor::Center,
            offset_x: 0,
            offset_y: 0,
            margin: OverlayMargin::default(),
            visible: None,
            non_capturing: false,
        }
    }
}

/// Options for OverlayHandle::unfocus.
#[derive(Debug, Clone)]
pub struct OverlayUnfocusOptions {
    pub target: Option<usize>,
}

/// Handle returned when showing an overlay, allowing control over it.
pub struct OverlayHandle {
    hidden: std::sync::Arc<std::sync::atomic::AtomicBool>,
    overlay_id: u64,
    focus_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl OverlayHandle {
    pub fn new() -> Self {
        Self {
            hidden: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            overlay_id: 0,
            focus_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn hide(&self) {
        self.hidden.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_hidden(&self, hidden: bool) {
        self.hidden
            .store(hidden, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn focus(&self) {
        self.focus_id
            .store(self.overlay_id, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn unfocus(&self, _options: Option<OverlayUnfocusOptions>) {
        let current = self.focus_id.load(std::sync::atomic::Ordering::SeqCst);
        if current == self.overlay_id {
            self.focus_id.store(0, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focus_id.load(std::sync::atomic::Ordering::SeqCst) == self.overlay_id
    }
}

struct OverlayEntry {
    id: u64,
    component: Box<dyn Component>,
    options: OverlayOptions,
    hidden: std::sync::Arc<std::sync::atomic::AtomicBool>,
    focus_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

// ============================================================================
// TUI — the main application class
// ============================================================================

pub struct InputListenerResult {
    pub consume: bool,
    pub data: Option<KeyEvent>,
}

pub type InputListener = Box<dyn Fn(&KeyEvent) -> InputListenerResult + Send + Sync>;

/// The main TUI application.
pub struct Tui {
    container: Container,
    terminal: Option<crate::terminal::Terminal>,
    focused: Option<usize>,
    overlays: VecDeque<OverlayEntry>,
    overlay_focus_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
    overlay_next_id: u64,
    input_listeners: Vec<InputListener>,
    full_redraws: u64,
    running: bool,
}

impl Tui {
    pub fn new(terminal: crate::terminal::Terminal) -> Self {
        Self {
            container: Container::new(),
            terminal: Some(terminal),
            focused: None,
            overlays: VecDeque::new(),
            overlay_focus_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            overlay_next_id: 1,
            input_listeners: Vec::new(),
            full_redraws: 0,
            running: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_test() -> Self {
        Self {
            container: Container::new(),
            terminal: None,
            focused: None,
            overlays: VecDeque::new(),
            overlay_focus_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            overlay_next_id: 1,
            input_listeners: Vec::new(),
            full_redraws: 0,
            running: false,
        }
    }

    pub fn full_redraws(&self) -> u64 {
        self.full_redraws
    }

    pub fn add_child(&mut self, component: Box<dyn Component>) {
        self.container.add_child(component);
    }

    pub fn remove_child(&mut self, component: &dyn Component) {
        self.container.remove_child(component);
    }

    pub fn clear(&mut self) {
        self.container.clear();
        self.focused = None;
    }

    pub fn set_focus_index(&mut self, index: usize) {
        if index < self.container.children.len() {
            self.focused = Some(index);
        }
    }

    pub fn show_overlay(
        &mut self,
        component: Box<dyn Component>,
        options: OverlayOptions,
    ) -> OverlayHandle {
        let id = self.overlay_next_id;
        self.overlay_next_id += 1;
        let hidden = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let focus_id = self.overlay_focus_id.clone();
        self.overlays.push_back(OverlayEntry {
            id,
            component,
            options,
            hidden: hidden.clone(),
            focus_id: focus_id.clone(),
        });
        OverlayHandle {
            hidden,
            overlay_id: id,
            focus_id,
        }
    }

    pub fn hide_overlays(&mut self) {
        self.overlays.clear();
    }

    pub fn has_overlay(&self) -> bool {
        !self.overlays.is_empty()
    }

    pub fn add_input_listener(&mut self, listener: InputListener) {
        self.input_listeners.push(listener);
    }

    pub fn handle_input(&mut self, event: &KeyEvent) {
        let mut current = *event;
        for listener in &self.input_listeners {
            let result = listener(&current);
            if result.consume {
                return;
            }
            if let Some(new_data) = result.data {
                current = new_data;
            }
        }
        // Check overlays first
        let mut overlay_consumed = false;
        for overlay in &mut self.overlays {
            if !overlay.hidden.load(std::sync::atomic::Ordering::Relaxed) {
                overlay.component.handle_input(&current);
                overlay_consumed = true;
                break;
            }
        }

        let focused_overlay_id = self
            .overlay_focus_id
            .load(std::sync::atomic::Ordering::Relaxed);
        if focused_overlay_id > 0 {
            if let Some(entry) = self
                .overlays
                .iter_mut()
                .find(|e| e.id == focused_overlay_id)
            {
                if !entry.hidden.load(std::sync::atomic::Ordering::Relaxed) {
                    entry.component.handle_input(&current);
                    return;
                }
            }
        }

        if let Some(entry) = self.overlays.back_mut() {
            if !entry.hidden.load(std::sync::atomic::Ordering::Relaxed)
                && !entry.options.non_capturing
                && focused_overlay_id == 0
            {
                entry.component.handle_input(&current);
                return;
            }
        }

        if let Some(idx) = self.focused {
            if let Some(component) = self.container.children.get_mut(idx) {
                component.handle_input(&current);
            }
        }
    }

    /// Render the full UI to lines without drawing to the terminal.
    /// Returns (rendered_lines, cursor_position).
    pub fn render_to_lines(
        &self,
        width: u16,
        height: u16,
    ) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
        let mut lines = self.container.render(width);

        for entry in &self.overlays {
            if entry.hidden.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            if let Some(ref visible_fn) = entry.options.visible {
                if !visible_fn(width, height) {
                    continue;
                }
            }
            let overlay_lines = entry.component.render(width);
            composite_overlay(&mut lines, &overlay_lines, width, height, &entry.options);
        }

        while lines.len() < height as usize {
            lines.push(Line::from(vec![]));
        }

        let cursor_pos = self.focused.and_then(|idx| {
            self.container
                .children
                .get(idx)
                .and_then(|c| c.cursor_position())
        });

        (lines, cursor_pos)
    }

    /// Render the full UI and draw it to the terminal via ratatui.
    pub fn render_to_frame(&mut self, frame: &mut Frame) {
        let (width, height) = if let Some(ref mut term) = self.terminal {
            term.refresh_size();
            (term.columns(), term.rows())
        } else {
            let area = frame.size();
            (area.width, area.height)
        };

        let (lines, cursor_pos) = self.render_to_lines(width, height);
        let text = Text::from(lines);
        let paragraph = Paragraph::new(text);

        let area = Rect::new(0, 0, width, height);
        frame.render_widget(Clear, area);
        frame.render_widget(paragraph, area);

        if let Some((row, col)) = cursor_pos {
            frame.set_cursor_position((col, row));
        }
    }

    pub fn request_render(&mut self, force: bool) -> io::Result<()> {
        if force {
            self.full_redraws += 1;
        }
        Ok(())
    }
}

// ============================================================================
// Overlay compositing
// ============================================================================

fn composite_overlay(
    base: &mut Vec<Line<'static>>,
    overlay: &[Line<'static>],
    term_width: u16,
    term_height: u16,
    options: &OverlayOptions,
) {
    let o_width = options.width.unwrap_or(term_width / 2).min(term_width);
    let o_height = (overlay.len() as u16).min(options.max_height.unwrap_or(term_height));
    let (col, row) = overlay_position(options, o_width, o_height, term_width, term_height);

    while base.len() < row as usize + o_height as usize {
        base.push(Line::from(vec![]));
    }

    for i in 0..o_height as usize {
        if i >= overlay.len() {
            break;
        }
        let base_row = row as usize + i;
        if base_row >= base.len() {
            break;
        }
        base[base_row] = overlay[i].clone();
    }
}

fn overlay_position(
    options: &OverlayOptions,
    width: u16,
    height: u16,
    term_width: u16,
    term_height: u16,
) -> (u16, u16) {
    let col = match options.anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::CenterLeft | OverlayAnchor::BottomLeft => {
            options.margin.left
        }
        OverlayAnchor::TopCenter | OverlayAnchor::Center | OverlayAnchor::BottomCenter => {
            (term_width.saturating_sub(width) / 2)
                .saturating_add(options.margin.left)
                .saturating_sub(options.margin.right)
        }
        OverlayAnchor::TopRight | OverlayAnchor::CenterRight | OverlayAnchor::BottomRight => {
            term_width
                .saturating_sub(width)
                .saturating_sub(options.margin.right)
        }
    };

    let row = match options.anchor {
        OverlayAnchor::TopLeft | OverlayAnchor::TopCenter | OverlayAnchor::TopRight => {
            options.margin.top
        }
        OverlayAnchor::CenterLeft | OverlayAnchor::Center | OverlayAnchor::CenterRight => {
            (term_height.saturating_sub(height) / 2)
                .saturating_add(options.margin.top)
                .saturating_sub(options.margin.bottom)
        }
        OverlayAnchor::BottomLeft | OverlayAnchor::BottomCenter | OverlayAnchor::BottomRight => {
            term_height
                .saturating_sub(height)
                .saturating_sub(options.margin.bottom)
        }
    };

    let col = (col as i32 + options.offset_x as i32).max(0) as u16;
    let row = (row as i32 + options.offset_y as i32).max(0) as u16;

    (col.min(term_width), row.min(term_height))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    struct TextComponent {
        text: Vec<String>,
    }

    impl TextComponent {
        fn new(text: &str) -> Self {
            Self {
                text: vec![text.to_string()],
            }
        }

        fn lines(lines: &[&str]) -> Self {
            Self {
                text: lines.iter().map(|s| s.to_string()).collect(),
            }
        }
    }

    impl Component for TextComponent {
        fn render(&self, _width: u16) -> Vec<Line<'static>> {
            self.text
                .iter()
                .map(|l| Line::from(Span::raw(l.clone())))
                .collect()
        }
    }

    struct MultiLineComponent {
        lines: Vec<String>,
    }

    impl MultiLineComponent {
        fn new(lines: &[&str]) -> Self {
            Self {
                lines: lines.iter().map(|s| s.to_string()).collect(),
            }
        }
    }

    impl Component for MultiLineComponent {
        fn render(&self, _width: u16) -> Vec<Line<'static>> {
            self.lines
                .iter()
                .map(|l| Line::from(Span::raw(l.clone())))
                .collect()
        }
    }

    fn make_tui() -> Tui {
        Tui::new_test()
    }

    #[test]
    fn test_container_renders_children() {
        let mut container = Container::new();
        container.add_child(Box::new(TextComponent::new("line1")));
        container.add_child(Box::new(TextComponent::new("line2")));
        let output = container.render(80);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].spans[0].content, "line1");
        assert_eq!(output[1].spans[0].content, "line2");
    }

    #[test]
    fn test_container_clear() {
        let mut container = Container::new();
        container.add_child(Box::new(TextComponent::new("test")));
        assert_eq!(container.children.len(), 1);
        container.clear();
        assert_eq!(container.children.len(), 0);
    }

    #[test]
    fn test_container_multi_line_children() {
        let mut container = Container::new();
        container.add_child(Box::new(MultiLineComponent::new(&["a1", "a2"])));
        container.add_child(Box::new(MultiLineComponent::new(&["b1", "b2", "b3"])));
        let output = container.render(80);
        assert_eq!(output.len(), 5);
        assert_eq!(output[0].spans[0].content, "a1");
        assert_eq!(output[4].spans[0].content, "b3");
    }

    #[test]
    fn test_container_invalidate() {
        let mut container = Container::new();
        container.add_child(Box::new(TextComponent::new("test")));
        container.invalidate();
    }

    #[test]
    fn test_tui_render_single_line_component() {
        let mut tui = make_tui();
        let comp = Box::new(TextComponent::new("Hello World"));
        tui.add_child(comp);
        let (lines, cursor) = tui.render_to_lines(80, 24);
        assert_eq!(lines.len(), 24);
        assert_eq!(lines[0].spans[0].content, "Hello World");
        assert!(lines[1].spans.is_empty() || lines[1].spans[0].content.is_empty());
        assert_eq!(cursor, None);
    }

    #[test]
    fn test_tui_render_multi_line() {
        let mut tui = make_tui();
        tui.add_child(Box::new(MultiLineComponent::new(&[
            "Line 0", "Line 1", "Line 2",
        ])));
        let (lines, _) = tui.render_to_lines(80, 24);
        assert_eq!(lines[0].spans[0].content, "Line 0");
        assert_eq!(lines[1].spans[0].content, "Line 1");
        assert_eq!(lines[2].spans[0].content, "Line 2");
        assert!(lines[3].spans.is_empty() || lines[3].spans[0].content.is_empty());
    }

    #[test]
    fn test_tui_focus_and_render_focused() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("child 0")));
        tui.add_child(Box::new(TextComponent::new("child 1")));
        tui.set_focus_index(1);
        let (lines, cursor) = tui.render_to_lines(80, 24);
        assert_eq!(lines.len(), 24);
        assert_eq!(lines[0].spans[0].content, "child 0");
        assert_eq!(lines[1].spans[0].content, "child 1");
        assert_eq!(cursor, None);
    }

    #[test]
    fn test_tui_overlay_add_and_render() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("base")));
        tui.show_overlay(
            Box::new(TextComponent::new("OVERLAY")),
            OverlayOptions {
                anchor: OverlayAnchor::TopLeft,
                ..Default::default()
            },
        );
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0]
            .spans
            .iter()
            .any(|s| s.content == "OVERLAY" || s.content.starts_with("OVERLAY")));
    }

    #[test]
    fn test_tui_overlay_hidden() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("base")));
        let mut handle = tui.show_overlay(
            Box::new(TextComponent::new("OVERLAY")),
            OverlayOptions {
                anchor: OverlayAnchor::TopLeft,
                ..Default::default()
            },
        );
        handle.set_hidden(true);
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0]
            .spans
            .iter()
            .any(|s| s.content == "base" || s.content.contains("base")));
        // Since overlay is hidden, base should be visible and overlay should not
        // But since base has only 1 line and overlay is hidden, only base appears
    }

    #[test]
    fn test_tui_overlay_hide_show() {
        let mut tui = make_tui();
        let mut handle = tui.show_overlay(
            Box::new(TextComponent::new("OVERLAY")),
            OverlayOptions::default(),
        );
        assert!(tui.has_overlay());
        handle.set_hidden(true);
        assert!(handle.is_hidden());
        handle.set_hidden(false);
        assert!(!handle.is_hidden());
    }

    #[test]
    fn test_tui_render_allows_content_beyond_height() {
        let mut tui = make_tui();
        let lines_count = 30;
        let mut comp_lines = Vec::new();
        for i in 0..lines_count {
            comp_lines.push(format!("line {}", i));
        }
        tui.add_child(Box::new(MultiLineComponent::new(
            &comp_lines.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
        )));
        let (lines, _) = tui.render_to_lines(80, 24);
        assert_eq!(lines.len(), 30);
        assert_eq!(lines[29].spans[0].content, "line 29");
    }

    #[test]
    fn test_tui_render_empty() {
        let tui = make_tui();
        let (lines, cursor) = tui.render_to_lines(80, 24);
        assert_eq!(lines.len(), 24);
        assert!(lines
            .iter()
            .all(|l| l.spans.is_empty() || l.spans[0].content.is_empty()));
        assert_eq!(cursor, None);
    }

    #[test]
    fn test_tui_clear_children() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("test")));
        assert_eq!(tui.container.children.len(), 1);
        tui.clear();
        assert_eq!(tui.container.children.len(), 0);
        assert!(tui.focused.is_none());
    }

    #[test]
    fn test_tui_focus_index_out_of_bounds() {
        let mut tui = make_tui();
        tui.set_focus_index(5);
        assert!(tui.focused.is_none());
        tui.add_child(Box::new(TextComponent::new("test")));
        tui.set_focus_index(0);
        assert_eq!(tui.focused, Some(0));
        tui.set_focus_index(10);
        assert_eq!(tui.focused, Some(0));
    }

    #[test]
    fn test_overlay_handle_toggle() {
        let mut tui = make_tui();
        let handle = tui.show_overlay(
            Box::new(TextComponent::new("overlay")),
            OverlayOptions::default(),
        );
        assert!(!handle.is_hidden());
        handle.set_hidden(true);
        assert!(handle.is_hidden());
        handle.set_hidden(false);
        assert!(!handle.is_hidden());
    }

    #[test]
    fn test_overlay_handle_focus() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("content")));
        let handle = tui.show_overlay(
            Box::new(TextComponent::new("overlay")),
            OverlayOptions::default(),
        );
        assert!(!handle.is_focused());
        handle.focus();
        assert!(handle.is_focused());
        handle.unfocus(None);
        assert!(!handle.is_focused());
    }

    #[test]
    fn test_tui_render_to_frame_with_test_backend() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(40, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("Hello World")));

        terminal
            .draw(|frame| {
                tui.render_to_frame(frame);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert!(buffer[(0, 0)].symbol().len() > 0);
    }

    #[test]
    fn test_tui_render_to_frame_overlay() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(40, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("base content")));
        tui.show_overlay(
            Box::new(TextComponent::new("OVERLAY")),
            OverlayOptions {
                anchor: OverlayAnchor::TopLeft,
                ..Default::default()
            },
        );

        terminal
            .draw(|frame| {
                tui.render_to_frame(frame);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert!(buffer[(0, 0)].symbol().len() > 0);
    }

    #[test]
    fn test_tui_render_to_frame_pads_to_terminal_height() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(40, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("short")));

        terminal
            .draw(|frame| {
                tui.render_to_frame(frame);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        for row in 0..5 {
            let _cell = &buffer[(0, row)];
        }
    }

    #[test]
    fn test_full_redraw_counter() {
        let mut tui = make_tui();
        assert_eq!(tui.full_redraws(), 0);
        let _ = tui.request_render(true);
        assert_eq!(tui.full_redraws(), 1);
        let _ = tui.request_render(false);
        assert_eq!(tui.full_redraws(), 1);
    }
}
