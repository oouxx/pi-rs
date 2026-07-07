//! Elm architecture core — Model, Msg, update, view, Cmd.

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
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
pub struct ToolCall {
    pub name: String,
    pub state: ToolCallState,
}

#[derive(Debug, Clone)]
pub enum ToolCallState {
    Pending,
    Running,
    Done,
    Failed,
}

// ============================================================================
// Model
// ============================================================================

/// Three-section layout state.
pub struct Model {
    pub width: u16,
    pub height: u16,
    pub mode: AppMode,
    /// Chat messages in chronological order (oldest first).
    pub messages: Vec<Message>,
    pub is_streaming: bool,
    pub input: Input,
    /// Model name shown in header.
    pub model_name: String,
    /// Animation tick counter (for spinner).
    pub tick: u64,
    /// Active tool calls (state machine).
    pub active_tools: Vec<ToolCall>,
    /// Active modal dialog (None = no dialog).
    pub dialog: Option<Dialog>,
    /// Autocomplete engine.
    pub completer: Completer,
}

/// A modal dialog box with callback on confirm.
pub struct Dialog {
    pub title: String,
    pub message: String,
    pub buttons: Vec<DialogButton>,
    pub selected: usize,
}

pub struct DialogButton {
    pub label: &'static str,
    pub action: DialogAction,
}

pub enum DialogAction {
    Confirm,
    Cancel,
    Custom(&'static str),
}

pub enum AppMode { Chat, Select { list: SelectList }, Editor { editor: Editor, title: String } }

pub struct Message { pub role: String, pub text: String }

impl Model {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width, height,
            mode: AppMode::Chat,
            messages: Vec::new(),
            is_streaming: false,
            input: Input::new(),
            model_name: String::new(),
            tick: 0,
            active_tools: Vec::new(),
            dialog: None,
            completer: Completer::new(),
        }
    }

    pub fn add_tool_call(&mut self, name: &str) {
        self.active_tools.push(ToolCall {
            name: name.to_string(),
            state: ToolCallState::Running,
        });
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
    Key(KeyEvent),
    Resize(u16, u16),
    Paste(String),
    NewMessage(String, String),
    StreamText(String),
    StreamEnd,
    OpenEditor(String, String),
    EditorDone(String),
    ToolStart(String),
    ToolEnd(String, bool),
    Tick,
    /// Show a modal dialog.
    ShowDialog(Dialog),
    /// Dismiss the current dialog.
    DismissDialog,
    /// Select next/previous button in dialog.
    DialogNext,
    DialogPrev,
    /// Activate the currently selected dialog button.
    DialogConfirm,
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
        Msg::Tick => { model.tick += 1; vec![] }
        Msg::ShowDialog(dialog) => { model.dialog = Some(dialog); vec![] }
        Msg::DismissDialog => { model.dialog = None; vec![] }
        Msg::DialogNext => {
            if let Some(ref mut d) = model.dialog {
                if d.selected + 1 < d.buttons.len() { d.selected += 1; }
            }
            vec![]
        }
        Msg::DialogPrev => {
            if let Some(ref mut d) = model.dialog {
                if d.selected > 0 { d.selected -= 1; }
            }
            vec![]
        }
        Msg::DialogConfirm => {
            if let Some(d) = model.dialog.take() {
                // Dialog action handled by the interactive mode
                // (returns Cancel so caller can check dialog state)
            }
            vec![]
        }
        Msg::Cancel => { model.mode = AppMode::Chat; vec![] }
    }
}

