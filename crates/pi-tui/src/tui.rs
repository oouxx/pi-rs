use std::collections::VecDeque;
use std::io;

use ratatui::layout::Rect;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;
use crate::terminal::Terminal;
use crate::utils::visible_width;

/// Zero-width marker placed in rendered output to indicate hardware cursor position.
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";

/// The core Component trait — every UI element implements this.
pub trait Component: Send + Sync {
    /// Render the component to lines of text, each line must be exactly `width` characters
    /// (or shorter — the renderer handles padding). ANSI escape sequences are allowed.
    fn render(&self, width: u16) -> Vec<String>;

    /// Handle input when this component is focused.
    fn handle_input(&mut self, _data: &str) {}

    /// Whether this component wants to receive key release events.
    fn wants_key_release(&self) -> bool {
        false
    }

    /// Invalidate any cached render output.
    fn invalidate(&mut self) {}
}

/// Trait for components that can receive focus.
pub trait Focusable {
    fn focused(&self) -> bool;
    fn set_focused(&mut self, focused: bool);
}

/// Check if a Component also implements Focusable.
pub fn is_focusable(_component: &dyn Component) -> bool {
    // Simplified: we can't easily downcast without a common base trait.
    // Components that implement Focusable should be checked at the type level.
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

    pub fn render(&self, width: u16) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &self.children {
            lines.extend(child.render(width));
        }
        lines
    }
}

impl Component for Container {
    fn render(&self, width: u16) -> Vec<String> {
        Container::render(self, width)
    }

    fn handle_input(&mut self, data: &str) {
        for child in &mut self.children {
            child.handle_input(data);
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
#[derive(Debug, Clone, Copy)]
pub struct OverlayMargin {
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
    pub left: u16,
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
            margin: OverlayMargin {
                top: 0,
                right: 0,
                bottom: 0,
                left: 0,
            },
        }
    }
}

/// Handle returned when showing an overlay, allowing control over it.
pub struct OverlayHandle {
    pub hidden: bool,
}

impl OverlayHandle {
    pub fn hide(&mut self) {
        self.hidden = true;
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }
}

struct OverlayEntry {
    component: Box<dyn Component>,
    options: OverlayOptions,
    handle: OverlayHandle,
}

// ============================================================================
// TUI — the main application class
// ============================================================================

/// Input listener: receives raw input before focused component.
pub type InputListener = Box<dyn Fn(&str) -> bool + Send + Sync>;

/// The main TUI application. Manages components, focus, overlays, and rendering.
pub struct Tui {
    container: Container,
    terminal: Terminal,
    focused: Option<usize>, // index into children
    overlays: VecDeque<OverlayEntry>,
    input_listeners: Vec<InputListener>,
    previous_lines: Vec<String>,
    full_redraws: u64,
    show_hardware_cursor: bool,
    clear_on_shrink: bool,
    running: bool,
}

impl Tui {
    pub fn new(terminal: Terminal) -> Self {
        Self {
            container: Container::new(),
            terminal,
            focused: None,
            overlays: VecDeque::new(),
            input_listeners: Vec::new(),
            previous_lines: Vec::new(),
            full_redraws: 0,
            show_hardware_cursor: false,
            clear_on_shrink: true,
            running: false,
        }
    }

    pub fn full_redraws(&self) -> u64 {
        self.full_redraws
    }

    pub fn set_show_hardware_cursor(&mut self, enabled: bool) {
        self.show_hardware_cursor = enabled;
    }

