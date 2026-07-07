//! Elm architecture core — Model, Msg, update, view, Cmd.

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::components::{Completer, Editor, Input, SelectList};

/// Spinner frames for tool execution animation.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ============================================================================
// Cmd
// ============================================================================

pub enum Cmd { Quit }

// ============================================================================
// ToolCall state machine
// ============================================================================

#[derive(Debug, Clone)]
pub struct ToolCall { pub name: String, pub state: ToolCallState }

#[derive(Debug, Clone)]
pub enum ToolCallState { Pending, Running, Done, Failed }

// ============================================================================
// Model
// ============================================================================

pub struct Model {
    pub width: u16, pub height: u16, pub mode: AppMode,
    pub messages: Vec<Message>, pub is_streaming: bool,
    pub input: Input, pub model_name: String, pub tick: u64,
    pub active_tools: Vec<ToolCall>, pub dialog: Option<Dialog>,
    pub completer: Completer,
    pub scroll_offset: usize,
    pub total_body_lines: usize,
}

pub enum AppMode { Chat, Select { list: SelectList }, Editor { editor: Editor, title: String } }
pub struct Message { pub role: String, pub text: String }

pub struct Dialog { pub title: String, pub message: String, pub buttons: Vec<DialogButton>, pub selected: usize }
pub struct DialogButton { pub label: &'static str, pub action: DialogAction }
pub enum DialogAction { Confirm, Cancel, Custom(&'static str) }

impl Model {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width, height, mode: AppMode::Chat, messages: Vec::new(),
            is_streaming: false, input: Input::new(), model_name: String::new(),
            tick: 0, active_tools: Vec::new(), dialog: None,
            completer: Completer::new(), scroll_offset: 0, total_body_lines: 0,
        }
    }

    pub fn add_tool_call(&mut self, name: &str) {
        self.active_tools.push(ToolCall { name: name.to_string(), state: ToolCallState::Running });
    }

    pub fn update_tool_call(&mut self, name: &str, state: ToolCallState) {
        if let Some(tool) = self.active_tools.iter_mut().rev().find(|t| t.name == name) {
            tool.state = state;
        }
    }
}

// ============================================================================
// Msg
// ============================================================================

pub enum Msg {
    Key(KeyEvent), Resize(u16, u16), Paste(String),
    NewMessage(String, String), StreamText(String), StreamEnd,
    OpenEditor(String, String), EditorDone(String),
    ToolStart(String), ToolEnd(String, bool),
    Tick,
    ScrollUp(u16), ScrollDown(u16), ScrollToBottom,
    ShowDialog(Dialog), DismissDialog, DialogNext, DialogPrev, DialogConfirm,
    Cancel,
}

// ============================================================================
// Update
// ============================================================================

pub fn update(model: &mut Model, msg: Msg) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => handle_key(model, key),
        Msg::Resize(w, h) => { model.width = w; model.height = h; vec![] }
        Msg::Paste(text) => { model.input.insert_str(&text); vec![] }
        Msg::NewMessage(role, text) => { model.messages.push(Message { role, text }); vec![] }
        Msg::StreamText(delta) => { if let Some(m) = model.messages.last_mut() { m.text.push_str(&delta); } vec![] }
        Msg::StreamEnd => { model.is_streaming = false; vec![] }
        Msg::OpenEditor(title, text) => { model.mode = AppMode::Editor { editor: Editor::new(&text), title }; vec![] }
        Msg::EditorDone(_) => { model.mode = AppMode::Chat; vec![] }
        Msg::ToolStart(name) => { model.add_tool_call(&name); vec![] }
        Msg::ToolEnd(name, is_error) => {
            model.update_tool_call(&name, if is_error { ToolCallState::Failed } else { ToolCallState::Done });
            vec![]
        }
        Msg::Tick => {
            model.tick += 1;
            if model.is_streaming { model.scroll_offset = 0; }
            vec![]
        }
        Msg::ScrollUp(amount) => {
            model.scroll_offset = (model.scroll_offset + amount as usize).min(model.total_body_lines);
            vec![]
        }
        Msg::ScrollDown(amount) => {
            model.scroll_offset = model.scroll_offset.saturating_sub(amount as usize);
            vec![]
        }
        Msg::ScrollToBottom => { model.scroll_offset = 0; vec![] }
        Msg::ShowDialog(dialog) => { model.dialog = Some(dialog); vec![] }
        Msg::DismissDialog => { model.dialog = None; vec![] }
        Msg::DialogNext => { if let Some(ref mut d) = model.dialog { if d.selected + 1 < d.buttons.len() { d.selected += 1; } } vec![] }
        Msg::DialogPrev => { if let Some(ref mut d) = model.dialog { if d.selected > 0 { d.selected -= 1; } } vec![] }
        Msg::DialogConfirm => { model.dialog.take(); vec![] }
        Msg::Cancel => { model.mode = AppMode::Chat; vec![] }
    }
}

