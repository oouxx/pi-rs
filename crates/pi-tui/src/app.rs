//! Elm architecture core — Model, Msg, update, view, Cmd.

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::text::Text;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::components::{Completer, Editor, Input, Markdown, SelectList};
use crate::scrollback::EntryLayoutInfo;

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Default truncation budget for tool outputs (lines shown when not
/// expanded).
const MAX_TOOL_LINES: usize = 8;
/// Messages longer than this collapse to a truncated "N more" view.
const MAX_MSG_LINES: usize = 50;
/// Fold threshold for truncated-mode content.
const TRUNCATED_BUDGET: usize = 40;

// ============================================================================
// Theme
// ============================================================================

#[derive(Clone)]
pub struct Theme {
    pub accent: Color, pub user: Color, pub assistant: Color,
    pub tool_running: Color, pub tool_done: Color, pub tool_failed: Color, pub tool_pending: Color,
    pub muted: Color, pub highlight_bg: Color, pub status_bg: Color,
}

impl Theme {
    pub fn default() -> Self {
        Self {
            accent: Color::Cyan, user: Color::Green, assistant: Color::Cyan,
            tool_running: Color::Yellow, tool_done: Color::Green, tool_failed: Color::Red,
            tool_pending: Color::DarkGray, muted: Color::DarkGray, highlight_bg: Color::Cyan,
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
pub struct ToolCall {
    pub id: u64,
    pub name: String, pub state: ToolCallState,
    pub output: String,
    pub approval: Option<ToolApproval>,
}

#[derive(Debug, Clone)]
pub enum ToolCallState { Pending, Running, Done, Failed }

#[derive(Debug, Clone)]
pub enum ToolApproval { Pending, Approved, Denied }

/// How a block is currently displayed (grok scrollback `DisplayMode`).
pub use crate::scrollback::DisplayMode;

pub enum AppMode { Chat, Select { list: SelectList }, Editor { editor: Box<Editor>, title: String } }
pub struct Message {
    /// Stable block id (scrollback navigation / fold state keying).
    pub id: u64,
    pub role: String,
    pub text: String,
    /// Streaming markdown renderer for this message's content.
    md: Markdown,
}

impl Message {
    /// Create a message and feed its initial text through the markdown
    /// pipeline. `width` is the wrap width at construction; the renderer
    /// re-wraps automatically on subsequent width changes.
    pub fn new(id: u64, role: impl Into<String>, text: impl Into<String>, width: usize) -> Self {
        let text = text.into();
        let md = Markdown::new(&text, width);
        Self { id, role: role.into(), text, md }
    }
}

pub struct Dialog { pub title: String, pub message: String, pub buttons: Vec<DialogButton>, pub selected: usize }
pub struct DialogButton { pub label: &'static str, pub action: DialogAction }
pub enum DialogAction { Confirm, ConfirmAlways, Cancel, Custom(&'static str) }

// ============================================================================
// Model
// ============================================================================

pub struct Model {
    pub theme: Theme, pub width: u16, pub height: u16, pub mode: AppMode,
    pub messages: Vec<Message>, pub is_streaming: bool,
    pub input: Input, pub model_name: String, pub tick: u64,
    pub active_tools: Vec<ToolCall>, pub dialog: Option<Dialog>,
    pub completer: Completer, pub scroll_offset: usize, pub auto_scroll: bool,
    pub cwd: String, pub git_branch: Option<String>,
    pub context_usage_pct: u8, pub elapsed_secs: u64,
    pub g_pressed: bool,
    /// Next stable block id (messages and tool calls share one sequence).
    next_block_id: u64,
    /// User-expanded blocks (grok `expanded_groups` — overrides the derived
    /// default folds).
    pub expanded_blocks: std::collections::HashSet<u64>,
    /// User-collapsed blocks (explicit Collapsed override).
    pub collapsed_blocks: std::collections::HashSet<u64>,
}

impl Model {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            theme: Theme::default(), width, height, mode: AppMode::Chat, messages: Vec::new(),
            is_streaming: false, input: Input::new(), model_name: String::new(),
            tick: 0, active_tools: Vec::new(), dialog: None,
            completer: Completer::new(), scroll_offset: 0, auto_scroll: true,
            cwd: String::new(), git_branch: None, context_usage_pct: 0, elapsed_secs: 0,
            g_pressed: false,
            next_block_id: 0,
            expanded_blocks: std::collections::HashSet::new(),
            collapsed_blocks: std::collections::HashSet::new(),
        }
    }

    /// Allocate the next stable block id.
    fn alloc_block_id(&mut self) -> u64 {
        let id = self.next_block_id;
        self.next_block_id = self.next_block_id.wrapping_add(1);
        id
    }

    /// Push a new message, allocating a stable block id (scrollback
    /// navigation / fold state keying) and re-attaching auto-scroll.
    pub fn push_message(&mut self, role: impl Into<String>, text: impl Into<String>) {
        let id = self.alloc_block_id();
        self.messages.push(Message::new(id, role, text, self.width as usize));
        self.auto_scroll = true;
    }

    pub fn add_tool_call(&mut self, name: &str) {
        let id = self.alloc_block_id();
        self.active_tools.push(ToolCall { id, name: name.to_string(), state: ToolCallState::Running, output: String::new(), approval: None });
    }

    pub fn update_tool_call(&mut self, name: &str, state: ToolCallState) {
        if let Some(tool) = self.active_tools.iter_mut().rev().find(|t| t.name == name) { tool.state = state; }
    }

    pub fn append_tool_output(&mut self, name: &str, text: &str) {
        if let Some(tool) = self.active_tools.iter_mut().rev().find(|t| t.name == name) { tool.output.push_str(text); }
    }

    /// Toggle the nearest foldable block (from the bottom) between
    /// collapsed and expanded — the Ctrl+F interaction. Overrides the
    /// derived default fold for that block (grok `expanded_groups`).
    pub fn toggle_block_fold(&mut self, id: u64) {
        if self.collapsed_blocks.remove(&id) {
            self.expanded_blocks.insert(id);
            return;
        }
        if self.expanded_blocks.remove(&id) {
            self.collapsed_blocks.insert(id);
            return;
        }
        // No user override yet — derive the current effective mode from the
        // default fold rules (a collapsed-looking block gets expanded, an
        // expanded one gets collapsed).
        if self.block_derived_collapsed(id) {
            self.expanded_blocks.insert(id);
        } else {
            self.collapsed_blocks.insert(id);
        }
    }

    /// Derived default: whether the block with this id renders collapsed
    /// without user overrides (finished tool calls; messages don't collapse
    /// by default).
    fn block_derived_collapsed(&self, id: u64) -> bool {
        self.active_tools.iter().rev().any(|t| {
            t.id == id
                && matches!(t.state, ToolCallState::Done | ToolCallState::Failed)
        })
    }

    /// The nearest foldable block id from the bottom of the transcript:
    /// finished tool calls first, then any message long enough to fold.
    pub fn nearest_foldable_block(&self) -> Option<u64> {
        // Bottom-up: tools are appended after messages in the transcript
        // order, so check tools first (they are the most recent blocks).
        for tool in self.active_tools.iter().rev() {
            if matches!(tool.state, ToolCallState::Done | ToolCallState::Failed) {
                return Some(tool.id);
            }
        }
        for msg in self.messages.iter().rev() {
            if msg.text.lines().count() > MAX_MSG_LINES {
                return Some(msg.id);
            }
        }
        None
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
    SetGitBranch(Option<String>), SetContextUsage(u8), SetElapsed(u64), SetModelName(String),
    SetEditorText(String), ExitSelect,
    AppendToolOutput(String, String), ToggleBlockFold(u64),
    ToolApprove(String), ToolDeny(String), ToolApprovalPending(String),
    ClearScreen, InputNewline, Cancel,
}

// ============================================================================
// Update
// ============================================================================

pub fn update(model: &mut Model, msg: Msg) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => handle_key(model, key),
        Msg::Resize(w, h) => { model.width = w; model.height = h; vec![] }
        Msg::Paste(text) => { model.input.insert_str(&text); vec![] }
        Msg::NewMessage(role, text) => { model.push_message(role, text); vec![] }
        Msg::StreamText(delta) => { if let Some(m) = model.messages.last_mut() { m.text.push_str(&delta); m.md.append_text(&delta); } vec![] }
        Msg::StreamEnd => { model.is_streaming = false; vec![] }
        Msg::OpenEditor(title, text) => { model.mode = AppMode::Editor { editor: Box::new(Editor::new(&text)), title }; vec![] }
        Msg::EditorDone(_) => { model.mode = AppMode::Chat; vec![] }
        Msg::ToolStart(name) => { model.add_tool_call(&name); vec![] }
        Msg::ToolEnd(name, is_error) => { model.update_tool_call(&name, if is_error { ToolCallState::Failed } else { ToolCallState::Done }); vec![] }
        Msg::Tick => { model.tick += 1; if model.auto_scroll { model.scroll_offset = 0; } vec![] }
        Msg::ScrollUp(amount) => { model.auto_scroll = false; model.scroll_offset = model.scroll_offset.saturating_add(amount as usize); vec![] }
        Msg::ScrollDown(amount) => { model.scroll_offset = model.scroll_offset.saturating_sub(amount as usize); if model.scroll_offset == 0 { model.auto_scroll = true; } vec![] }
        Msg::ScrollToBottom => { model.scroll_offset = 0; model.auto_scroll = true; vec![] }
        Msg::ShowDialog(d) => { model.dialog = Some(d); vec![] }
        Msg::DismissDialog => { model.dialog = None; vec![] }
        Msg::DialogNext => { if let Some(ref mut d) = model.dialog { if d.selected + 1 < d.buttons.len() { d.selected += 1; } } vec![] }
        Msg::DialogPrev => { if let Some(ref mut d) = model.dialog { if d.selected > 0 { d.selected -= 1; } } vec![] }
        Msg::DialogConfirm => { model.dialog.take(); vec![] }
        Msg::SetGitBranch(b) => { model.git_branch = b; vec![] }
        Msg::SetContextUsage(p) => { model.context_usage_pct = p; vec![] }
        Msg::SetElapsed(s) => { model.elapsed_secs = s; vec![] }
        Msg::SetModelName(name) => { model.model_name = name; vec![] }
        Msg::SetEditorText(text) => { model.input.set_value(&text); vec![] }
        Msg::ExitSelect => { model.mode = AppMode::Chat; vec![] }
        Msg::AppendToolOutput(n, t) => { model.append_tool_output(&n, &t); vec![] }
        Msg::ToggleBlockFold(id) => { model.toggle_block_fold(id); vec![] }
        Msg::ToolApprove(n) => { if let Some(t) = model.active_tools.iter_mut().rev().find(|t| t.name == n) { t.approval = Some(ToolApproval::Approved); } vec![] }
        Msg::ToolDeny(n) => { if let Some(t) = model.active_tools.iter_mut().rev().find(|t| t.name == n) { t.approval = Some(ToolApproval::Denied); } vec![] }
        Msg::ToolApprovalPending(n) => { if let Some(t) = model.active_tools.iter_mut().rev().find(|t| t.name == n) { t.approval = Some(ToolApproval::Pending); } vec![] }
        Msg::ClearScreen => { model.messages.clear(); model.active_tools.clear(); model.scroll_offset = 0; vec![] }
        Msg::InputNewline => { model.input.insert_char('\n'); vec![] }
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
                    if let Some(pos) = current.rfind(['/', '@']) {
                        let prefix = &current[..=pos];
                        model.input.clear(); model.input.insert_str(&format!("{prefix}{text} "));
                    }
                }
                model.completer.deactivate();
            }
            KeyCode::Esc => { model.completer.deactivate(); }
            _ => {}
        }
        return vec![];
    }
    if key.code != KeyCode::Char('g') { model.g_pressed = false; }
    // Ctrl+F: fold/unfold the nearest foldable block (grok's fold key).
    if key.code == KeyCode::Char('f') && key.modifiers == crossterm::event::KeyModifiers::CONTROL {
        if let Some(id) = model.nearest_foldable_block() {
            model.toggle_block_fold(id);
            // Keep the viewport anchored (user is inspecting history).
            model.auto_scroll = false;
        }
        return vec![];
    }
    match &mut model.mode {
        AppMode::Chat => match key.code {
            KeyCode::Char(c) => {
                if c == 'g' && model.g_pressed && model.input.value().is_empty() { model.g_pressed = false; model.scroll_offset = usize::MAX; model.auto_scroll = false; return vec![]; }
                if c == 'g' && model.input.value().is_empty() { model.g_pressed = true; return vec![]; }
                if c == 'G' && model.input.value().is_empty() { model.scroll_offset = 0; model.auto_scroll = true; return vec![]; }
                if let Some(t) = Completer::should_activate(c) { model.completer.activate(t, ""); }
                else if model.completer.trigger.is_some() && !c.is_whitespace() { let mut q = model.completer.query.clone(); q.push(c); model.completer.activate(model.completer.trigger.unwrap(), &q); }
                else { model.completer.deactivate(); }
                model.input.insert_char(c);
            }
            KeyCode::Backspace => {
                model.input.backspace();
                if model.completer.trigger.is_some() {
                    let current = model.input.value();
                    if let Some(pos) = current.rfind(['/', '@']) {
                        model.completer.activate(model.completer.trigger.unwrap(), &current[pos + 1..]);
                    } else { model.completer.deactivate(); }
                }
            }
            KeyCode::Tab => { if !model.completer.visible { for _ in 0..4 { model.input.insert_char(' '); } } }
            KeyCode::Enter => {
                if key.modifiers == crossterm::event::KeyModifiers::SHIFT || key.modifiers == crossterm::event::KeyModifiers::ALT {
                    model.input.insert_char('\n');
                } else { model.input.clear(); model.completer.deactivate(); }
            }
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
// View — four-section layout
// ============================================================================

pub fn view(model: &mut Model, frame: &mut Frame) {
    let area = frame.area(); let t = model.theme.clone();
    if let AppMode::Editor { editor, title, .. } = &model.mode {
        render_editor(frame, area, editor, title, &t); return;
    }
    let input_h = input_height(model);
    let chunks = Layout::new(Direction::Vertical, [Constraint::Length(1), Constraint::Min(1), Constraint::Length(input_h), Constraint::Length(1)]).split(area);
    render_header(model, frame, chunks[0], &t);
    render_body(model, frame, chunks[1], &t);
    render_input(model, frame, chunks[2], &t);
    render_status(model, frame, chunks[3], &t);
    if model.dialog.is_some() { render_dialog(model, frame, area, &t); return; }
    if let AppMode::Select { list, .. } = &model.mode {
        let oa = Rect::new(area.width / 4, area.height / 4, area.width / 2, area.height / 2);
        frame.render_widget(Clear, oa); list.render_to_frame(frame, oa);
    }
}

fn input_height(model: &Model) -> u16 { (model.input.value().lines().count() as u16).clamp(1, 5) }

// ============================================================================
// Header
// ============================================================================

fn render_header(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let label = if model.model_name.is_empty() { " pi-rs ".into() } else { format!(" {} ", model.model_name) };
    let sp = if model.is_streaming || !model.active_tools.is_empty() { Some(SPINNER[(model.tick / 3) as usize % SPINNER.len()]) } else { None };
    let status = match (sp, model.is_streaming) {
        (Some(ch), true) => Span::styled(format!(" {ch} streaming "), Style::new().fg(Color::Green)),
        (Some(ch), false) => Span::styled(format!(" {ch} working "), Style::new().fg(Color::Yellow)),
        _ => Span::raw(""),
    };
    let hint = Span::styled(" Ctrl+C:abort x2:quit ", Style::new().fg(t.muted));
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(label, Style::new().fg(t.accent).add_modifier(Modifier::BOLD)), status, Span::raw(" "), hint])).style(Style::new().bg(t.status_bg)), area);
}

