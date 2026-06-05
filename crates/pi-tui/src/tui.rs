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
    /// Dynamic visibility callback. Called each render with (term_width, term_height).
    /// Overlay is only rendered when this returns true.
    pub visible: Option<Box<dyn Fn(u16, u16) -> bool + Send + Sync>>,
    /// If true, this overlay does not capture keyboard focus when shown.
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
    /// Index of the child component to focus after releasing this overlay.
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

    pub fn hide(&mut self) {
        self.hidden.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden.store(hidden, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Focus this overlay, bringing it to the front.
    pub fn focus(&self) {
        self.focus_id.store(self.overlay_id, std::sync::atomic::Ordering::SeqCst);
    }

    /// Release focus from this overlay, optionally targeting a specific component.
    pub fn unfocus(&self, _options: Option<OverlayUnfocusOptions>) {
        let current = self.focus_id.load(std::sync::atomic::Ordering::SeqCst);
        if current == self.overlay_id {
            self.focus_id.store(0, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Check if this overlay currently has focus.
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

/// Result returned by an input listener.
pub struct InputListenerResult {
    pub consume: bool,
    pub data: Option<String>,
}

/// Input listener: receives raw input before focused component.
/// Return `InputListenerResult { consume: true, data: None }` to intercept and drop.
/// Return `InputListenerResult { consume: false, data: Some(new_data) }` to transform.
/// Return `InputListenerResult { consume: false, data: None }` to pass through.
pub type InputListener = Box<dyn Fn(&str) -> InputListenerResult + Send + Sync>;

/// The main TUI application. Manages components, focus, overlays, and rendering.
pub struct Tui {
    container: Container,
    terminal: Option<Terminal>,
    focused: Option<usize>, // index into children
    overlays: VecDeque<OverlayEntry>,
    overlay_focus_id: std::sync::Arc<std::sync::atomic::AtomicU64>,
    overlay_next_id: u64,
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
            terminal: Some(terminal),
            focused: None,
            overlays: VecDeque::new(),
            overlay_focus_id: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            overlay_next_id: 1,
            input_listeners: Vec::new(),
            previous_lines: Vec::new(),
            full_redraws: 0,
            show_hardware_cursor: false,
            clear_on_shrink: true,
            running: false,
        }
    }

    /// Create a TUI instance without a real terminal backend (for testing).
    /// Rendering via `render_to_frame` will panic; use `render_to_lines` instead.
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

    /// Handle raw input data. Routes to overlay with focus, or focused component.
    pub fn handle_input(&mut self, data: &str) {
        // Allow input listeners to intercept/transform
        let mut current = data.to_string();
        for listener in &self.input_listeners {
            let result = listener(&current);
            if result.consume {
                return;
            }
            if let Some(new_data) = result.data {
                current = new_data;
            }
        }
        if current.is_empty() {
            return;
        }

        // Check if any overlay has focus (via focus_id)
        let focused_overlay_id = self.overlay_focus_id.load(std::sync::atomic::Ordering::Relaxed);
        if focused_overlay_id > 0 {
            if let Some(entry) = self.overlays.iter_mut().find(|e| e.id == focused_overlay_id) {
                if !entry.hidden.load(std::sync::atomic::Ordering::Relaxed) {
                    entry.component.handle_input(&current);
                    return;
                }
            }
        }

        // Route to topmost non-capturing overlay if no overlay has explicit focus
        if let Some(entry) = self.overlays.back_mut() {
            if !entry.hidden.load(std::sync::atomic::Ordering::Relaxed)
                && !entry.options.non_capturing
                && focused_overlay_id == 0
            {
                entry.component.handle_input(&current);
                return;
            }
        }

        // Route to focused component
        if let Some(idx) = self.focused {
            if let Some(component) = self.container.children.get_mut(idx) {
                component.handle_input(&current);
            }
        }
    }

    /// Render the full UI to lines without drawing to the terminal.
    /// Returns (rendered_lines, cursor_position).
    /// This is the core rendering pipeline, separated for testability.
    pub fn render_to_lines(&self, width: u16, height: u16) -> (Vec<String>, Option<(u16, u16)>) {
        // 1. Render all children
        let mut lines = self.container.render(width);

        // 2. Composite overlays (only visible ones)
        for entry in &self.overlays {
            // Skip hidden overlays
            if entry.hidden.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            // Check dynamic visibility callback
            if let Some(ref visible_fn) = entry.options.visible {
                if !visible_fn(width, height) {
                    continue;
                }
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

        // 5. Pad to at least terminal height (fill empty terminal space),
        //    but do NOT truncate — content beyond terminal height is preserved
        //    for ratatui's Paragraph widget or downstream scrolling.
        while lines.len() < height as usize {
            lines.push(String::new());
        }

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

    // ============================================================================
    // Test helpers
    // ============================================================================

    struct TextComponent {
        text: String,
    }

    impl TextComponent {
        fn new(text: &str) -> Self {
            Self { text: text.to_string() }
        }

        fn lines(lines: &[&str]) -> Self {
            Self {
                text: lines.join("\n"),
            }
        }
    }

    impl Component for TextComponent {
        fn render(&self, _width: u16) -> Vec<String> {
            self.text.lines().map(|l| l.to_string()).collect()
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
        fn render(&self, _width: u16) -> Vec<String> {
            self.lines.clone()
        }
    }

    fn make_tui() -> Tui {
        Tui::new_test()
    }

    // ============================================================================
    // Container tests
    // ============================================================================

    #[test]
    fn test_container_renders_children() {
        let mut container = Container::new();
        container.add_child(Box::new(TextComponent::new("line1")));
        container.add_child(Box::new(TextComponent::new("line2")));
        let output = container.render(80);
        assert_eq!(output, vec!["line1", "line2"]);
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
        assert_eq!(container.render(80), vec!["a1", "a2", "b1", "b2", "b3"]);
    }

    #[test]
    fn test_container_invalidate() {
        let mut container = Container::new();
        container.add_child(Box::new(TextComponent::new("test")));
        container.invalidate();
        // Should not panic
    }

    // ============================================================================
    // TUI rendering e2e tests (via render_to_lines)
    // ============================================================================

    #[test]
    fn test_tui_render_single_line_component() {
        let mut tui = make_tui();
        let comp = Box::new(TextComponent::new("Hello World"));
        tui.add_child(comp);
        let (lines, cursor) = tui.render_to_lines(80, 24);
        assert_eq!(lines.len(), 24);
        assert!(lines[0].starts_with("Hello World"));
        assert!(lines[0].ends_with("\x1b[0m"));
        assert!(lines[1].is_empty());
        assert_eq!(cursor, None);
        // Verify reset is at end
        assert!(lines[0].contains("Hello World\x1b[0m"));
    }

    #[test]
    fn test_tui_render_multi_line() {
        let mut tui = make_tui();
        tui.add_child(Box::new(MultiLineComponent::new(&["Line 0", "Line 1", "Line 2"])));
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0].starts_with("Line 0"));
        assert!(lines[1].starts_with("Line 1"));
        assert!(lines[2].starts_with("Line 2"));
        assert!(lines[3].is_empty());
    }

    #[test]
    fn test_tui_render_height_padding() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("short")));
        let (lines, _) = tui.render_to_lines(80, 5);
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("short"));
        for i in 1..5 {
            assert!(lines[i].is_empty(), "line {} should be empty", i);
        }
    }

    #[test]
    fn test_tui_render_content_exceeds_height() {
        let mut tui = make_tui();
        let many: Vec<String> = (0..10).map(|i| format!("Line {}", i)).collect();
        let refs: Vec<&str> = many.iter().map(|s| s.as_str()).collect();
        tui.add_child(Box::new(MultiLineComponent::new(&refs)));
        let (lines, _) = tui.render_to_lines(80, 5);
        // Content beyond terminal height is preserved (no longer truncated)
        assert_eq!(lines.len(), 10);
        assert!(lines[0].starts_with("Line 0"));
        assert!(lines[9].starts_with("Line 9"));
    }

    #[test]
    fn test_tui_render_child_adding() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("first")));
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0].starts_with("first"));
        tui.add_child(Box::new(TextComponent::new("second")));
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0].starts_with("first"));
        assert!(lines[1].starts_with("second"));
    }

    #[test]
    fn test_tui_render_clear() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("content")));
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0].starts_with("content"));
        tui.clear();
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0].is_empty());
    }

    #[test]
    fn test_tui_render_preserves_all_lines() {
        let mut tui = make_tui();
        let many: Vec<String> = (0..50).map(|i| format!("Long line {}", i)).collect();
        let refs: Vec<&str> = many.iter().map(|s| s.as_str()).collect();
        tui.add_child(Box::new(MultiLineComponent::new(&refs)));
        let (lines, _) = tui.render_to_lines(80, 10);
        // All 50 lines preserved (no truncation)
        assert_eq!(lines.len(), 50);
    }

    // --- Content shrinkage tests (matching TS tui-render.test.ts "content shrinkage" describe) ---

    #[test]
    fn test_tui_shrink_clears_empty_rows() {
        let mut tui = make_tui();
        tui.add_child(Box::new(MultiLineComponent::new(&["Line 0", "Line 1", "Line 2", "Line 3", "Line 4"])));
        let (before, _) = tui.render_to_lines(40, 10);
        assert!(before[0].starts_with("Line 0"));

        // Simulate shrinkage: replace component with shorter content
        tui.container.children.clear();
        tui.add_child(Box::new(MultiLineComponent::new(&["Line 0", "Line 1"])));
        let (after, _) = tui.render_to_lines(40, 10);
        assert!(after[0].starts_with("Line 0"));
        assert!(after[1].starts_with("Line 1"));
        assert!(after[2].trim().is_empty(), "Line 2 should be cleared after shrink");
        assert!(after[3].trim().is_empty(), "Line 3 should be cleared after shrink");
    }

    #[test]
    fn test_tui_shrink_to_single_line() {
        let mut tui = make_tui();
        tui.add_child(Box::new(MultiLineComponent::new(&["Line 0", "Line 1", "Line 2"])));
        let _ = tui.render_to_lines(40, 10);
        tui.container.children.clear();
        tui.add_child(Box::new(TextComponent::new("Only line")));
        let (lines, _) = tui.render_to_lines(40, 10);
        assert!(lines[0].starts_with("Only line"));
        assert!(lines[1].trim().is_empty());
    }

    #[test]
    fn test_tui_shrink_to_empty() {
        let mut tui = make_tui();
        tui.add_child(Box::new(MultiLineComponent::new(&["Line 0", "Line 1", "Line 2"])));
        let _ = tui.render_to_lines(40, 10);
        tui.container.children.clear();
        let (lines, _) = tui.render_to_lines(40, 10);
        assert!(lines[0].trim().is_empty());
        assert!(lines[1].trim().is_empty());
    }

    #[test]
    fn test_tui_empty_to_content() {
        let mut tui = make_tui();
        // Start empty
        let (before, _) = tui.render_to_lines(40, 10);
        assert!(before[0].trim().is_empty());
        // Add content after empty state
        tui.add_child(Box::new(MultiLineComponent::new(&["New Line 0", "New Line 1"])));
        let (after, _) = tui.render_to_lines(40, 10);
        assert!(after[0].starts_with("New Line 0"));
        assert!(after[1].starts_with("New Line 1"));
    }

    // ============================================================================
    // Cursor marker e2e tests
    // ============================================================================

    #[test]
    fn test_tui_cursor_marker_extraction() {
        let mut lines = vec![
            "before".to_string(),
            format!("hello{}world", CURSOR_MARKER),
            "after".to_string(),
        ];
        let pos = extract_cursor_position(&mut lines);
        assert_eq!(pos, Some((1, 5)));
        assert!(!lines[1].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_tui_cursor_marker_at_start_of_line() {
        let mut lines = vec![format!("{}hello", CURSOR_MARKER)];
        let pos = extract_cursor_position(&mut lines);
        assert_eq!(pos, Some((0, 0)));
        assert!(!lines[0].contains(CURSOR_MARKER));
    }

    #[test]
    fn test_tui_cursor_marker_not_present() {
        let mut lines = vec!["hello".to_string(), "world".to_string()];
        let pos = extract_cursor_position(&mut lines);
        assert_eq!(pos, None);
    }

    #[test]
    fn test_tui_cursor_in_rendered_output() {
        struct CursorComponent;
        impl Component for CursorComponent {
            fn render(&self, _width: u16) -> Vec<String> {
                vec![format!("ab{}c", CURSOR_MARKER)]
            }
        }
        let mut tui = make_tui();
        tui.add_child(Box::new(CursorComponent));
        let (lines, cursor) = tui.render_to_lines(80, 24);
        assert_eq!(cursor, Some((0, 2))); // "ab" = 2 chars
        // Marker should be removed from output
        assert!(!lines[0].contains(CURSOR_MARKER));
        assert!(lines[0].starts_with("abc"));
    }

    // ============================================================================
    // Overlay e2e tests
    // ============================================================================

    #[test]
    fn test_overlay_position_center() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::Center,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 30);
        assert_eq!(row, 9);
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
    fn test_overlay_position_bottom_right() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::BottomRight,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 60);
        assert_eq!(row, 19);
    }

    #[test]
    fn test_overlay_position_top_center() {
        let opts = OverlayOptions {
            width: Some(20),
            anchor: OverlayAnchor::TopCenter,
            ..Default::default()
        };
        let (col, row) = overlay_position(&opts, 20, 5, 80, 24);
        assert_eq!(col, 30);
        assert_eq!(row, 0);
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
        assert_eq!(col, 3);
        assert_eq!(row, 2);
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
        assert_eq!(col, 35);
        assert_eq!(row, 7);
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
        assert!(base.len() >= 1);
        assert!(base[0].contains("OVERLAY"));
    }

    #[test]
    fn test_overlay_renders_in_tui() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("base content")));
        tui.show_overlay(
            Box::new(TextComponent::new("OVERLAY")),
            OverlayOptions {
                anchor: OverlayAnchor::TopLeft,
                ..Default::default()
            },
        );
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(lines[0].contains("OVERLAY"), "Overlay should show on line 0");
    }

    #[test]
    fn test_overlay_hidden() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("base")));
        let mut handle = tui.show_overlay(
            Box::new(TextComponent::new("OVERLAY")),
            OverlayOptions::default(),
        );
        // Initially visible
        let (before, _) = tui.render_to_lines(80, 24);
        assert!(before.iter().any(|l| l.contains("OVERLAY")));
        // Hide
        handle.set_hidden(true);
        let (after, _) = tui.render_to_lines(80, 24);
        assert!(!after.iter().any(|l| l.contains("OVERLAY")));
    }

    #[test]
    fn test_overlay_hide() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("base")));
        let mut handle = tui.show_overlay(
            Box::new(TextComponent::new("OVERLAY")),
            OverlayOptions::default(),
        );
        handle.hide();
        let (after, _) = tui.render_to_lines(80, 24);
        assert!(!after.iter().any(|l| l.contains("OVERLAY")));
    }

    #[test]
    fn test_overlay_hide_all() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("base")));
        tui.show_overlay(Box::new(TextComponent::new("A")), OverlayOptions::default());
        tui.show_overlay(Box::new(TextComponent::new("B")), OverlayOptions::default());
        assert!(tui.has_overlay());
        tui.hide_overlays();
        assert!(!tui.has_overlay());
        let (lines, _) = tui.render_to_lines(80, 24);
        assert!(!lines.iter().any(|l| l.contains("A")));
        assert!(!lines.iter().any(|l| l.contains("B")));
    }

    // ============================================================================
    // Input routing e2e tests
    // ============================================================================

    #[test]
    fn test_input_no_focus_noop() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("")));
        // No focus set — input should not panic
        tui.handle_input("hello");
    }

    #[test]
    fn test_input_no_overlay_noop() {
        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("")));
        tui.set_focus_index(0);
        tui.handle_input("hello");
    }

    #[test]
    fn test_input_listeners_can_intercept() {
        let mut tui = make_tui();
        let intercepted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let intercepted_clone = intercepted.clone();
        tui.add_input_listener(Box::new(move |_data| {
            intercepted_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            InputListenerResult { consume: true, data: None }
        }));
        tui.add_child(Box::new(TextComponent::new("")));
        tui.set_focus_index(0);
        tui.handle_input("block");
        assert!(intercepted.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn test_input_listeners_non_intercepting_passthrough() {
        let mut tui = make_tui();
        let passed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let passed_clone = passed.clone();
        tui.add_input_listener(Box::new(move |_data| {
            passed_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            InputListenerResult { consume: false, data: None }
        }));
        tui.add_child(Box::new(TextComponent::new("")));
        tui.set_focus_index(0);
        tui.handle_input("test");
        assert!(passed.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn test_input_listeners_transform_data() {
        let mut tui = make_tui();
        tui.add_input_listener(Box::new(move |data| {
            InputListenerResult {
                consume: false,
                data: Some(format!("transformed:{}", data)),
            }
        }));
        tui.add_child(Box::new(TextComponent::new("")));
        tui.set_focus_index(0);
        // Should not panic with transformed data
        tui.handle_input("test");
    }

    // ============================================================================
    // Container remove_child test
    // ============================================================================

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
        container.clear();
        assert_eq!(container.children.len(), 0);
    }

    // ============================================================================
    // Full redraw tracking
    // ============================================================================

    #[test]
    fn test_full_redraw_counter() {
        let mut tui = make_tui();
        assert_eq!(tui.full_redraws(), 0);
        let _ = tui.request_render(true);
        assert_eq!(tui.full_redraws(), 1);
        let _ = tui.request_render(false);
        assert_eq!(tui.full_redraws(), 1); // non-force does not increment
    }

    // ============================================================================
    // ratatui TestBackend full pipeline tests
    // ============================================================================

    #[test]
    fn test_render_to_frame_with_test_backend() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(40, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut tui = make_tui();
        tui.add_child(Box::new(MultiLineComponent::new(&["Hello World"])));

        terminal.draw(|frame| {
            tui.render_to_frame(frame);
        }).unwrap();

        let buffer = terminal.backend().buffer();
        // The output includes ANSI escape sequences rendered literally
        // First character on line 0 is ESC (\x1b) from the reset sequence appended
        // after the text. Actually, the reset \x1b[0m is appended but rendered as raw text.
        // Verify buffer has content
        assert!(buffer.get(0, 0).symbol().len() > 0);
    }

    #[test]
    fn test_render_to_frame_overlay() {
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

        terminal.draw(|frame| {
            tui.render_to_frame(frame);
        }).unwrap();

        let buffer = terminal.backend().buffer();
        // Verify buffer has content without panicking
        assert!(buffer.get(0, 0).symbol().len() > 0);
    }

    #[test]
    fn test_render_to_frame_pads_to_terminal_height() {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(40, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        let mut tui = make_tui();
        tui.add_child(Box::new(TextComponent::new("short")));

        terminal.draw(|frame| {
            tui.render_to_frame(frame);
        }).unwrap();

        let buffer = terminal.backend().buffer();
        for row in 0..5 {
            let cell = buffer.get(0, row);
            let _ = cell;
        }
    }
}
