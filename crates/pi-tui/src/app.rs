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

pub enum Cmd {
    Quit,
}

// ============================================================================
// Model
// ============================================================================

pub struct Model {
    pub width: u16,
    pub height: u16,
    pub mode: AppMode,
    pub messages: Vec<Message>,
    pub is_streaming: bool,
    pub input: Input,
}

pub enum AppMode {
    Chat,
    Select { list: SelectList },
    Editor { editor: Editor, title: String },
}

pub struct Message {
    pub role: String,
    pub text: String,
}

impl Model {
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height, mode: AppMode::Chat, messages: Vec::new(), is_streaming: false, input: Input::new() }
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
        AppMode::Chat => {
            match key.code {
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
            }
        }
        AppMode::Select { list } => { list.handle_key(&key); }
        AppMode::Editor { editor, .. } => { editor.handle_key(&key); }
    }
    vec![]
}

// ============================================================================
// View
// ============================================================================

pub fn view(model: &Model, frame: &mut Frame) {
    let area = frame.size();

    if let AppMode::Editor { editor, title, .. } = &model.mode {
        render_editor(frame, area, editor, title);
        return;
    }

    let input_h = 5u16;
    let chat_h = area.height.saturating_sub(input_h);
    let chunks = Layout::new(Direction::Vertical, [Constraint::Length(chat_h), Constraint::Length(input_h)]).split(area);

    render_chat(model, frame, chunks[0]);
    render_input(model, frame, chunks[1]);

    if let AppMode::Select { list, .. } = &model.mode {
        let oa = Rect::new(area.width / 4, area.height / 4, area.width / 2, area.height / 2);
        frame.render_widget(Clear, oa);
        list.render_to_frame(frame, oa);
    }
}

fn render_chat(model: &Model, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Chat ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut y = inner.bottom() as i32 - 1;
    for msg in model.messages.iter().rev() {
        let txt = &msg.text;
        let mut lines: Vec<Line<'static>> = Vec::new();
        for l in txt.lines() {
            lines.push(Line::from(Span::raw(l.to_string())));
        }
        if lines.is_empty() { lines.push(Line::from(Span::raw(""))); }

        y -= lines.len() as i32;
        if y + lines.len() as i32 <= inner.top() as i32 { break; }
        let sy = y.max(inner.top() as i32) as u16;

        let role_style = Style::new()
            .fg(match msg.role.as_str() { "user" => Color::Green, "assistant" => Color::Cyan, _ => Color::White })
            .add_modifier(Modifier::BOLD);
        if sy >= inner.top() && sy < inner.bottom() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(format!(" {} ", msg.role), role_style))),
                Rect::new(inner.x, sy, inner.width, 1));
        }
        for (i, ln) in lines.iter().enumerate() {
            let ly = sy + 1 + i as u16;
            if ly < inner.top() || ly >= inner.bottom() { continue; }
            frame.render_widget(Paragraph::new(ln.clone()), Rect::new(inner.x + 1, ly, inner.width.saturating_sub(2), 1));
        }
    }
}

fn render_input(model: &Model, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Input ");
    let style = if model.is_streaming { Style::default().fg(Color::DarkGray) } else { Style::default() };
    let p = Paragraph::new(Text::styled(model.input.value().to_string(), style)).block(block).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
    if !model.is_streaming {
        let cx = area.x + 1 + (model.input.cursor_pos() as u16).min(area.width.saturating_sub(2));
        frame.set_cursor_position((cx, area.y + 1));
    }
}

fn render_editor(frame: &mut Frame, area: Rect, editor: &Editor, title: &str) {
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
        .title(format!(" {title} "));
    let inner = block.inner(area);
    let p = Paragraph::new(Text::raw(editor.text())).block(block);
    frame.render_widget(p, area);
    frame.set_cursor_position((inner.x + editor.cursor_col(), inner.y + editor.cursor_row()));
}

// ============================================================================
// Main loop
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