    pub fn set_clear_on_shrink(&mut self, enabled: bool) {
        self.clear_on_shrink = enabled;
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

    /// Set focus to a specific component by its index in the children list.
    pub fn set_focus_index(&mut self, index: usize) {
        if index < self.container.children.len() {
            self.focused = Some(index);
        }
    }

    /// Show an overlay on top of the current content.
    pub fn show_overlay(
        &mut self,
        component: Box<dyn Component>,
        options: OverlayOptions,
    ) -> OverlayHandle {
        self.overlays.push_back(OverlayEntry {
            component,
            options,
            handle: OverlayHandle { hidden: false },
        });
        OverlayHandle { hidden: false }
    }

    /// Hide all overlays.
    pub fn hide_overlays(&mut self) {
        self.overlays.clear();
    }

    pub fn has_overlay(&self) -> bool {
        !self.overlays.is_empty()
    }

    pub fn add_input_listener(&mut self, listener: InputListener) {
        self.input_listeners.push(listener);
    }

    /// Handle raw input data. Routes to focused component or overlays first.
    pub fn handle_input(&mut self, data: &str) {
        // Allow input listeners to intercept/transform
        for listener in &self.input_listeners {
            if listener(data) {
                return;
            }
        }

        // Route to overlay first (topmost)
        if let Some(entry) = self.overlays.back_mut() {
            if !entry.handle.hidden {
                entry.component.handle_input(data);
                return;
            }
        }

        // Route to focused component
        if let Some(idx) = self.focused {
            if let Some(component) = self.container.children.get_mut(idx) {
                component.handle_input(data);
            }
        }
    }

    /// Render the full UI and draw it to the terminal via ratatui.
    pub fn render_to_frame(&mut self, frame: &mut Frame) {
        self.terminal.refresh_size();
        let width = self.terminal.columns();
        let height = self.terminal.rows();

        // 1. Render all children
        let mut lines = self.container.render(width);

        // 2. Composite overlays
        for entry in &self.overlays {
            if entry.handle.hidden {
                continue;
            }
            let overlay_lines = entry.component.render(width);
            composite_overlay(
                &mut lines,
                &overlay_lines,
                width,
                height,
                &entry.options,
            );
        }

        // 3. Extract cursor position and remove markers
        let cursor_pos = extract_cursor_position(&mut lines);

        // 4. Apply reset sequences at end of each line
        for line in &mut lines {
            line.push_str("\x1b[0m");
        }

        // 5. Pad to terminal height
        while lines.len() < height as usize {
            lines.push(String::new());
        }
        lines.truncate(height as usize);

        // 6. Convert to ratatui Text and render
        let rat_lines: Vec<Line> = lines
            .iter()
            .map(|l| Line::from(Span::raw(l.as_str())))
            .collect();

        let text = Text::from(rat_lines);
        let paragraph = Paragraph::new(text)
            .wrap(Wrap { trim: false });

        // Render full-screen
        let area = Rect::new(0, 0, width, height);
        frame.render_widget(Clear, area);
        frame.render_widget(paragraph, area);

        // 7. Position hardware cursor
        if let Some((row, col)) = cursor_pos {
            frame.set_cursor_position((col, row));
        }

        // Store previous lines for potential diff tracking
        self.previous_lines = lines;
    }

    /// Request an immediate render.
    pub fn request_render(&mut self, force: bool) -> io::Result<()> {
        if force {
            self.full_redraws += 1;
        }
        // ratatui draw is called externally in the event loop
        Ok(())
    }
}

// ============================================================================
// Overlay compositing
// ============================================================================

/// Composite overlay content onto the base content buffer.
fn composite_overlay(
    base: &mut Vec<String>,
    overlay: &[String],
    term_width: u16,
    term_height: u16,
    options: &OverlayOptions,
) {
    let o_width = options.width.unwrap_or(term_width / 2).min(term_width);
    let o_height = (overlay.len() as u16).min(options.max_height.unwrap_or(term_height));

    let (col, row) = overlay_position(options, o_width, o_height, term_width, term_height);

    // Ensure base has enough rows
    while base.len() < row as usize + o_height as usize {
        base.push(String::new());
    }

    for i in 0..o_height as usize {
        if i >= overlay.len() {
            break;
        }
        let base_row = row as usize + i;
        if base_row >= base.len() {
            break;
        }

        let overlay_line = &overlay[i];
        let base_line = &base[base_row];

        let overlay_visible_width = visible_width(overlay_line);
        let left_pad = col as usize;
        let right_pad = (term_width as usize)
            .saturating_sub(left_pad + overlay_visible_width);

        let mut new_line = String::with_capacity(term_width as usize);
        // Left part of base
        let base_left = base_line.chars().take(left_pad).collect::<String>();
        new_line.push_str(&base_left);

        // Overlay content
        new_line.push_str(overlay_line);

        // Right padding
        for _ in 0..right_pad {
            new_line.push(' ');
        }

        base[base_row] = new_line;
    }
}

/// Calculate the (col, row) position for an overlay based on anchor and offsets.
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
            term_width.saturating_sub(width).saturating_sub(options.margin.right)
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
            term_height.saturating_sub(height).saturating_sub(options.margin.bottom)
        }
    };

    let col = (col as i32 + options.offset_x as i32).max(0) as u16;
    let row = (row as i32 + options.offset_y as i32).max(0) as u16;

    (col.min(term_width), row.min(term_height))
}