fn handle_key(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    use crossterm::event::KeyCode;
    if key.kind != crossterm::event::KeyEventKind::Press { return vec![]; }

    // If completer popup is visible, route keys to it
    if model.completer.visible {
        match key.code {
            KeyCode::Tab | KeyCode::Down => { model.completer.next(); }
            KeyCode::Up => { model.completer.prev(); }
            KeyCode::Enter | KeyCode::Right => {
                let insert = model.completer.selected_insert();
                if let Some(text) = insert {
                    let current = model.input.value().to_string();
                    if let Some(trigger_pos) = current.rfind(|c: char| c == '/' || c == '@') {
                        let prefix = &current[..=trigger_pos];
                        model.input.clear();
                        model.input.insert_str(&format!("{prefix}{text} "));
                    }
                }
                model.completer.deactivate();
            }
            KeyCode::Esc => { model.completer.deactivate(); }
            _ => {} // Ignore typing while popup is shown
        }
        return vec![];
    }

    match &mut model.mode {
        AppMode::Chat => match key.code {
            KeyCode::Char(c) => {
                // Check if this character should trigger completion
                if let Some(trigger) = Completer::should_activate(c) {
                    model.completer.activate(trigger, "");
                } else if model.completer.trigger.is_some() && !c.is_whitespace() {
                    // Continue updating query for active completion
                    let mut q = model.completer.query.clone();
                    q.push(c);
                    model.completer.activate(model.completer.trigger.unwrap(), &q);
                } else {
                    model.completer.deactivate();
                }
                model.input.insert_char(c);
            }
            KeyCode::Backspace => {
                model.input.backspace();
                // Update completion query if active
                if model.completer.trigger.is_some() {
                    let current = model.input.value();
                    if let Some(pos) = current.rfind(|c: char| c == '/' || c == '@') {
                        let q = &current[pos + 1..];
                        model.completer.activate(model.completer.trigger.unwrap(), q);
                    } else {
                        model.completer.deactivate();
                    }
                }
            }
            KeyCode::Tab => {
                if !model.completer.visible {
                    for _ in 0..4 { model.input.insert_char(' '); }
                } else {
                    model.completer.next();
                }
            }
            KeyCode::Enter => { model.input.clear(); model.completer.deactivate(); }
            KeyCode::Delete => model.input.delete(),
            KeyCode::Left => model.input.move_left(),
            KeyCode::Right => model.input.move_right(),
            KeyCode::Home => model.input.move_home(),
            KeyCode::End => model.input.move_end(),
            _ => {}
        },
        AppMode::Select { list } => { list.handle_key(&key); }
        AppMode::Editor { editor, .. } => { editor.handle_key(&key); }
    }
    vec![]
}

// ============================================================================
// View — three-section layout: header / body / input
// ============================================================================

pub fn view(model: &Model, frame: &mut Frame) {
    let area = frame.size();

    // Editor mode takes full screen
    if let AppMode::Editor { editor, title, .. } = &model.mode {
        render_editor(frame, area, editor, title);
        return;
    }

    // Three-section layout
    let chunks = Layout::new(Direction::Vertical, [
        Constraint::Length(1),          // header
        Constraint::Min(1),             // body (fills remaining)
        Constraint::Length(3),          // input
    ]).split(area);

    render_header(model, frame, chunks[0]);
    render_body(model, frame, chunks[1]);
    render_input(model, frame, chunks[2]);

    // Modal dialog overlay
    if model.dialog.is_some() {
        render_dialog(model, frame, area);
        return; // Dialog blocks everything else
    }

    // Select overlay
    if let AppMode::Select { list, .. } = &model.mode {
        let oa = Rect::new(area.width / 4, area.height / 4, area.width / 2, area.height / 2);
        frame.render_widget(Clear, oa);
        list.render_to_frame(frame, oa);
    }
}