// ============================================================================
// Body — fixed-position layout (no overlap)
// ============================================================================

/// One unified scrollback block (message or tool call) with its derived
/// default display mode. Built by [`render_body`], consumed by the fold
/// scan/projection and the render loop.
struct BlockView {
    id: u64,
    role: &'static str,
    default_mode: DisplayMode,
    md_lines: Vec<Line<'static>>,
    tool_name: String,
    tool_state: Option<ToolCallState>,
    tool_approval: Option<ToolApproval>,
    tool_output: String,
}

fn render_body(model: &mut Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let wrap_w = (area.width as usize).saturating_sub(3).max(10);

    // ── 1. Unified block sequence (tools then messages, matching the
    //        historical order) with derived default display modes. ──────
    let mut blocks: Vec<BlockView> = Vec::new();
    for tool in &model.active_tools {
        let default_mode = if matches!(tool.state, ToolCallState::Done | ToolCallState::Failed) {
            DisplayMode::Collapsed
        } else {
            DisplayMode::Expanded
        };
        blocks.push(BlockView {
            id: tool.id,
            role: "tool",
            default_mode,
            md_lines: Vec::new(),
            tool_name: tool.name.clone(),
            tool_state: Some(tool.state.clone()),
            tool_approval: tool.approval.clone(),
            tool_output: tool.output.clone(),
        });
    }
    for msg in &mut model.messages {
        let lines = msg.md.render(wrap_w).to_vec();
        let default_mode = if lines.len() > MAX_MSG_LINES {
            DisplayMode::Truncated
        } else {
            DisplayMode::Expanded
        };
        blocks.push(BlockView {
            id: msg.id,
            role: if msg.role == "user" { "user" } else if msg.role == "system" { "system" } else { "assistant" },
            default_mode,
            md_lines: lines,
            tool_name: String::new(),
            tool_state: None,
            tool_approval: None,
            tool_output: String::new(),
        });
    }

    // ── 2. Fold scan + projection (grok scrollback groups philosophy): ──
    //    one scan derives every fold; `project_to_layout` is the single
    //    writer of the per-entry layout flags the renderer consumes.
    use crate::scrollback::groups::{FoldEntry, project_to_layout, scan};

    let fold_entries: Vec<FoldEntry> = blocks
        .iter()
        .map(|b| FoldEntry {
            id: b.id,
            display_mode: b.default_mode,
            is_tool: b.role == "tool",
            tool_finished: matches!(
                b.tool_state,
                Some(ToolCallState::Done | ToolCallState::Failed)
            ),
            total_lines: 0,
        })
        .collect();
    let spans = scan(&fold_entries, 1, &model.expanded_blocks);

    let mut layout = vec![EntryLayoutInfo::default(); blocks.len()];
    // Seed heights from each block's resolved display mode (projection
    // overrides group members afterwards).
    for (i, b) in blocks.iter().enumerate() {
        let resolved = resolve_mode(model, b);
        layout[i].height = block_height(b, resolved, wrap_w);
        layout[i].gap_after = 1;
    }
    project_to_layout(&spans, &mut layout, 1);

    // ── 3. Render — bottom-up with fixed item heights (unchanged). ─────
    let total_h: u16 = layout.iter().map(|l| l.height + l.gap_after).sum();
    let max_skip = total_h.saturating_sub(area.height.max(1));
    let mut skip = model.scroll_offset.min(max_skip as usize);
    let mut y = area.bottom() as i32 - 1;

    for (idx, item) in blocks.iter().enumerate().rev() {
        let info = layout[idx];
        let h = (info.height + info.gap_after) as i32;
        if skip >= h as usize { skip -= h as usize; continue; }
        let item_top = y - h + 1;
        if item_top + h <= area.top() as i32 { break; }

        let sy = item_top.max(area.top() as i32) as u16;
        let mut ly = sy as i32;

        if info.is_group_header() || info.verb_group_header {
            // Synthetic fold header (verb run / truncation group).
            let label = if info.verb_group_header {
                format!(" \u{25b8} {} tool calls ", info.group_header_count)
            } else {
                format!(" ... {} more ", info.group_header_count)
            };
            if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                frame.render_widget(Paragraph::new(Line::from(Span::styled(label, Style::new().fg(t.muted).add_modifier(Modifier::ITALIC)))), Rect::new(area.x + 2, ly as u16, area.width.saturating_sub(3), 1));
            }
            ly += 1;
            if info.group_collapse_header && info.verb_group_header {
                // Expanded verb group: header + first member row below.
                if item.role == "tool" {
                    if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                        frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {} ", item.tool_name), Style::new().fg(t.tool_done)))), Rect::new(area.x + 1, ly as u16, area.width.saturating_sub(2), 1));
                    }
                    ly += 1;
                }
            }
            y -= h;
            skip = 0;
            continue;
        }
        if info.height == 0 { y -= h; skip = 0; continue; }

        // Effective display mode for this block (user overrides already
        // folded into the layout via resolve_mode + projection).
        let mode = resolve_mode(model, item);

        // Role label
        let rstyle = if item.role == "user" { Style::new().fg(t.user).add_modifier(Modifier::BOLD) }
            else if item.role == "assistant" { Style::new().fg(t.assistant).add_modifier(Modifier::BOLD) }
            else if item.role == "tool" { Style::new().fg(t.tool_running) }
            else { Style::default() };
        if ly >= area.top() as i32 && ly < area.bottom() as i32 {
            frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {} ", item.role), rstyle))), Rect::new(area.x, ly as u16, area.width, 1));
        }
        ly += 1;

        // Tool-specific content
        if item.role == "tool" {
            if let Some(ref ap) = item.tool_approval {
                let txt = match ap {
                    ToolApproval::Pending => " [a] Approve   [d] Deny ",
                    ToolApproval::Approved => " \u{2713} Approved ",
                    ToolApproval::Denied => " \u{2717} Denied ",
                };
                if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                    frame.render_widget(Paragraph::new(Line::from(Span::styled(txt, Style::new().fg(t.tool_running)))), Rect::new(area.x + 2, ly as u16, area.width.saturating_sub(3), 1));
                }
                ly += 1;
            }
            if mode == DisplayMode::Collapsed {
                // Single collapsed tool: name row only (verb groups already
                // handled by the header path above).
                if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                    frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" \u{25b8} {}", item.tool_name), Style::new().fg(t.tool_done)))), Rect::new(area.x + 1, ly as u16, area.width.saturating_sub(2), 1));
                }
                ly += 1;
            } else if !item.tool_output.is_empty() {
                let max = if mode == DisplayMode::Expanded { usize::MAX } else { MAX_TOOL_LINES };
                let total = item.tool_output.lines().count();
                for (i, line) in item.tool_output.lines().enumerate().take(max) {
                    if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                        let trimmed = line.trim_start();
                        let (st, pf) = if trimmed.starts_with('+') && !trimmed.starts_with("+++") { (Style::default().fg(Color::Green), "+") }
                            else if trimmed.starts_with('-') && !trimmed.starts_with("---") { (Style::default().fg(Color::Red), "-") }
                            else { (Style::default().fg(t.muted), " ") };
                        frame.render_widget(Paragraph::new(Line::from(Span::styled(format!("{pf}{line}"), st))), Rect::new(area.x + 3, ly as u16, area.width.saturating_sub(4), 1));
                    }
                    ly += 1;
                }
                if total > max && mode != DisplayMode::Expanded {
                    if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                        frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" ... {} more lines ", total - max), Style::new().fg(t.muted).add_modifier(Modifier::ITALIC)))), Rect::new(area.x + 3, ly as u16, area.width.saturating_sub(4), 1));
                    }
                    ly += 1;
                }
            }
        } else {
            // Chat message text — rendered through the markdown pipeline.
            let max = match mode {
                DisplayMode::Collapsed => 1,
                DisplayMode::Truncated => TRUNCATED_BUDGET,
                DisplayMode::Expanded => usize::MAX,
            };
            let total = item.md_lines.len();
            for line in item.md_lines.iter().take(max) {
                if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                    frame.render_widget(Paragraph::new(line.clone()), Rect::new(area.x + 1, ly as u16, area.width.saturating_sub(2), 1));
                }
                ly += 1;
            }
            if total > max && mode != DisplayMode::Expanded {
                if ly >= area.top() as i32 && ly < area.bottom() as i32 {
                    frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" ... {} more lines ", total - max), Style::new().fg(t.muted).add_modifier(Modifier::ITALIC)))), Rect::new(area.x + 1, ly as u16, area.width.saturating_sub(2), 1));
                }
                ly += 1;
            }
        }

        y -= h; // Move past this item (fixed step)
        skip = 0;
    }

    if total_h == 0 {
        frame.render_widget(Paragraph::new(Line::from(Span::styled(" No messages yet. Type and press Enter.", Style::new().fg(t.muted)))), Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1));
    }
    if model.scroll_offset > 0 {
        let pct = if total_h == 0 { 0 } else { (model.scroll_offset * 100 / total_h as usize).min(100) };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" \u{2191} {pct}% "), Style::new().fg(t.muted)))), Rect::new(area.x + area.width.saturating_sub(12), area.y, 12, 1));
    }
}