// ============================================================================
// Handle key
// ============================================================================

fn handle_key(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    use crossterm::event::KeyCode;
    if key.kind != crossterm::event::KeyEventKind::Press
        && key.kind != crossterm::event::KeyEventKind::Release
    { return vec![]; }
    if model.completer.visible {
        match key.code {
            KeyCode::Tab | KeyCode::Down => { model.completer.next(); }
            KeyCode::Up => { model.completer.prev(); }
            KeyCode::Enter | KeyCode::Right => {
                if let Some(text) = model.completer.selected_insert() {
                    let current = model.input.value().to_string();
                    if let Some(pos) = current.rfind(|c: char| c == '/' || c == '@') {
                        let prefix = &current[..=pos];
                        model.input.clear();
                        model.input.insert_str(&format!("{prefix}{text} "));
                    }
                }
                model.completer.deactivate();
            }
            KeyCode::Esc => { model.completer.deactivate(); }
            _ => {}
        }
        return vec![];
    }
    let _ = model;
    vec![]
}

// ============================================================================
// View — three-section layout: header / body / input
// ============================================================================

pub fn view(model: &Model, frame: &mut Frame) {
    let area = frame.size();
    if let AppMode::Editor { editor, title, .. } = &model.mode {
        render_editor(frame, area, editor, title);
        return;
    }

    let chunks = Layout::new(Direction::Vertical, [
        Constraint::Length(1), Constraint::Min(1), Constraint::Length(2),
    ]).split(area);

    render_header(model, frame, chunks[0]);
    render_body(model, frame, chunks[1]);
    render_input(model, frame, chunks[2]);

    if model.dialog.is_some() { render_dialog(model, frame, area); return; }
    if let AppMode::Select { list, .. } = &model.mode {
        let oa = Rect::new(area.width / 4, area.height / 4, area.width / 2, area.height / 2);
        frame.render_widget(Clear, oa);
        list.render_to_frame(frame, oa);
    }
}

// ============================================================================
// Header
// ============================================================================

