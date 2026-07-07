//! Elm architecture core — Model, Msg, update, view, Cmd.
//!
//! Aligned with crates/pi-tui/plans.md design (except markdown/highlight).

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::components::{Completer, Editor, Input, SelectList};

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ============================================================================
// Theme
// ============================================================================

#[derive(Clone)]
pub struct Theme {
    pub accent: Color,
    pub user: Color,
    pub assistant: Color,
    pub tool_running: Color,
    pub tool_done: Color,
    pub tool_failed: Color,
    pub tool_pending: Color,
    pub muted: Color,
    pub highlight_bg: Color,
    pub status_bg: Color,
}

impl Theme {
    pub fn default() -> Self {
        Self {
            accent: Color::Cyan,
            user: Color::Green,
            assistant: Color::Cyan,
            tool_running: Color::Yellow,
            tool_done: Color::Green,
            tool_failed: Color::Red,
            tool_pending: Color::DarkGray,
            muted: Color::DarkGray,
            highlight_bg: Color::Cyan,
            status_bg: Color::Rgb(0x1a, 0x1b, 0x26),
        }
    }
}

// ============================================================================
// Cmd
// ============================================================================

pub enum Cmd { Quit }

// ============================================================================
// State types
// ============================================================================

#[derive(Debug, Clone)]
pub struct ToolCall { pub name: String, pub state: ToolCallState }
#[derive(Debug, Clone)]
pub enum ToolCallState { Pending, Running, Done, Failed }

pub enum AppMode { Chat, Select { list: SelectList }, Editor { editor: Editor, title: String } }
pub struct Message { pub role: String, pub text: String }