/// Resolve a block's effective display mode: user overrides first, then the
/// derived default (grok: user override wins over the fold scan).
fn resolve_mode(model: &Model, b: &BlockView) -> DisplayMode {
    if model.collapsed_blocks.contains(&b.id) {
        DisplayMode::Collapsed
    } else if model.expanded_blocks.contains(&b.id) {
        DisplayMode::Expanded
    } else {
        b.default_mode
    }
}

/// Height of a block in its resolved mode (before group projection).
fn block_height(b: &BlockView, mode: DisplayMode, wrap_w: usize) -> u16 {
    let role = 1u16;
    let approval = if b.role == "tool" && b.tool_approval.is_some() { 1 } else { 0 };
    let content: u16 = match b.role {
        "tool" => {
            if mode == DisplayMode::Collapsed {
                1 // `▸ name`
            } else {
                let total = b.tool_output.lines().count();
                let max = if mode == DisplayMode::Expanded { usize::MAX } else { MAX_TOOL_LINES };
                let shown = total.min(max);
                let more = if total > max { 1 } else { 0 };
                (shown + more) as u16
            }
        }
        _ => {
            let total = b.md_lines.len();
            let max = match mode {
                DisplayMode::Collapsed => 1,
                DisplayMode::Truncated => TRUNCATED_BUDGET,
                DisplayMode::Expanded => usize::MAX,
            };
            let shown = total.min(max);
            let more = if total > max { 1 } else { 0 };
            (shown + more) as u16
        }
    };
    role + approval + content
}

