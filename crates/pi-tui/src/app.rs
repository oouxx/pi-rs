//! Elm architecture core — Model, Msg, update, view, Cmd.

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::components::{Editor, Input, SelectList};

// ============================================================================
// Cmd
// ============================================================================

pub enum Cmd { Quit }

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
        Msg::Cancel => { model.mode = AppMode::Chat; vec![] }
    }
}

fn handle_key(model: &mut Model, key: KeyEvent) -> Vec<Cmd> {
    use crossterm::event::KeyCode;
    if key.kind != crossterm::event::KeyEventKind::Press { return vec![]; }
    match &mut model.mode {
        AppMode::Chat => match key.code {
            KeyCode::Enter => { model.input.clear(); }
            KeyCode::Char(c) => model.input.insert_char(c),
            KeyCode::Backspace => model.input.backspace(),
            KeyCode::Delete => model.input.delete(),
            KeyCode::Left => model.input.move_left(),
            KeyCode::Right => model.input.move_right(),
            KeyCode::Home => model.input.move_home(),
            KeyCode::End => model.input.move_end(),
            KeyCode::Tab => { for _ in 0..4 { model.input.insert_char(' '); } }
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

    let status = if model.is_streaming {
        Span::styled(" ● streaming ", Style::new().fg(Color::Green))
    } else {
        Span::raw("")
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

    // Render from bottom up, wrapping text
    let mut y = inner.bottom() as i32 - 1;

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