pub struct Dialog { pub title: String, pub message: String, pub buttons: Vec<DialogButton>, pub selected: usize }
pub struct DialogButton { pub label: &'static str, pub action: DialogAction }
pub enum DialogAction { Confirm, Cancel, Custom(&'static str) }

// ============================================================================
// Model
// ============================================================================

pub struct Model {
    pub theme: Theme,
    pub width: u16, pub height: u16, pub mode: AppMode,
    pub messages: Vec<Message>, pub is_streaming: bool,
    pub input: Input, pub model_name: String, pub tick: u64,
    pub active_tools: Vec<ToolCall>, pub dialog: Option<Dialog>,
    pub completer: Completer,
    pub scroll_offset: usize, pub auto_scroll: bool,
    /// Cwd shown in status bar.
    pub cwd: String,
    /// Git branch shown in status bar.
    pub git_branch: Option<String>,
}

impl Model {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            theme: Theme::default(),
            width, height, mode: AppMode::Chat, messages: Vec::new(),
            is_streaming: false, input: Input::new(), model_name: String::new(),
            tick: 0, active_tools: Vec::new(), dialog: None,
            completer: Completer::new(), scroll_offset: 0, auto_scroll: true,
            cwd: String::new(), git_branch: None,
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
    SetGitBranch(Option<String>),
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
        Msg::NewMessage(role, text) => { model.messages.push(Message { role, text }); model.auto_scroll = true; vec![] }
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
            if model.auto_scroll { model.scroll_offset = 0; }
            vec![]
        }
        Msg::ScrollUp(amount) => { model.auto_scroll = false; model.scroll_offset = model.scroll_offset.saturating_add(amount as usize); vec![] }
        Msg::ScrollDown(amount) => {
            model.scroll_offset = model.scroll_offset.saturating_sub(amount as usize);
            if model.scroll_offset == 0 { model.auto_scroll = true; }
            vec![]
        }
        Msg::ScrollToBottom => { model.scroll_offset = 0; model.auto_scroll = true; vec![] }
        Msg::ShowDialog(dialog) => { model.dialog = Some(dialog); vec![] }
        Msg::DismissDialog => { model.dialog = None; vec![] }
        Msg::DialogNext => { if let Some(ref mut d) = model.dialog { if d.selected + 1 < d.buttons.len() { d.selected += 1; } } vec![] }
        Msg::DialogPrev => { if let Some(ref mut d) = model.dialog { if d.selected > 0 { d.selected -= 1; } } vec![] }
        Msg::DialogConfirm => { model.dialog.take(); vec![] }
        Msg::SetGitBranch(branch) => { model.git_branch = branch; vec![] }
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
    match &mut model.mode {
        AppMode::Chat => match key.code {
            KeyCode::Char(c) => {
                if let Some(trigger) = Completer::should_activate(c) {
                    model.completer.activate(trigger, "");
                } else if model.completer.trigger.is_some() && !c.is_whitespace() {
                    let mut q = model.completer.query.clone();
                    q.push(c);
                    model.completer.activate(model.completer.trigger.unwrap(), &q);
                } else { model.completer.deactivate(); }
                model.input.insert_char(c);
            }
            KeyCode::Backspace => {
                model.input.backspace();
                if model.completer.trigger.is_some() {
                    let current = model.input.value();
                    if let Some(pos) = current.rfind(|c: char| c == '/' || c == '@') {
                        model.completer.activate(model.completer.trigger.unwrap(), &current[pos + 1..]);
                    } else { model.completer.deactivate(); }
                }
            }
            KeyCode::Tab => { if !model.completer.visible { for _ in 0..4 { model.input.insert_char(' '); } } }
            KeyCode::Enter => { model.input.clear(); model.completer.deactivate(); }
            KeyCode::Delete => model.input.delete(),
            KeyCode::Left => model.input.move_left(),
            KeyCode::Right => model.input.move_right(),
            KeyCode::Up => { if model.input.value().is_empty() { model.scroll_offset = model.scroll_offset.saturating_add(1); model.auto_scroll = false; } else { model.input.move_left(); } }
            KeyCode::Down => { if model.input.value().is_empty() { model.scroll_offset = model.scroll_offset.saturating_sub(1); if model.scroll_offset == 0 { model.auto_scroll = true; } } else { model.input.move_right(); } }
            KeyCode::Home => { if model.input.value().is_empty() { model.scroll_offset = usize::MAX; model.auto_scroll = false; } model.input.move_home(); }
            KeyCode::End => { if model.input.value().is_empty() { model.scroll_offset = 0; model.auto_scroll = true; } model.input.move_end(); }
            KeyCode::PageUp => { model.scroll_offset = model.scroll_offset.saturating_add(20); model.auto_scroll = false; }
            KeyCode::PageDown => { model.scroll_offset = model.scroll_offset.saturating_sub(20); if model.scroll_offset == 0 { model.auto_scroll = true; } }
            _ => {}
        },
        AppMode::Select { list } => { list.handle_key(&key); }
        AppMode::Editor { editor, .. } => { editor.handle_key(&key); }
    }
    vec![]
}

// ============================================================================
// View — four-section layout: header / body / input / status
// ============================================================================

pub fn view(model: &Model, frame: &mut Frame) {
    let area = frame.size();
    let t = &model.theme;

    if let AppMode::Editor { editor, title, .. } = &model.mode {
        render_editor(frame, area, editor, title, t);
        return;
    }

    let input_h = input_height(model);
    let chunks = Layout::new(Direction::Vertical, [
        Constraint::Length(1),          // header
        Constraint::Min(1),             // body
        Constraint::Length(input_h),    // input (dynamic)
        Constraint::Length(1),          // status bar
    ]).split(area);

    render_header(model, frame, chunks[0], t);
    render_body(model, frame, chunks[1], t);
    render_input(model, frame, chunks[2], t);
    render_status(model, frame, chunks[3], t);

    if model.dialog.is_some() { render_dialog(model, frame, area, t); return; }
    if let AppMode::Select { list, .. } = &model.mode {
        let oa = Rect::new(area.width / 4, area.height / 4, area.width / 2, area.height / 2);
        frame.render_widget(Clear, oa);
        list.render_to_frame(frame, oa);
    }
}

fn input_height(model: &Model) -> u16 {
    let lines = model.input.value().lines().count();
    1u16.saturating_add((lines as u16).clamp(1, 5)) // separator + input lines
}

// ============================================================================
// Header
// ============================================================================

fn render_header(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let label = if model.model_name.is_empty() { " pi-rs ".into() } else { format!(" {} ", model.model_name) };
    let sp = if model.is_streaming || !model.active_tools.is_empty() {
        Some(SPINNER[(model.tick / 3) as usize % SPINNER.len()])
    } else { None };
    let status = match (sp, model.is_streaming) {
        (Some(ch), true) => Span::styled(format!(" {ch} streaming "), Style::new().fg(Color::Green)),
        (Some(ch), false) => Span::styled(format!(" {ch} working "), Style::new().fg(Color::Yellow)),
        _ => Span::raw(""),
    };
    let hint = Span::styled(" Ctrl+C:abort x2:quit ", Style::new().fg(t.muted));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
            status, Span::raw(" "), hint,
        ])).style(Style::new().bg(t.status_bg)),
        area,
    );
}

// ============================================================================
// Body
// ============================================================================