/// Header: model name + streaming indicator + key hints.
fn render_header(model: &Model, frame: &mut Frame, area: Rect) {
    let model_label = if model.model_name.is_empty() {
        " pi-rs ".to_string()
    } else {
        format!(" {} ", model.model_name)
    };

    let spinner_char = if model.is_streaming || !model.active_tools.is_empty() {
        let idx = (model.tick / 3) as usize % SPINNER_FRAMES.len();
        Some(SPINNER_FRAMES[idx])
    } else {
        None
    };

    let status = match (spinner_char, model.is_streaming) {
        (Some(ch), true) => Span::styled(format!(" {ch} streaming "), Style::new().fg(Color::Green)),
        (Some(ch), false) => Span::styled(format!(" {ch} working "), Style::new().fg(Color::Yellow)),
        _ => Span::raw(""),
    };

    let hint = Span::styled(
        " Ctrl+C:abort x2:quit ",
        Style::new().fg(Color::DarkGray),
    );

    let line = Line::from(vec![
        Span::styled(model_label, Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        status,
        Span::raw(" "),
        hint,
    ]);

    frame.render_widget(Paragraph::new(line).style(Style::new().bg(Color::Black)), area);
}

/// Body: scrollable chat messages (newest anchored at bottom).
fn render_body(model: &Model, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render active tool calls as cards
    let mut y = inner.bottom() as i32 - 1;

    // Tool call cards (above messages)
    for tool in model.active_tools.iter().rev() {
        let spinner = {
            let idx = (model.tick / 3) as usize % SPINNER_FRAMES.len();
            SPINNER_FRAMES[idx]
        };
        let (icon, status_style) = match tool.state {
            ToolCallState::Running => (
                format!(" {spinner} "),
                Style::new().fg(Color::Yellow),
            ),
            ToolCallState::Done => (
                " ✓ ".to_string(),
                Style::new().fg(Color::Green),
            ),
            ToolCallState::Failed => (
                " ✗ ".to_string(),
                Style::new().fg(Color::Red),
            ),
            ToolCallState::Pending => (
                " ○ ".to_string(),
                Style::new().fg(Color::DarkGray),
            ),
        };

        let card_line = format!("{}{}", icon, tool.name);
        let total = 1usize;
        y -= total as i32;
        if y + total as i32 <= inner.top() as i32 { break; }
        let sy = y.max(inner.top() as i32) as u16;
        if sy >= inner.top() && sy < inner.bottom() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(card_line, status_style))),
                Rect::new(inner.x + 1, sy, inner.width.saturating_sub(2), 1),
            );
        }
    }

    // Chat messages
    for msg in model.messages.iter().rev() {
        // Wrap message text to inner width
        let raw_lines = wrap_text(&msg.text, (inner.width as usize).saturating_sub(2));
        let total_lines = 1 + raw_lines.len(); // role label + wrapped lines

        y -= total_lines as i32;
        if y + total_lines as i32 <= inner.top() as i32 { break; }
        let sy = y.max(inner.top() as i32) as u16;

        // Role label
        let role_color = match msg.role.as_str() {
            "user" => Color::Green,
            "assistant" => Color::Cyan,
            "tool" => Color::Yellow,
            _ => Color::White,
        };
        let role_label = Line::from(Span::styled(
            format!(" {} ", msg.role),
            Style::new().fg(role_color).add_modifier(Modifier::BOLD),
        ));

        if sy >= inner.top() && sy < inner.bottom() {
            frame.render_widget(Paragraph::new(role_label), Rect::new(inner.x, sy, inner.width, 1));
        }

        // Message text lines
        for (i, ln) in raw_lines.iter().enumerate() {
            let ly = sy + 1 + i as u16;
            if ly < inner.top() || ly >= inner.bottom() { continue; }
            let style = match msg.role.as_str() {
                "user" => Style::default().fg(Color::White),
                "assistant" => Style::default().fg(Color::White),
                _ => Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            };
            frame.render_widget(
                Paragraph::new(ln.clone()).style(style),
                Rect::new(inner.x + 1, ly, inner.width.saturating_sub(2), 1),
            );
        }
    }
}