// ============================================================================
// Input
// ============================================================================

fn render_input(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let prompt = if model.is_streaming { "\u{23F3} " } else { "> " };
    let prompt_style = if model.is_streaming { Style::new().fg(Color::Green) } else { Style::new().fg(t.accent) };
    let input_style = if model.is_streaming { Style::default().fg(t.muted) } else { Style::default() };
    let text = model.input.value();
    let cursor_display = model.input.cursor_display_col();
    let pw = 2u16;

    let mut ly = area.y;
    for (ly, (i, line)) in (area.y..).zip(text.lines().enumerate()) {
        if ly >= area.y + area.height {
            break;
        }
        if i == 0 {
            frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(prompt, prompt_style), Span::styled(line.to_string(), input_style)])), Rect::new(area.x, ly, area.width, 1));
        } else {
            frame.render_widget(Paragraph::new(Text::styled(line.to_string(), input_style)), Rect::new(area.x + pw, ly, area.width.saturating_sub(pw), 1));
        }
    }
    if text.is_empty() {
        frame.render_widget(Paragraph::new(Line::from(Span::styled(prompt, prompt_style))), Rect::new(area.x, area.y, area.width, 1));
    }
    if model.completer.visible { model.completer.render(frame, area.x + pw + cursor_display, area.y); }
    if !model.is_streaming {
        let cx = (area.x + pw + cursor_display).min(area.x + area.width.saturating_sub(1));
        let cy = area.y + text.lines().count().saturating_sub(1) as u16;
        frame.set_cursor_position((cx, cy));
    }
}