fn render_body(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let wrap_w = (area.width as usize).saturating_sub(3).max(10);
    let body_h = area.height as usize;

    struct Item { role: String, lines: Vec<String> }
    let mut items: Vec<Item> = Vec::new();
    for tool in &model.active_tools { items.push(Item { role: "tool".into(), lines: vec![tool.name.clone()] }); }
    for msg in &model.messages { items.push(Item { role: msg.role.clone(), lines: simple_wrap(&msg.text, wrap_w) }); }

    let total_lines: usize = items.iter().map(|it| 1 + it.lines.len()).sum();
    let mut skip = model.scroll_offset;
    let mut y = area.bottom() as i32 - 1;

    for item in items.iter().rev() {
        if skip >= 1 + item.lines.len() { skip -= 1 + item.lines.len(); continue; }
        let remaining = (1 + item.lines.len()).saturating_sub(skip);
        if remaining == 0 || y - remaining as i32 + 1 <= area.top() as i32 - body_h as i32 { break; }
        y = y - remaining as i32 + 1;
        if y + remaining as i32 <= area.top() as i32 { break; }
        let sy = y.max(area.top() as i32) as u16;
        y = sy as i32;

        match item.role.as_str() {
            "tool" => {
                if let Some(tool) = model.active_tools.iter().find(|t| t.name == item.lines[0]) {
                    let sp = SPINNER[(model.tick / 3) as usize % SPINNER.len()];
                    let (icon, col) = match tool.state {
                        ToolCallState::Running => (format!(" {sp} "), t.tool_running),
                        ToolCallState::Done => (" \u{2713} ".into(), t.tool_done),
                        ToolCallState::Failed => (" \u{2717} ".into(), t.tool_failed),
                        ToolCallState::Pending => (" \u{25CB} ".into(), t.tool_pending),
                    };
                    if sy < area.bottom() { frame.render_widget(Paragraph::new(Line::from(Span::styled(format!("{}{}", icon, tool.name), Style::new().fg(col)))), Rect::new(area.x + 1, sy, area.width.saturating_sub(2), 1)); }
                }
                y += 1;
            }
            _ => {
                let rc = match item.role.as_str() { "user" => t.user, "assistant" => t.assistant, _ => Color::White };
                if sy < area.bottom() { frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {} ", item.role), Style::new().fg(rc).add_modifier(Modifier::BOLD)))), Rect::new(area.x, sy, area.width, 1)); }
                y += 1;
                for line in &item.lines {
                    if y >= area.top() as i32 && y < area.bottom() as i32 {
                        let st = if item.role == "tool" { Style::default().fg(t.muted).add_modifier(Modifier::ITALIC) } else { Style::default() };
                        frame.render_widget(Paragraph::new(Line::from(Span::raw(line.clone()))).style(st), Rect::new(area.x + 1, y as u16, area.width.saturating_sub(2), 1));
                    }
                    y += 1;
                }
            }
        }
        skip = 0;
    }

    if total_lines == 0 {
        frame.render_widget(Paragraph::new(Line::from(Span::styled(" No messages yet. Type and press Enter.", Style::new().fg(t.muted)))), Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1));
    }
    if model.scroll_offset > 0 {
        frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" \u{2191} {} ", model.scroll_offset), Style::new().fg(t.muted)))), Rect::new(area.x + area.width.saturating_sub(12), area.y, 12, 1));
    }
    if !model.auto_scroll && model.scroll_offset == 0 {
        frame.render_widget(Paragraph::new(Line::from(Span::styled(" \u{2191} paused ", Style::new().fg(t.muted)))), Rect::new(area.x + area.width.saturating_sub(12), area.y, 12, 1));
    }
}

// ============================================================================
// Input (multi-line aware)
// ============================================================================

fn render_input(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let sep = if model.is_streaming { "\u{23F3} " } else { "> " };
    let sep_style = if model.is_streaming { Style::new().fg(Color::Green) } else { Style::new().fg(t.accent) };

    frame.render_widget(Paragraph::new(Line::from(Span::styled(sep, sep_style))), Rect::new(area.x, area.y, area.width, 1));

    let text = model.input.value();
    let cursor_display = model.input.cursor_display_col();
    let input_style = if model.is_streaming { Style::default().fg(t.muted) } else { Style::default() };

    // Multi-line: split on newlines, render each line
    let input_area_y = area.y + 1;
    for (i, line) in text.lines().enumerate() {
        let ly = input_area_y + i as u16;
        if ly >= area.y + area.height { break; }
        frame.render_widget(Paragraph::new(Text::styled(line.to_string(), input_style)), Rect::new(area.x + 2, ly, area.width.saturating_sub(2), 1));
    }

    if model.completer.visible { model.completer.render(frame, area.x + cursor_display + 2, area.y); }

    if !model.is_streaming {
        let cx = (area.x + 2 + cursor_display).min(area.x + area.width.saturating_sub(1));
        let cy = input_area_y + text.lines().count().saturating_sub(1) as u16;
        frame.set_cursor_position((cx, cy));
    }
}