fn render_header(model: &Model, frame: &mut Frame, area: Rect) {
    let label = if model.model_name.is_empty() { " pi-rs ".into() } else { format!(" {} ", model.model_name) };
    let spinner = if model.is_streaming || !model.active_tools.is_empty() {
        Some(SPINNER_FRAMES[(model.tick / 3) as usize % SPINNER_FRAMES.len()])
    } else { None };
    let status = match (spinner, model.is_streaming) {
        (Some(ch), true) => Span::styled(format!(" {ch} streaming "), Style::new().fg(Color::Green)),
        (Some(ch), false) => Span::styled(format!(" {ch} working "), Style::new().fg(Color::Yellow)),
        _ => Span::raw(""),
    };
    let hint = Span::styled(" Ctrl+C:abort x2:quit ", Style::new().fg(Color::DarkGray));
    let line = Line::from(vec![
        Span::styled(label, Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        status, Span::raw(" "), hint,
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(Color::Black)), area);
}

// ============================================================================
// Body — virtual scrolling
// ============================================================================

fn render_body(model: &Model, frame: &mut Frame, area: Rect) {
    // Borderless body — just use the area directly
    let inner = area;
    let body_h = inner.height as usize;

    // Wrap width
    let wrap_w = (inner.width as usize).saturating_sub(3).max(10);

    // Compute item heights. Each item: role label (1) + wrapped text lines.
    struct Item { role: String, lines: Vec<String> }
    let mut items: Vec<Item> = Vec::new();

    // Tool call cards (1 line each, use "tool" as role)
    for tool in &model.active_tools {
        items.push(Item { role: "tool".to_string(), lines: vec![tool.name.clone()] });
    }

    // Messages
    for msg in &model.messages {
        let wrapped = simple_wrap(&msg.text, wrap_w);
        items.push(Item { role: msg.role.clone(), lines: wrapped });
    }

    // Compute total lines (stored via interior mutability)
    let total_lines: usize = items.iter().map(|it| 1 + it.lines.len()).sum();
    // Not stored to model here (view has immutable ref) — scroll bounds are approximate.
    // The update function can correct over-scroll.

    // Visible range: skip `scroll_offset` lines from the bottom
    // Walk items from end, skip offset lines, then render
    let mut skip = model.scroll_offset;
    let mut y = inner.bottom() as i32 - 1;
    let top_line = inner.top() as i32;

    for item in items.iter().rev() {
        if skip >= 1 + item.lines.len() {
            skip -= 1 + item.lines.len();
            continue;
        }

        let remaining = (1 + item.lines.len()).saturating_sub(skip);
        let render_this = remaining > 0 && y - remaining as i32 + 1 > top_line - body_h as i32;
        if !render_this { break; }

        y = y - remaining as i32 + 1; // align to top of visible portion
        if y + remaining as i32 <= top_line { break; }
        let sy = y.max(top_line) as u16;
        y = sy as i32; // reset y to actual top

        match item.role.as_str() {
            "tool" => {
                let tool = model.active_tools.iter().find(|t| t.name == item.lines[0]);
                if let Some(t) = tool {
                    let sp = SPINNER_FRAMES[(model.tick / 3) as usize % SPINNER_FRAMES.len()];
                    let (icon, st) = match t.state {
                        ToolCallState::Running => (format!(" {sp} "), Color::Yellow),
                        ToolCallState::Done => (" ✓ ".into(), Color::Green),
                        ToolCallState::Failed => (" ✗ ".into(), Color::Red),
                        ToolCallState::Pending => (" ○ ".into(), Color::DarkGray),
                    };
                    if sy >= inner.top() && sy < inner.bottom() {
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(format!("{}{}", icon, t.name), Style::new().fg(st)))),
                            Rect::new(inner.x + 1, sy, inner.width.saturating_sub(2), 1),
                        );
                    }
                }
                y += 1;
            }
            _ => {
                // Role label
                let rc = match item.role.as_str() {
                    "user" => Color::Green, "assistant" => Color::Cyan, _ => Color::White,
                };
                if sy >= inner.top() && sy < inner.bottom() {
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(format!(" {} ", item.role),
                            Style::new().fg(rc).add_modifier(Modifier::BOLD)))),
                        Rect::new(inner.x, sy, inner.width, 1),
                    );
                }
                y += 1;

                // Text lines
                for line in &item.lines {
                    if y >= inner.top() as i32 && y < inner.bottom() as i32 {
                        let text_style = match item.role.as_str() {
                            "tool" => Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                            _ => Style::default().fg(Color::White),
                        };
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::raw(line.clone()))).style(text_style),
                            Rect::new(inner.x + 1, y as u16, inner.width.saturating_sub(2), 1),
                        );
                    }
                    y += 1;
                }
            }
        }
        skip = 0;
    }

    if total_lines == 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " No messages yet. Type and press Enter.",
                Style::new().fg(Color::DarkGray),
            ))),
            Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1),
        );
    }

    // Scroll indicator
    if model.scroll_offset > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" ↑ {} lines ", model.scroll_offset),
                Style::new().fg(Color::DarkGray).bg(Color::Black),
            ))),
            Rect::new(inner.x + inner.width.saturating_sub(12), inner.y, 12, 1),
        );
    }
}

// ============================================================================
// Input
// ============================================================================

fn render_input(model: &Model, frame: &mut Frame, area: Rect) {
    let style = if model.is_streaming { Style::default().fg(Color::DarkGray) } else { Style::default() };

    // Thin separator line above prompt
    let sep_y = area.y;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {} ", if model.is_streaming { "⏳" } else { ">" }),
            if model.is_streaming { Style::new().fg(Color::Green) } else { Style::new().fg(Color::Cyan) },
        ))),
        Rect::new(area.x, sep_y, area.width, 1),
    );

    let inner_y = area.y + 1;
    let text = model.input.value();
    let cursor_display = model.input.cursor_display_col();
    let scroll_off = (cursor_display as usize).saturating_sub(area.width as usize);
    let visible = if scroll_off > 0 {
        // Find byte offset for the display column offset
        let mut display_cols = 0usize;
        let mut byte_off = 0usize;
        for ch in text.chars() {
            let w = unicode_width::UnicodeWidthStr::width(ch.to_string().as_str());
            if display_cols + w > scroll_off { break; }
            display_cols += w;
            byte_off += ch.len_utf8();
        }
        &text[byte_off..]
    } else {
        text
    };

    frame.render_widget(Paragraph::new(Text::styled(visible.to_string(), style)),
        Rect::new(area.x, inner_y, area.width, 1));

    if model.completer.visible {
        let cx = area.x + cursor_display.saturating_sub(scroll_off as u16);
        model.completer.render(frame, cx, area.y);
    }

    if !model.is_streaming {
        let cx = area.x + cursor_display.saturating_sub(scroll_off as u16);
        frame.set_cursor_position((cx.min(area.x + area.width.saturating_sub(1)), inner_y));
    }
}