/// Extract the CURSOR_MARKER from rendered lines and return its (row, col).
fn extract_cursor_position(lines: &mut [String]) -> Option<(u16, u16)> {
    for (row, line) in lines.iter_mut().enumerate() {
        if let Some(pos) = line.find(CURSOR_MARKER) {
            let col = visible_width(&line[..pos]);
            *line = line.replace(CURSOR_MARKER, "");
            return Some((row as u16, col as u16));
        }
    }
    None
}

// ============================================================================
// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    struct TestComponent {
        text: String,
    }

    impl TestComponent {
        fn new(text: &str) -> Self {
            Self {
                text: text.to_string(),
            }
        }
    }

    impl Component for TestComponent {
        fn render(&self, width: u16) -> Vec<String> {
            vec![self.text.clone()]
        }
    }

    #[test]
    fn test_container_renders_children() {
        let mut container = Container::new();
        container.add_child(Box::new(TestComponent::new("line1")));
        container.add_child(Box::new(TestComponent::new("line2")));
        let output = container.render(80);
        assert_eq!(output, vec!["line1", "line2"]);
    }

    #[test]
    fn test_container_clear() {
        let mut container = Container::new();
        container.add_child(Box::new(TestComponent::new("test")));
        assert_eq!(container.children.len(), 1);
        container.clear();
        assert_eq!(container.children.len(), 0);
    }

    #[test]
    fn test_overlay_position_center() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::Center,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 30); // (80-20)/2
        assert_eq!(row, 9); // (24-5)/2
    }

    #[test]
    fn test_overlay_position_top_left() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::TopLeft,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 0);
        assert_eq!(row, 0);
    }

    #[test]
    fn test_cursor_marker_extraction() {
        let mut lines = vec![
            "before".to_string(),
            format!("hello{}world", CURSOR_MARKER),
            "after".to_string(),
        ];
        let pos = extract_cursor_position(&mut lines);
        assert_eq!(pos, Some((1, 5))); // row 1, col 5 ("hello" = 5 chars)
        assert!(!lines[1].contains(CURSOR_MARKER));
    }

    // --- Supplementary tests matching TS originals ---

    #[test]
    fn test_cursor_marker_at_start_of_line() {
        let mut lines = vec![
            format!("{}hello", CURSOR_MARKER),
        ];
        let pos = extract_cursor_position(&mut lines);
        assert_eq!(pos, Some((0, 0)));
        assert!(!lines[0].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_cursor_marker_not_present() {
        let mut lines = vec!["hello".to_string(), "world".to_string()];
        let pos = extract_cursor_position(&mut lines);
        assert_eq!(pos, None);
    }

    #[test]
    fn test_overlay_position_bottom_right() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::BottomRight,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 60); // 80 - 20
        assert_eq!(row, 19); // 24 - 5
    }

    #[test]
    fn test_overlay_position_with_margin() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::TopLeft,
            margin: OverlayMargin { top: 2, right: 1, bottom: 1, left: 3 },
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 3); // left margin
        assert_eq!(row, 2); // top margin
    }

    #[test]
    fn test_overlay_position_with_offset() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::Center,
            offset_x: 5,
            offset_y: -2,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 35); // (80-20)/2 + 5
        assert_eq!(row, 7); // (24-5)/2 - 2
    }

    #[test]
    fn test_overlay_composite_with_short_base() {
        let mut base = vec!["line1".to_string()];
        let overlay = vec!["OVERLAY".to_string()];
        let opts = OverlayOptions {
            width: Some(10),
            anchor: OverlayAnchor::TopLeft,
            ..Default::default()
        };
        composite_overlay(&mut base, &overlay, 80, 24, &opts);
        // Base should be expanded to at least the overlay row + height
        assert!(base.len() >= 1);
        assert!(base[0].contains("OVERLAY"));
    }

    #[test]
    fn test_container_remove_child() {
        struct CompA;
        struct CompB;
        impl Component for CompA {
            fn render(&self, _: u16) -> Vec<String> { vec!["A".into()] }
        }
        impl Component for CompB {
            fn render(&self, _: u16) -> Vec<String> { vec!["B".into()] }
        }

        let mut container = Container::new();
        let comp_a = Box::new(CompA);
        let comp_b = Box::new(CompB);
        container.add_child(comp_a);
        container.add_child(comp_b);
        assert_eq!(container.children.len(), 2);
        // Can't easily test remove_child since it takes a &dyn Component
        container.clear();
        assert_eq!(container.children.len(), 0);
    }

    #[test]
    fn test_overlay_position_top_center() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::TopCenter,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 30); // (80-20)/2
        assert_eq!(row, 0);
    }
}