// ============================================================================
// Status bar
// ============================================================================

fn render_status(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let ml = if model.model_name.is_empty() { "no model" } else { &model.model_name };
    let cwd: String = if model.cwd.is_empty() { "~".into() } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        model.cwd.replace(&home, "~")
    };
    let git = match &model.git_branch { Some(b) => format!(" \u{2395} {b} "), None => String::new() };
    let ctx = if model.context_usage_pct > 0 {
        let pct = model.context_usage_pct.min(100);
        let col = if pct > 90 { Color::Red } else if pct > 70 { Color::Yellow } else { Color::Green };
        Span::styled(format!(" \u{25A0}\u{25A0} {}% ", pct), Style::new().fg(col))
    } else { Span::raw("") };
    let elapsed = if model.elapsed_secs > 0 { format!(" \u{23F1} {}:{:02} ", model.elapsed_secs / 60, model.elapsed_secs % 60) } else { String::new() };
    frame.render_widget(Paragraph::new(Line::from(vec![
        Span::styled(format!(" {ml} "), Style::new().fg(t.accent)),
        Span::styled(" | ", Style::new().fg(t.muted)),
        Span::styled(cwd, Style::new().fg(Color::White)),
        Span::styled(git, Style::new().fg(t.muted)),
        ctx, Span::styled(elapsed, Style::new().fg(t.muted)),
    ])).style(Style::new().bg(t.status_bg)), area);
}