// ============================================================================
// Editor / Dialog
// ============================================================================

fn render_editor(frame: &mut Frame, area: Rect, editor: &Editor, title: &str) {
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .title(format!(" {title} "));
    let inner = block.inner(area);
    let p = Paragraph::new(Text::raw(editor.text())).block(block);
    frame.render_widget(p, area);
    frame.set_cursor_position((inner.x + editor.cursor_col(), inner.y + editor.cursor_row()));
}

fn render_dialog(model: &Model, frame: &mut Frame, area: Rect) {
    let dialog = match &model.dialog { Some(d) => d, None => return };
    frame.render_widget(Clear, area);

    let dw = (area.width / 3 * 2).max(40).min(area.width.saturating_sub(4));
    let dh = 5u16 + dialog.message.lines().count() as u16;
    let da = Rect::new((area.width - dw) / 2, (area.height - dh) / 2, dw, dh);

    frame.render_widget(Clear, da);

    // Title
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {} ", dialog.title),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))),
        Rect::new(da.x, da.y, da.width, 1),
    );

    // Dimmed backdrop for message area
    let inner = Rect::new(da.x, da.y + 1, da.width, da.height.saturating_sub(2));

    let mut y = inner.y;
    for line in dialog.message.lines() {
        if y < inner.y + inner.height {
            frame.render_widget(Paragraph::new(Line::from(Span::raw(line.to_string()))), Rect::new(inner.x, y, inner.width, 1));
            y += 1;
        }
    }

    let ba = Rect::new(inner.x, da.y + dh - 2, inner.width, 1);
    let total_w: usize = dialog.buttons.iter().map(|b| b.label.len() + 2).sum();
    let spacing = (inner.width as usize).saturating_sub(total_w) / (dialog.buttons.len() + 1).max(1);

    let mut spans = vec![Span::raw(" ".repeat(spacing))];
    for (i, btn) in dialog.buttons.iter().enumerate() {
        let s = if i == dialog.selected { Style::new().fg(Color::Black).bg(Color::Cyan) } else { Style::new().fg(Color::White).bg(Color::DarkGray) };
        spans.push(Span::styled(format!(" {} ", btn.label), s));
        spans.push(Span::raw(" ".repeat(spacing)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), ba);
}

// ============================================================================
// Helpers
// ============================================================================

fn simple_wrap(text: &str, width: usize) -> Vec<String> {
    if width < 2 { return vec![text.to_string()]; }
    let mut lines = Vec::new();
    for line in text.lines() {
        let owned = line.to_string();
        let bytes = owned.as_bytes();
        let mut start = 0;
        while start < owned.len() {
            let end = (start + width).min(owned.len());
            let brk = if end < owned.len() {
                let mut p = end;
                while p > start && bytes[p] != b' ' { p -= 1; }
                if p > start { p } else { end }
            } else { end };
            lines.push(owned[start..brk].to_string());
            start = if brk < owned.len() && bytes[brk] == b' ' { brk + 1 } else { brk };
        }
    }
    if lines.is_empty() { lines.push(String::new()); }
    lines
}

fn wrap_text(text: &str, width: usize) -> Vec<Line<'static>> {
    simple_wrap(text, width).into_iter().map(|s| Line::from(Span::raw(s))).collect()
}

// ============================================================================
// Main loop (standalone)
// ============================================================================

pub async fn run(
    mut model: Model,
    mut terminal: crate::terminal::Terminal,
    mut input_rx: tokio::sync::mpsc::UnboundedReceiver<KeyEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::time::{sleep, Duration};
    loop {
        terminal.ratatui_terminal().draw(|frame| view(&model, frame))?;
        tokio::select! {
            Some(key) = input_rx.recv() => {
                let cmds = update(&mut model, Msg::Key(key));
                for cmd in cmds { if let Cmd::Quit = cmd { return Ok(()); } }
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }
    }
}