// ============================================================================
// Status bar — model | cwd | git | context
// ============================================================================

fn render_status(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let model_label = if model.model_name.is_empty() { "no model" } else { &model.model_name };

    let cwd_short: String = if model.cwd.is_empty() { "~".to_string() } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        model.cwd.replace(&home, "~")
    };

    let git = match &model.git_branch {
        Some(b) => format!(" \u{2395} {b} "),
        None => String::new(),
    };

    let line = Line::from(vec![
        Span::styled(format!(" {model_label} "), Style::new().fg(t.accent)),
        Span::styled(" | ", Style::new().fg(t.muted)),
        Span::styled(cwd_short, Style::new().fg(Color::White)),
        Span::styled(git, Style::new().fg(t.muted)),
        Span::raw(" "),
    ]);

    frame.render_widget(Paragraph::new(line).style(Style::new().bg(t.status_bg)), area);
}

// ============================================================================
// Editor / Dialog
// ============================================================================

fn render_editor(frame: &mut Frame, area: Rect, editor: &Editor, title: &str, _t: &Theme) {
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(format!(" {title} "));
    let inner = block.inner(area);
    frame.render_widget(Paragraph::new(Text::raw(editor.text())).block(block), area);
    frame.set_cursor_position((inner.x + editor.cursor_col(), inner.y + editor.cursor_row()));
}

fn render_dialog(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let dialog = match &model.dialog { Some(d) => d, None => return };
    frame.render_widget(Clear, area);
    let dw = (area.width / 3 * 2).max(40).min(area.width.saturating_sub(4));
    let dh = 5u16 + dialog.message.lines().count() as u16;
    let da = Rect::new((area.width - dw) / 2, (area.height - dh) / 2, dw, dh);
    frame.render_widget(Clear, da);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {} ", dialog.title), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))), Rect::new(da.x, da.y, da.width, 1));
    let inner = Rect::new(da.x, da.y + 1, da.width, da.height.saturating_sub(2));
    let mut y = inner.y;
    for line in dialog.message.lines() { if y < inner.y + inner.height { frame.render_widget(Paragraph::new(Line::from(Span::raw(line.to_string()))), Rect::new(inner.x, y, inner.width, 1)); y += 1; } }
    let ba = Rect::new(inner.x, da.y + dh - 2, inner.width, 1);
    let total_w: usize = dialog.buttons.iter().map(|b| b.label.len() + 2).sum();
    let spacing = (inner.width as usize).saturating_sub(total_w) / (dialog.buttons.len() + 1).max(1);
    let mut spans = vec![Span::raw(" ".repeat(spacing))];
    for (i, btn) in dialog.buttons.iter().enumerate() {
        let s = if i == dialog.selected { Style::new().fg(Color::Black).bg(t.highlight_bg) } else { Style::new().fg(Color::White).bg(t.muted) };
        spans.push(Span::styled(format!(" {} ", btn.label), s)); spans.push(Span::raw(" ".repeat(spacing)));
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
        let owned = line.to_string(); let bytes = owned.as_bytes(); let mut start = 0;
        while start < owned.len() {
            let end = (start + width).min(owned.len());
            let brk = if end < owned.len() { let mut p = end; while p > start && bytes[p] != b' ' { p -= 1; } if p > start { p } else { end } } else { end };
            lines.push(owned[start..brk].to_string());
            start = if brk < owned.len() && bytes[brk] == b' ' { brk + 1 } else { brk };
        }
    }
    if lines.is_empty() { lines.push(String::new()); } lines
}

// ============================================================================
// Main loop
// ============================================================================

pub async fn run(
    mut model: Model, mut terminal: crate::terminal::Terminal,
    mut input_rx: tokio::sync::mpsc::UnboundedReceiver<KeyEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::time::{sleep, Duration};
    loop {
        terminal.ratatui_terminal().draw(|frame| view(&model, frame))?;
        tokio::select! {
            Some(key) = input_rx.recv() => { let cmds = update(&mut model, Msg::Key(key)); for cmd in cmds { if let Cmd::Quit = cmd { return Ok(()); } } }
            _ = sleep(Duration::from_millis(50)) => {}
        }
    }
}