// ============================================================================
// Editor / Dialog
// ============================================================================

fn render_editor(frame: &mut Frame, area: Rect, editor: &Editor, title: &str, _t: &Theme) {
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(format!(" {title} "));
    let inner = block.inner(area);
    // Render via the vendored textarea widget (wrapped-line viewport,
    // selection, scrollbar) inside the title block. Block first so the
    // textarea never paints over the borders.
    frame.render_widget(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(format!(" {title} ")), area);
    frame.render_widget_ref(editor.textarea(), inner);
    if let Some((x, y)) = editor.textarea().cursor_pos(inner) {
        frame.set_cursor_position((inner.x + x, inner.y + y));
    }
}

fn render_dialog(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let d = match &model.dialog { Some(d) => d, None => return };
    frame.render_widget(Clear, area);
    let dw = (area.width / 3 * 2).max(40).min(area.width.saturating_sub(4));
    let dh = 5u16 + d.message.lines().count() as u16;
    let da = Rect::new((area.width - dw) / 2, (area.height - dh) / 2, dw, dh);
    frame.render_widget(Clear, da);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(format!(" {} ", d.title), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)))), Rect::new(da.x, da.y, da.width, 1));
    let inner = Rect::new(da.x, da.y + 1, da.width, da.height.saturating_sub(2));
    let mut y = inner.y;
    for line in d.message.lines() { if y < inner.y + inner.height { frame.render_widget(Paragraph::new(Line::from(Span::raw(line.to_string()))), Rect::new(inner.x, y, inner.width, 1)); y += 1; } }
    let ba = Rect::new(inner.x, da.y + dh - 2, inner.width, 1);
    let tw: usize = d.buttons.iter().map(|b| b.label.len() + 2 + if matches!(b.action, DialogAction::ConfirmAlways) { 2 } else { 0 }).sum();
    let sp = (inner.width as usize).saturating_sub(tw) / (d.buttons.len() + 1).max(1);
    let mut spans = vec![Span::raw(" ".repeat(sp))];
    for (i, btn) in d.buttons.iter().enumerate() {
        let s = if i == d.selected { Style::new().fg(Color::Black).bg(t.highlight_bg) } else { Style::new().fg(Color::White).bg(t.muted) };
        let lbl = if matches!(btn.action, DialogAction::ConfirmAlways) { format!(" {} [A] ", btn.label) } else { format!(" {} ", btn.label) };
        spans.push(Span::styled(lbl, s)); spans.push(Span::raw(" ".repeat(sp)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), ba);
}