/// Input: single-line text input at the bottom.
fn render_input(model: &Model, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(if model.is_streaming { " ⏳ Waiting " } else { " >>> " })
        .title_alignment(ratatui::layout::Alignment::Left);

    let style = if model.is_streaming {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render input text (single line, scrolls horizontally if too long)
    let display_text = model.input.value();
    let scroll_offset = if model.input.cursor_pos() > inner.width as usize {
        model.input.cursor_pos() - inner.width as usize
    } else {
        0
    };
    let visible = if scroll_offset > 0 && display_text.len() > scroll_offset {
        &display_text[scroll_offset..]
    } else {
        display_text
    };

    let p = Paragraph::new(Text::styled(visible.to_string(), style));
    frame.render_widget(p, Rect::new(inner.x, inner.y, inner.width, inner.height));

    // Completion popup (anchored above the input line)
    if model.completer.visible {
        let cursor_x = inner.x + (model.input.cursor_pos() as u16).saturating_sub(scroll_offset as u16);
        model.completer.render(frame, cursor_x, area.y);
    }

    // Cursor
    if !model.is_streaming {
        let cursor_x = inner.x + (model.input.cursor_pos() as u16).saturating_sub(scroll_offset as u16);
        frame.set_cursor_position((cursor_x.min(inner.x + inner.width.saturating_sub(1)), inner.y));
    }
}

/// Full-screen editor (for composing longer messages).
fn render_editor(frame: &mut Frame, area: Rect, editor: &Editor, title: &str) {
    let block = Block::default()
        .borders(Borders::ALL).border_type(BorderType::Rounded)
        .title(format!(" {title} — Esc:cancel Ctrl+S:confirm "));
    let inner = block.inner(area);
    let p = Paragraph::new(Text::raw(editor.text())).block(block);
    frame.render_widget(p, area);
    frame.set_cursor_position((inner.x + editor.cursor_col(), inner.y + editor.cursor_row()));
}

// ============================================================================
// Dialog — centered modal popup
// ============================================================================

/// Render a centered modal dialog that dims the background.
fn render_dialog(model: &Model, frame: &mut Frame, area: Rect) {
    let dialog = match &model.dialog {
        Some(d) => d,
        None => return,
    };

    // Dim the full screen behind the dialog
    frame.render_widget(Clear, area);

    // Calculate centered dialog area
    let dlg_w = (area.width / 3 * 2).max(40).min(area.width.saturating_sub(4));
    let dlg_h = 5u16 + dialog.message.lines().count() as u16;
    let dlg_x = (area.width - dlg_w) / 2;
    let dlg_y = (area.height - dlg_h) / 2;

    let dlg_area = Rect::new(dlg_x, dlg_y, dlg_w, dlg_h);

    // Dialog border
    let style = Style::new().bg(Color::Black).fg(Color::White);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::new().fg(Color::Cyan))
        .title(format!(" {} ", dialog.title))
        .style(style);

    // Message + buttons inside
    let inner = block.inner(dlg_area);
    frame.render_widget(block, dlg_area);

    // Message lines
    let mut y = inner.y;
    for line in dialog.message.lines() {
        if y < inner.y + inner.height {
            frame.render_widget(
                Paragraph::new(Line::from(Span::raw(line.to_string()))),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
    }

    // Buttons row (centered at bottom)
    let btn_area = Rect::new(inner.x, dlg_area.y + dlg_h - 2, inner.width, 1);
    let mut btn_spans = Vec::new();
    let total_btn_width: usize = dialog.buttons.iter().map(|b| b.label.len() + 2).sum();
    let spacing = (inner.width as usize).saturating_sub(total_btn_width) / (dialog.buttons.len() + 1).max(1);

    btn_spans.push(Span::raw(" ".repeat(spacing)));
    for (i, button) in dialog.buttons.iter().enumerate() {
        let is_focused = i == dialog.selected;
        let btn_style = if is_focused {
            Style::new().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::new().fg(Color::White).bg(Color::DarkGray)
        };
        btn_spans.push(Span::styled(format!(" {} ", button.label), btn_style));
        btn_spans.push(Span::raw(" ".repeat(spacing)));
    }

    frame.render_widget(Paragraph::new(Line::from(btn_spans)), btn_area);
}

// ============================================================================
// Helpers
// ============================================================================

/// Simple word-wrap at `width`. Preserves existing newlines.
fn wrap_text(text: &str, width: usize) -> Vec<Line<'static>> {
    if width < 2 { return vec![Line::from(Span::raw(text.to_string()))]; }
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut start = 0;
        let line_owned = line.to_string();
        let bytes = line_owned.as_bytes();
        while start < line_owned.len() {
            let end = (start + width).min(line_owned.len());
            let break_at = if end < line_owned.len() {
                let mut pos = end;
                while pos > start && bytes[pos] != b' ' { pos -= 1; }
                if pos > start { pos } else { end }
            } else { end };
            lines.push(Line::from(Span::raw(line_owned[start..break_at].to_string())));
            start = if break_at < line_owned.len() && bytes[break_at] == b' ' { break_at + 1 } else { break_at };
        }
    }
    if lines.is_empty() { lines.push(Line::from(Span::raw(String::new()))); }
    lines
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