// ============================================================================
// Helpers
// ============================================================================

// ============================================================================
// Main loop
// ============================================================================

pub async fn run(mut model: Model, mut terminal: crate::terminal::Terminal, mut input_rx: tokio::sync::mpsc::UnboundedReceiver<KeyEvent>) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::time::{sleep, Duration};
    loop {
        terminal.ratatui_terminal().draw(|frame| view(&mut model, frame))?;
        tokio::select! {
            Some(key) = input_rx.recv() => { let cmds = update(&mut model, Msg::Key(key)); for cmd in cmds { if matches!(cmd, Cmd::Quit) { return Ok(()); } } }
            _ = sleep(Duration::from_millis(50)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Messages must route their text through the markdown pipeline, so
    /// markdown syntax is parsed (emphasis markers stripped) and semantic
    /// spans carry the corresponding style.
    #[test]
    fn messages_render_through_markdown_pipeline() {
        let mut model = Model::new(120, 30);
        update(
            &mut model,
            Msg::NewMessage(
                "assistant".into(),
                "# Title\n\nSome **bold** and `code`.\n".into(),
            ),
        );
        let lines = model.messages[0].md.render(80).to_vec();
        let spans: Vec<_> = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        let plain: String = spans.concat();
        // Heading text present, but the `# ` marker is gone (parsed).
        assert!(plain.contains("Title"), "heading rendered: {plain}");
        assert!(!plain.contains("# Title"), "heading marker must be parsed out");
        // Bold emphasis parsed: asterisks removed, text present.
        assert!(plain.contains("bold"), "bold text present: {plain}");
        assert!(!plain.contains("**"), "emphasis markers must be parsed out");
        // Inline code present.
        assert!(plain.contains("code"), "inline code present: {plain}");

        // The bold span must carry the bold modifier (styled output).
        assert!(
            lines.iter().flat_map(|l| l.spans.iter()).any(|s| {
                s.content.contains("bold") && s.style.add_modifier.contains(Modifier::BOLD)
            }),
            "bold span should be styled BOLD: {lines:?}"
        );
    }

    /// Streaming deltas must be fed into the per-message markdown renderer,
    /// so a reply split into `NewMessage` + `StreamText` deltas still renders
    /// as one parsed markdown document.
    #[test]
    fn streaming_deltas_feed_the_markdown_pipeline() {
        let mut model = Model::new(120, 30);
        update(&mut model, Msg::NewMessage("assistant".into(), "Hello".into()));
        update(&mut model, Msg::StreamText(" **world**".into()));
        assert_eq!(model.messages[0].text, "Hello **world**");

        let lines = model.messages[0].md.render(80).to_vec();
        let plain: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(plain.contains("Hello"), "initial text present: {plain}");
        assert!(plain.contains("world"), "streamed delta present: {plain}");
        assert!(!plain.contains("**"), "streamed emphasis parsed: {plain}");
    }

    /// A finished tool call folds by default (grok verb-run member); Ctrl+F
    /// (toggle_block_fold) overrides: expand → collapse → expand.
    #[test]
    fn finished_tool_collapses_and_toggles() {
        let mut model = Model::new(120, 30);
        update(&mut model, Msg::ToolStart("read".into()));
        update(&mut model, Msg::ToolEnd("read".into(), false));
        let id = model.active_tools[0].id;

        // Derived default: finished tool → collapsed (no user override).
        assert!(model.block_derived_collapsed(id));
        assert!(!model.expanded_blocks.contains(&id));
        assert!(!model.collapsed_blocks.contains(&id));

        // Ctrl+F → expand (user override wins over the derived fold).
        model.toggle_block_fold(id);
        assert!(model.expanded_blocks.contains(&id));

        // Ctrl+F again → collapse.
        model.toggle_block_fold(id);
        assert!(model.collapsed_blocks.contains(&id));
        assert!(!model.expanded_blocks.contains(&id));
    }

    /// `nearest_foldable_block` walks bottom-up and skips running tools.
    #[test]
    fn nearest_foldable_block_prefers_finished_tools() {
        let mut model = Model::new(120, 30);
        update(&mut model, Msg::ToolStart("bash".into()));
        update(&mut model, Msg::ToolEnd("bash".into(), false));
        update(&mut model, Msg::ToolStart("read".into())); // still running
        let bash_id = model.active_tools[0].id;
        assert_eq!(
            model.nearest_foldable_block(),
            Some(bash_id),
            "running tool must be skipped, finished tool found"
        );
    }

    /// A message longer than MAX_MSG_LINES is foldable; shorter ones are
    /// not (nothing for Ctrl+F to act on).
    #[test]
    fn long_messages_are_foldable() {
        let mut model = Model::new(120, 30);
        let long = "line\n".repeat(MAX_MSG_LINES + 10);
        update(&mut model, Msg::NewMessage("assistant".into(), long));
        let long_id = model.messages[0].id;
        assert_eq!(model.nearest_foldable_block(), Some(long_id));

        let mut model2 = Model::new(120, 30);
        update(&mut model2, Msg::NewMessage("assistant".into(), "short".into()));
        assert_eq!(model2.nearest_foldable_block(), None);
    }
}
