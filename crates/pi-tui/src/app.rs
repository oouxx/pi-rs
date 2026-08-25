//! Elm architecture core — Model, Msg, update, view, Cmd.
//!
//! The view follows the TS original interactive TUI
//! (`packages/coding-agent/src/modes/interactive`): the transcript renders
//! user messages as full-width background boxes, tool calls as
//! state-colored boxes (pending/success/error, output previewed to 10
//! lines when not expanded), assistant text plain; the dock is a status
//! line (spinner), a bordered editor and a two-line footer with the
//! context usage left and the model right-aligned. Colors come from the
//! TS built-in dark theme (see [`crate::theme`]).

use crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::components::{
    Completer, CompletionCommand, CompletionItem, CompletionRequest, CompletionTrigger, Editor, Input, Markdown, SelectList,
};
use crate::theme::Theme;

// ============================================================================
// Edit tool diff rendering (TS `components/diff.ts` `renderDiff`)
// ============================================================================

/// TS `parseDiffLine`: `^([+\-\s])(\s*\d*)\s(.*)$` — leading prefix
/// (`+`/`-`/space), an optional line-number run (kept verbatim, including
/// padStart spaces), then a mandatory whitespace separator, then content.
fn parse_diff_line(line: &str) -> Option<(char, String, &str)> {
    let mut chars = line.chars();
    let prefix = chars.next()?;
    if !matches!(prefix, '+' | '-' | ' ' | '\t') {
        return None;
    }
    let rest = chars.as_str();
    let bytes = rest.as_bytes();
    // (\s*\d*): optional whitespace run, then digits.
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    // Mandatory separator whitespace before content. When the greedy
    // `\s*\d*` run consumed everything, the regex engine backtracks:
    // `\s*` gives up its whitespace so `\s` can match it (`- abc` →
    // lineNum "", content "abc").
    let separator = idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t');
    if separator {
        let line_num = rest[..idx].to_string();
        let content = &rest[idx + 1..];
        Some((prefix, line_num, content))
    } else if idx > 0 && bytes[0] == b' ' {
        // Backtrack: `\s*` takes nothing, `\s` takes the first space.
        Some((prefix, String::new(), &rest[idx..]))
    } else {
        None
    }
}

/// TS `replaceTabs`: tabs render as three spaces.
fn replace_tabs(text: &str) -> String {
    text.replace('\t', "   ")
}

/// TS `renderIntraLineDiff` (word-level diff of a single removed/added line
/// pair): changed tokens render with inverse video; leading whitespace of the
/// first changed token is kept plain so indentation is not highlighted.
fn intra_line_spans(
    old: &str,
    new: &str,
    removed_style: Style,
    added_style: Style,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_words(old, new);
    let mut removed_line: Vec<Span<'static>> = Vec::new();
    let mut added_line: Vec<Span<'static>> = Vec::new();
    let mut first_removed = true;
    let mut first_added = true;

    for change in diff.iter_all_changes() {
        let value = change.value();
        match change.tag() {
            ChangeTag::Delete => {
                let mut v = value.to_string();
                if first_removed {
                    let leading_len = v.len() - v.trim_start().len();
                    if leading_len > 0 {
                        removed_line.push(Span::raw(v[..leading_len].to_string()));
                        v = v[leading_len..].to_string();
                    }
                    first_removed = false;
                }
                if !v.is_empty() {
                    removed_line.push(Span::styled(v, removed_style.add_modifier(Modifier::REVERSED)));
                }
            }
            ChangeTag::Insert => {
                let mut v = value.to_string();
                if first_added {
                    let leading_len = v.len() - v.trim_start().len();
                    if leading_len > 0 {
                        added_line.push(Span::raw(v[..leading_len].to_string()));
                        v = v[leading_len..].to_string();
                    }
                    first_added = false;
                }
                if !v.is_empty() {
                    added_line.push(Span::styled(v, added_style.add_modifier(Modifier::REVERSED)));
                }
            }
            ChangeTag::Equal => {
                removed_line.push(Span::raw(value.to_string()));
                added_line.push(Span::raw(value.to_string()));
            }
        }
    }
    (removed_line, added_line)
}

/// One diff line rendered inside the edit tool box: context lines in
/// `toolDiffContext`, `-` lines in `toolDiffRemoved`, `+` lines in
/// `toolDiffAdded`; a single removed/added pair gets intra-line word
/// highlighting (TS `renderDiff`). Returns the line height consumed.
/// TS `renderDiff` 主循环的一步：渲染 `lines[i]`。`-` 行会前瞻收集连续
/// 的 `-`/`+` 行做分组——恰好 1 删 1 增时做词级 intra-line 高亮（inverse
/// 标记变更 token），否则整组按行着色。返回消费的行数。
fn render_edit_diff_line(
    frame: &mut Frame,
    area: Rect,
    y: i32,
    bg: Color,
    lines: &[&str],
    i: usize,
    t: &Theme,
) -> usize {
    let removed_style = Style::new().fg(t.diff_removed);
    let added_style = Style::new().fg(t.diff_added);
    let context_style = Style::new().fg(t.diff_context);

    let Some((prefix, line_num, content)) = parse_diff_line(lines[i]) else {
        // Non-diff line (blank, "..." skips, unparseable): context color, verbatim.
        render_boxed_row(frame, area, y, bg, vec![Span::styled(lines[i].to_string(), context_style)]);
        return 1;
    };

    match prefix {
        '-' => {
            // Collect consecutive removed lines, then consecutive added lines
            // (TS groups a change block the same way).
            let mut removed: Vec<(String, String)> = Vec::new();
            let mut j = i;
            while j < lines.len() {
                let Some((p, ln, c)) = parse_diff_line(lines[j]) else { break };
                if p != '-' { break; }
                removed.push((ln, c.to_string()));
                j += 1;
            }
            let mut added: Vec<(String, String)> = Vec::new();
            while j < lines.len() {
                let Some((p, ln, c)) = parse_diff_line(lines[j]) else { break };
                if p != '+' { break; }
                added.push((ln, c.to_string()));
                j += 1;
            }
            let consumed = removed.len() + added.len();

            if removed.len() == 1 && added.len() == 1 {
                // Single-line modification: word-level diff with inverse
                // video on the changed tokens (TS `renderIntraLineDiff`).
                let (removed_spans, added_spans) = intra_line_spans(
                    &replace_tabs(&removed[0].1),
                    &replace_tabs(&added[0].1),
                    removed_style,
                    added_style,
                );
                let mut r = vec![Span::styled(format!("-{} ", removed[0].0), removed_style)];
                r.extend(removed_spans);
                render_boxed_row(frame, area, y, bg, r);
                let mut a = vec![Span::styled(format!("+{} ", added[0].0), added_style)];
                a.extend(added_spans);
                render_boxed_row(frame, area, y + 1, bg, a);
            } else {
                let mut ly = y;
                for (ln, content) in &removed {
                    render_boxed_row(
                        frame,
                        area,
                        ly,
                        bg,
                        vec![Span::styled(
                            format!("-{ln} {}", replace_tabs(content)),
                            removed_style,
                        )],
                    );
                    ly += 1;
                }
                for (ln, content) in &added {
                    render_boxed_row(
                        frame,
                        area,
                        ly,
                        bg,
                        vec![Span::styled(
                            format!("+{ln} {}", replace_tabs(content)),
                            added_style,
                        )],
                    );
                    ly += 1;
                }
            }
            consumed
        }
        '+' => {
            render_boxed_row(
                frame,
                area,
                y,
                bg,
                vec![Span::styled(
                    format!("+{line_num} {}", replace_tabs(content)),
                    added_style,
                )],
            );
            1
        }
        _ => {
            render_boxed_row(
                frame,
                area,
                y,
                bg,
                vec![Span::styled(
                    format!(" {line_num} {}", replace_tabs(content)),
                    context_style,
                )],
            );
            1
        }
    }
}

const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Tool-output preview lines when not expanded (TS
/// `FALLBACK_PREVIEW_LINES` = 10: the fallback tool renderer shows the
/// first 10 lines plus `... (N more lines, ctrl+o to expand)`).
const FALLBACK_PREVIEW_LINES: usize = 10;
/// Tool-output tail preview for the bash renderer (TS
/// `BASH_PREVIEW_LINES` = 5 in `core/tools/bash.ts`).
const BASH_PREVIEW_LINES: usize = 5;
/// Horizontal padding inside user/tool boxes (TS `outputPad` = 1).
const BOX_PAD_X: u16 = 1;
/// Vertical padding inside user/tool boxes (TS `Box(padX, 1, bg)`).
const BOX_PAD_Y: u16 = 1;
/// Minimum content rows of the input editor (TS `max(5, 30% rows)`).
const EDITOR_MIN_ROWS: u16 = 5;
/// Context usage thresholds (TS footer: error >90, warning >70).
const CTX_ERROR_PCT: u8 = 90;
const CTX_WARNING_PCT: u8 = 70;

// ============================================================================
// Cmd
// ============================================================================

#[derive(Debug)]
pub enum Cmd {
    Quit,
    /// 请求宿主异步计算补全候选（slash 命令 fuzzy / 命令参数 / `@` 文件走查）。
    RequestCompletion(CompletionRequest),
    /// Ctrl+X（TS `app.message.copy`）：复制最后一条 assistant 消息到系统
    /// 剪贴板——由宿主（interactive 模式）在 agent 任务里执行。
    CopyLastMessage,
}

/// Cumulative token/cost totals for the footer stats line (TS
/// `createUsageTotals()` + `addUsageToTotals()`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    /// Cache hit rate of the latest assistant message (TS
    /// `latestCacheHitRate` — shown as `CH{n}%` when any cache tokens were
    /// used).
    pub cache_hit_rate: Option<f64>,
}

/// Format token counts for the compact footer display (TS
/// `formatTokens()`): `<1000` raw, `<10k` one decimal + `k`, `<1M` rounded
/// `k`, `<10M` one decimal + `M`, else rounded `M`.
pub fn format_tokens(count: u64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    if count < 10_000 {
        return format!("{:.1}k", count as f64 / 1000.0);
    }
    if count < 1_000_000 {
        return format!("{}k", (count as f64 / 1000.0).round() as u64);
    }
    if count < 10_000_000 {
        return format!("{:.1}M", count as f64 / 1_000_000.0);
    }
    format!("{}M", (count as f64 / 1_000_000.0).round() as u64)
}

// ============================================================================
// State types
// ============================================================================

#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Stable block id (transcript ordering / identity).
    pub id: u64,
    /// Agent-side tool call id — the row's identity for state/output
    /// updates. Two calls of the same tool never share a row.
    pub call_id: String,
    pub name: String, pub state: ToolCallState,
    /// JSON-serialized tool arguments (TS `JSON.stringify(args, null, 2)`
    /// — rendered below the title like the TS fallback tool component).
    pub args: String,
    pub output: String,
    /// Output truncation metadata (TS `details.truncation` + full output
    /// path) — renders the warning line for bash/read.
    pub truncation: Option<ToolTruncation>,
    /// Wall-clock start of the execution (TS bash renderer records
    /// `startedAt` in `renderCall`; drives the `Elapsed`/`Took` line).
    pub started_at: Option<std::time::Instant>,
    /// Wall-clock end (TS bash renderer sets `endedAt` when the result is
    /// no longer partial).
    pub ended_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
pub enum ToolCallState { Pending, Running, Done, Failed }

/// TS `DEFAULT_MAX_BYTES` / `DEFAULT_MAX_LINES` (tools/truncate.ts) —
/// fallbacks for the truncation warning text when a tool's details omit
/// `maxBytes` / `maxLines` (TS `?? DEFAULT_MAX_BYTES` / `?? DEFAULT_MAX_LINES`).
const DEFAULT_MAX_BYTES: u64 = 50 * 1024;
const DEFAULT_MAX_LINES: u64 = 2000;

/// Bash/read/grep truncation metadata (TS `TruncationResult` + the tool
/// `details` fields) — renders the warning lines inside the tool box.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolTruncation {
    pub truncated: bool,
    pub truncated_by: Option<String>,
    pub output_lines: u64,
    pub total_lines: u64,
    pub max_lines: u64,
    pub max_bytes: u64,
    pub full_output_path: Option<String>,
    /// read: first line exceeded the byte limit (TS `firstLineExceedsLimit`).
    pub first_line_exceeds_limit: bool,
    /// grep: match limit reached (TS `details.matchLimitReached`).
    pub match_limit_reached: Option<u64>,
    /// grep: some lines were truncated (TS `details.linesTruncated`).
    pub lines_truncated: Option<bool>,
}

/// Format a byte count like TS `formatSize` (`B` / `KB` / `MB`).
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Strip ANSI escape sequences (TS `stripAnsi` in utils/ansi.ts): CSI
/// (`ESC [ ... final`) and OSC (`ESC ] ... ST` where ST is BEL, `ESC \`,
/// or `0x9C`).
fn strip_ansi(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{1b}' || c == '\u{9b}' {
            // OSC: `ESC ]` ... ST (BEL / `ESC \` / `0x9C`).
            if c == '\u{1b}' && i + 1 < chars.len() && chars[i + 1] == ']' {
                i += 2;
                while i < chars.len() {
                    let s = chars[i];
                    if s == '\u{07}' || s == '\u{9c}' {
                        i += 1;
                        break;
                    }
                    if s == '\u{1b}' && i + 1 < chars.len() && chars[i + 1] == '\\' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            // CSI: optional intro bytes `[\]()#;?`, optional params
            // `\d{1,4}([;:]\d{0,4})*`, then one final byte
            // `[\dA-PR-TZcf-nq-uy=><~]`.
            let mut j = i + 1;
            if c == '\u{1b}' {
                if j < chars.len() && chars[j] == '[' {
                    j += 1;
                } else {
                    out.push(c);
                    i += 1;
                    continue;
                }
            }
            while j < chars.len() && matches!(chars[j], '[' | ']' | '(' | ')' | '#' | ';' | '?') {
                j += 1;
            }
            if j < chars.len() && chars[j].is_ascii_digit() {
                let mut digits = 0usize;
                while j < chars.len() && chars[j].is_ascii_digit() && digits < 4 {
                    j += 1;
                    digits += 1;
                }
                loop {
                    if j < chars.len() && (chars[j] == ';' || chars[j] == ':') {
                        j += 1;
                        digits = 0;
                        while j < chars.len() && chars[j].is_ascii_digit() && digits < 4 {
                            j += 1;
                            digits += 1;
                        }
                    } else {
                        break;
                    }
                }
            }
            if j < chars.len() && is_csi_final(chars[j]) {
                i = j + 1;
            } else {
                // Not a valid CSI sequence — keep the ESC/CSI introducer.
                out.push(c);
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// CSI final byte class from the TS `stripAnsi` regex
/// (`[\dA-PR-TZcf-nq-uy=><~]` — `cf-nq` is `c` plus `f..=n` plus `q`, so
/// the common SGR final byte `m` is included).
fn is_csi_final(c: char) -> bool {
    matches!(
        c,
        '0'..='9'
            | 'A'..='P'
            | 'R'..='T'
            | 'Z'
            | 'c'
            | 'f'..='n'
            | 'q'
            | 'u'..='y'
            | '='
            | '>'
            | '<'
            | '~'
    )
}

/// Drop control characters that would break terminal rendering, keeping
/// `\t` `\n` `\r` (TS `sanitizeBinaryOutput` in utils/shell.ts).
fn sanitize_binary_output(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            let code = c as u32;
            if code == 0x09 || code == 0x0A || code == 0x0D {
                return true;
            }
            if code <= 0x1F {
                return false;
            }
            if (0xFFF9..=0xFFFB).contains(&code) {
                return false;
            }
            true
        })
        .collect()
}

/// Sanitize tool output for display (TS `getTextOutput` in
/// tools/render-utils.ts): `sanitizeBinaryOutput(stripAnsi(text))` then
/// drop `\r`.
fn sanitize_output_text(text: &str) -> String {
    sanitize_binary_output(&strip_ansi(text)).replace('\r', "")
}

/// Format a duration like TS `formatDuration` (`(ms / 1000).toFixed(1) +
/// "s"`): one decimal plus the unit, rounding half away from zero (JS
/// `toFixed`).
fn format_duration(ms: u128) -> String {
    format!("{:.1}s", (ms as f64 / 1000.0 * 10.0).round() / 10.0)
}


pub enum AppMode { Chat, Select { list: SelectList }, Editor { editor: Box<Editor>, title: String } }
/// Terminal stop reason of an assistant message (TS `StopReason`), used to
/// render the TS post-content notices (truncated/aborted/error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Length,
    Aborted,
    Error,
}

pub struct Message {
    /// Stable block id (transcript ordering / identity).
    pub id: u64,
    pub role: String,
    pub text: String,
    /// Streaming markdown renderer for this message's content.
    md: Markdown,
    /// Thinking content (TS `thinking` content blocks) — rendered in
    /// `thinkingText` italic below the text, like the TS
    /// `AssistantMessageComponent`.
    pub thinking: String,
    /// Streaming markdown renderer for the thinking content.
    thinking_md: Markdown,
    /// Terminal stop reason — renders the TS post-content notice.
    pub stop_reason: Option<StopReason>,
    /// Provider error message (TS `errorMessage`).
    pub error_message: Option<String>,
}

impl Message {
    /// Create a message and feed its initial text through the markdown
    /// pipeline. `width` is the wrap width at construction; the renderer
    /// re-wraps automatically on subsequent width changes.
    pub fn new(id: u64, role: impl Into<String>, text: impl Into<String>, width: usize) -> Self {
        let text = text.into();
        let md = Markdown::new(&text, width);
        let thinking_md = Markdown::new("", width);
        Self {
            id,
            role: role.into(),
            text,
            md,
            thinking: String::new(),
            thinking_md,
            stop_reason: None,
            error_message: None,
        }
    }

    /// Append a thinking delta (TS `thinking_delta` events) — routed
    /// through the thinking markdown pipeline.
    pub fn append_thinking(&mut self, delta: &str) {
        self.thinking.push_str(delta);
        self.thinking_md.append_text(delta);
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
    /// Context usage percentage (TS `contextUsage.percent`, one decimal).
    pub context_usage_pct: f64,
    /// Whether context usage is known (TS shows `?/window` when null).
    pub context_usage_known: bool,
    pub elapsed_secs: u64,
    pub g_pressed: bool,
    /// Next stable block id (messages and tool calls share one sequence).
    next_block_id: u64,
    // ── TS footer data (FooterComponent) ────────────────────────────────
    /// Session display name — appended to the pwd line (`pwd • name`).
    pub session_name: Option<String>,
    /// Cumulative token/cost totals (TS `usageTotals`).
    pub usage_totals: UsageTotals,
    /// Context window of the active model (TS `contextWindow`).
    pub context_window: u64,
    /// Auto-compaction enabled — appends ` (auto)` to the context display.
    pub auto_compact: bool,
    /// Current thinking level (TS `state.thinkingLevel`).
    pub thinking_level: Option<String>,
    /// Provider of the active model (TS `state.model.provider`).
    pub provider: Option<String>,
    /// Whether the active model supports reasoning (TS `model.reasoning`).
    pub reasoning: bool,
    // ── Startup header (TS builtInHeader ExpandableText) ────────────────
    /// Whether the startup header is visible (TS shows it unless quiet
    /// startup is configured).
    pub show_header: bool,
    /// Ctrl+O (app.tools.expand): expands the header and all tool outputs.
    pub tool_output_expanded: bool,
    /// Last Ctrl+C press time — two presses within 500ms quit (TS
    /// `handleCtrlC`); the first press clears the editor.
    pub last_ctrl_c: Option<std::time::Instant>,
}

impl Model {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            theme: Theme::default(), width, height, mode: AppMode::Chat, messages: Vec::new(),
            is_streaming: false, input: Input::new(), model_name: String::new(),
            tick: 0, active_tools: Vec::new(), dialog: None,
            completer: Completer::new(), scroll_offset: 0, auto_scroll: true,
            cwd: String::new(), git_branch: None, context_usage_pct: 0.0,
            context_usage_known: false, elapsed_secs: 0,
            g_pressed: false,
            next_block_id: 0,
            session_name: None,
            usage_totals: UsageTotals::default(),
            context_window: 0,
            auto_compact: true,
            thinking_level: None,
            provider: None,
            reasoning: false,
            show_header: true,
            tool_output_expanded: false,
            last_ctrl_c: None,
        }
    }

    /// Allocate the next stable block id.
    fn alloc_block_id(&mut self) -> u64 {
        let id = self.next_block_id;
        self.next_block_id = self.next_block_id.wrapping_add(1);
        id
    }

    /// Push a new message, allocating a stable block id and re-attaching
    /// auto-scroll.
    pub fn push_message(&mut self, role: impl Into<String>, text: impl Into<String>) {
        let id = self.alloc_block_id();
        self.messages.push(Message::new(id, role, text, self.width as usize));
        self.auto_scroll = true;
    }

    pub fn add_tool_call(&mut self, call_id: &str, name: &str, args: &str) {
        let id = self.alloc_block_id();
        self.active_tools.push(ToolCall {
            id,
            call_id: call_id.to_string(),
            name: name.to_string(),
            state: ToolCallState::Running,
            args: args.to_string(),
            output: String::new(),
            truncation: None,
            started_at: Some(std::time::Instant::now()),
            ended_at: None,
        });
    }

    pub fn update_tool_call(&mut self, call_id: &str, state: ToolCallState) {
        if let Some(tool) = self.active_tools.iter_mut().rev().find(|t| t.call_id == call_id) { tool.state = state; }
    }

    /// Replace a tool row's output with a full snapshot (the bridge sends
    /// the accumulated text on every update — appending would duplicate).
    /// The text is sanitized for display exactly like the TS renderers'
    /// `getTextOutput` (strip ANSI, drop binary control characters, remove
    /// `\r`) — the raw tool result keeps ANSI for the model, only the TUI
    /// display cleans it.
    pub fn set_tool_output(&mut self, call_id: &str, text: &str) {
        if let Some(tool) = self.active_tools.iter_mut().rev().find(|t| t.call_id == call_id) {
            tool.output = sanitize_output_text(text);
        }
    }
}

// ============================================================================
// Msg
// ============================================================================

pub enum Msg {
    Key(KeyEvent), Resize(u16, u16), Paste(String),
    NewMessage(String, String), StreamText(String), StreamEnd,
    /// Finalize the streaming assistant message: thinking content, terminal
    /// stop reason and provider error message (TS `Done`/`Error` events).
    MessageEnd {
        thinking: String,
        stop_reason: Option<StopReason>,
        error_message: Option<String>,
    },
    OpenEditor(String, String), EditorDone(String),
    ToolStart(String, String, String), ToolEnd(String, String, bool),
    Tick,
    ScrollUp(u16), ScrollDown(u16), ScrollToBottom,
    ShowDialog(Dialog), DismissDialog, DialogNext, DialogPrev, DialogConfirm,
    SetGitBranch(Option<String>), SetContextUsage(f64), SetContextUsageKnown(bool),
    SetElapsed(u64), SetModelName(String),
    SetEditorText(String), ExitSelect,
    SetToolOutput(String, String, String),
    SetToolTruncation(String, Option<ToolTruncation>),
    ClearScreen, InputNewline, Cancel,
    // ── TS footer/header data ────────────────────────────────────────────
    SetSessionName(Option<String>),
    SetUsageTotals(UsageTotals),
    SetContextWindow(u64),
    SetThinkingLevel(Option<String>),
    SetProvider(Option<String>),
    SetReasoning(bool),
    /// Ctrl+O (app.tools.expand): toggle the startup header and tool
    /// output expansion.
    ToggleToolExpansion,
    /// Switch the active palette (the `/theme` command). Replaces the
    /// `Theme` wholesale so every surface re-reads its colors next frame.
    SetTheme(Theme),
    /// 宿主异步计算的补全候选回填（`seq` 对齐 [`crate::components::Completer::request_seq`]，
    /// 过期结果被丢弃，等价 TS AbortController）。
    CompletionResults { seq: u64, items: Vec<CompletionItem> },
}

// ============================================================================
// Update
// ============================================================================

pub fn update(model: &mut Model, msg: Msg) -> Vec<Cmd> {
    match msg {
        Msg::Key(key) => {
            let mut cmds = handle_key(model, key);
            if let Some(cmd) = completion_request_after_key(model, &key) {
                cmds.push(cmd);
            }
            cmds
        }
        Msg::CompletionResults { seq, items } => {
            if items.is_empty() {
                if seq == model.completer.request_seq {
                    // 对齐 TS getSuggestions 返回 null：无候选 → 关闭弹窗。
                    model.completer.deactivate();
                }
            } else {
                model.completer.apply_results(seq, items);
            }
            vec![]
        }
        Msg::Resize(w, h) => { model.width = w; model.height = h; vec![] }
        Msg::Paste(text) => { model.input.insert_str(&text); vec![] }
        Msg::NewMessage(role, text) => { model.push_message(role, text); vec![] }
        Msg::StreamText(delta) => { if let Some(m) = model.messages.last_mut() { m.text.push_str(&delta); m.md.append_text(&delta); } vec![] }
        Msg::MessageEnd { thinking, stop_reason, error_message } => {
            if let Some(m) = model.messages.last_mut() {
                if !thinking.is_empty() {
                    m.thinking = thinking;
                    m.thinking_md = Markdown::new(&m.thinking, model.width as usize);
                }
                m.stop_reason = stop_reason;
                m.error_message = error_message;
            }
            model.is_streaming = false;
            vec![]
        }
        Msg::StreamEnd => { model.is_streaming = false; vec![] }
        Msg::OpenEditor(title, text) => { model.mode = AppMode::Editor { editor: Box::new(Editor::new(&text)), title }; vec![] }
        Msg::EditorDone(_) => { model.mode = AppMode::Chat; vec![] }
        Msg::ToolStart(call_id, name, args) => { model.add_tool_call(&call_id, &name, &args); vec![] }
        Msg::ToolEnd(call_id, _name, is_error) => {
            model.update_tool_call(&call_id, if is_error { ToolCallState::Failed } else { ToolCallState::Done });
            if let Some(t) = model.active_tools.iter_mut().rev().find(|t| t.call_id == call_id) {
                t.ended_at = Some(std::time::Instant::now());
            }
            vec![]
        }
        Msg::Tick => { model.tick += 1; vec![] }
        // TS ScrollView: `scroll_offset` = viewport top offset in content
        // rows (0 = top; max = bottom). ScrollUp looks at older content
        // (offset decreases), ScrollDown newer; reaching the bottom
        // re-engages follow (grok/TS `follow_by_overscroll` semantics —
        // the clamp+re-engage happens in render_body).
        Msg::ScrollUp(amount) => {
            model.auto_scroll = false;
            model.scroll_offset = model.scroll_offset.saturating_sub(amount as usize);
            vec![]
        }
        Msg::ScrollDown(amount) => {
            model.scroll_offset = model.scroll_offset.saturating_add(amount as usize);
            vec![]
        }
        Msg::ScrollToBottom => { model.auto_scroll = true; vec![] }
        Msg::ShowDialog(d) => { model.dialog = Some(d); vec![] }
        Msg::DismissDialog => { model.dialog = None; vec![] }
        Msg::DialogNext => { if let Some(ref mut d) = model.dialog { if d.selected + 1 < d.buttons.len() { d.selected += 1; } } vec![] }
        Msg::DialogPrev => { if let Some(ref mut d) = model.dialog { if d.selected > 0 { d.selected -= 1; } } vec![] }
        Msg::DialogConfirm => { model.dialog.take(); vec![] }
        Msg::SetGitBranch(b) => { model.git_branch = b; vec![] }
        Msg::SetContextUsage(p) => { model.context_usage_pct = p; model.context_usage_known = true; vec![] }
        Msg::SetContextUsageKnown(known) => { model.context_usage_known = known; vec![] }
        Msg::SetElapsed(s) => { model.elapsed_secs = s; vec![] }
        Msg::SetModelName(name) => { model.model_name = name; vec![] }
        Msg::SetSessionName(name) => { model.session_name = name; vec![] }
        Msg::SetUsageTotals(totals) => { model.usage_totals = totals; vec![] }
        Msg::SetContextWindow(w) => { model.context_window = w; vec![] }
        Msg::SetThinkingLevel(level) => { model.thinking_level = level; vec![] }
        Msg::SetProvider(provider) => { model.provider = provider; vec![] }
        Msg::SetReasoning(on) => { model.reasoning = on; vec![] }
        Msg::ToggleToolExpansion => {
            model.tool_output_expanded = !model.tool_output_expanded;
            vec![]
        }
        Msg::SetTheme(theme) => { model.theme = theme; vec![] }
        Msg::SetEditorText(text) => { model.input.set_value(&text); vec![] }
        Msg::ExitSelect => { model.mode = AppMode::Chat; vec![] }
        Msg::SetToolOutput(call_id, _name, text) => { model.set_tool_output(&call_id, &text); vec![] }
        Msg::SetToolTruncation(call_id, truncation) => {
            if let Some(t) = model.active_tools.iter_mut().rev().find(|t| t.call_id == call_id) {
                t.truncation = truncation;
            }
            vec![]
        }
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
            KeyCode::Down => { model.completer.next(); return vec![]; }
            KeyCode::Up => { model.completer.prev(); return vec![]; }
            // TS `tui.input.tab` / `tui.select.confirm`：Tab/Enter 都应用
            // 选中项（TS 中 Tab 是"应用"，不是"下一个"；Down 才是下一个）。
            KeyCode::Tab | KeyCode::Enter => {
                if model.completer.has_fresh_results() {
                    if let Some(new_value) = model.completer.apply_selected(model.input.value()) {
                        model.input.set_value(&new_value);
                    }
                }
                model.completer.deactivate();
                return vec![];
            }
            KeyCode::Esc => { model.completer.deactivate(); return vec![]; }
            // Typing/editing keys fall through to the normal path below,
            // which inserts the character; `completion_request_after_key`
            // then re-queries（等价 TS updateAutocomplete）。
            _ => {}
        }
    }
    if key.code != KeyCode::Char('g') { model.g_pressed = false; }
    // Ctrl+O: toggle tool output expansion + the startup header (TS
    // `app.tools.expand`).
    if key.code == KeyCode::Char('o') && key.modifiers == crossterm::event::KeyModifiers::CONTROL {
        model.tool_output_expanded = !model.tool_output_expanded;
        return vec![];
    }
    // 应用级快捷键（Ctrl+C/Ctrl+D/Ctrl+V/Ctrl+X）只在下栏编辑器聚焦时生效
    // ——TS 把它们注册在 defaultEditor（CustomEditor.handleInput）上；对话框/
    // 选择器各自处理自己的键（对话框 Editor 的 ctrl+d/v/x 由 textarea 原生
    // 处理：delete-char-forward / 剪贴板粘贴 / 剪切选区）。
    if matches!(model.mode, AppMode::Chat) {
        // Ctrl+X: app.message.copy（TS `handleCopyCommand`）——复制最后一条
        // assistant 消息，执行在宿主（interactive 模式）的 agent 任务里。
        if key.code == KeyCode::Char('x') && key.modifiers == crossterm::event::KeyModifiers::CONTROL {
            return vec![Cmd::CopyLastMessage];
        }
        // Ctrl+C: app.clear（对齐 TS `handleCtrlC`）——500ms 内连按两次退出，
        // 否则清空编辑器并关闭补全弹窗（TS setText 会 cancelAutocomplete）。
        if key.code == KeyCode::Char('c') && key.modifiers == crossterm::event::KeyModifiers::CONTROL {
            let now = std::time::Instant::now();
            if model
                .last_ctrl_c
                .map(|t| now.duration_since(t) < std::time::Duration::from_millis(500))
                .unwrap_or(false)
            {
                model.last_ctrl_c = None;
                return vec![Cmd::Quit];
            }
            model.last_ctrl_c = Some(now);
            model.input.clear();
            model.completer.deactivate();
            return vec![];
        }
        // Ctrl+D: app.exit（对齐 TS `handleCtrlD`，CustomEditor 只在空编辑器
        // 时触发）——编辑器为空时退出；非空时是 delete-char-forward（删除
        // 光标后的字符），不是插入字面 'd'。
        if key.code == KeyCode::Char('d') && key.modifiers == crossterm::event::KeyModifiers::CONTROL {
            if model.input.value().is_empty() {
                return vec![Cmd::Quit];
            }
            model.input.delete();
            return vec![];
        }
        // Ctrl+V: app.clipboard.pasteImage 的文本路径（对齐 TS
        // `handleClipboardPaste` → `readClipboardText`）——从系统剪贴板读
        // 文本插入光标处；读不到时静默忽略。图片路径（readClipboardImage）
        // 未复刻，见 DEVIATIONS.md。
        if key.code == KeyCode::Char('v') && key.modifiers == crossterm::event::KeyModifiers::CONTROL {
            if let Some(text) = crate::clipboard::read_clipboard_text() {
                model.input.insert_str(&text);
            }
            return vec![];
        }
    }
    match &mut model.mode {
        AppMode::Chat => match key.code {
            KeyCode::Char(c) => {
                if c == 'g' && model.g_pressed && model.input.value().is_empty() { model.g_pressed = false; model.scroll_offset = 0; model.auto_scroll = false; return vec![]; }
                if c == 'g' && model.input.value().is_empty() { model.g_pressed = true; return vec![]; }
                if c == 'G' && model.input.value().is_empty() { model.auto_scroll = true; return vec![]; }
                model.input.insert_char(c);
            }
            KeyCode::Backspace => {
                model.input.backspace();
            }
            KeyCode::Tab => {
                // Tab 触发补全（对齐 TS `handleTabCompletion`）：请求在
                // `completion_request_after_key` 里按 force 处理。
            }
            KeyCode::Enter => {
                if key.modifiers == crossterm::event::KeyModifiers::SHIFT || key.modifiers == crossterm::event::KeyModifiers::ALT {
                    model.input.insert_char('\n');
                } else { model.input.clear(); model.completer.deactivate(); }
            }
            KeyCode::Delete => model.input.delete(),
            KeyCode::Left => model.input.move_left(),
            KeyCode::Right => model.input.move_right(),
            // 空输入时方向键滚动 transcript（TS ScrollView 语义）：
            // Up/PageUp/Home 看更早内容（offset 减），Down/PageDown/End 看
            // 更新内容（offset 增，到底恢复跟随）。
            KeyCode::Up => { if model.input.value().is_empty() { model.scroll_offset = model.scroll_offset.saturating_sub(1); model.auto_scroll = false; } else { model.input.move_left(); } }
            KeyCode::Down => { if model.input.value().is_empty() { model.scroll_offset = model.scroll_offset.saturating_add(1); } else { model.input.move_right(); } }
            KeyCode::Home => { if model.input.value().is_empty() { model.scroll_offset = 0; model.auto_scroll = false; } model.input.move_home(); }
            KeyCode::End => { if model.input.value().is_empty() { model.auto_scroll = true; } model.input.move_end(); }
            KeyCode::PageUp => { model.scroll_offset = model.scroll_offset.saturating_sub(20); model.auto_scroll = false; }
            KeyCode::PageDown => { model.scroll_offset = model.scroll_offset.saturating_add(20); }
            _ => {}
        },
        AppMode::Select { list } => { list.handle_key(&key); }
        AppMode::Editor { editor, .. } => { editor.handle_key(&key); }
    }
    vec![]
}

// ============================================================================
// Completion trigger context（对齐 TS editor.ts 触发规则 + getSuggestions）
// ============================================================================

/// 从当前输入文本推断补全上下文（TS `getSuggestions` 的前缀判定）。
/// 返回 `(trigger, prefix, query, command, debounce_ms)`；`None` = 不应补全。
fn completion_context(
    text: &str,
    force: bool,
) -> Option<(CompletionTrigger, String, String, Option<String>, u64)> {
    // `/` 行首：命令名（无空格）或命令参数（有空格）。
    if let Some(after) = text.strip_prefix('/') {
        if let Some(space) = after.find(' ') {
            let (cmd, arg) = (&after[..space], &after[space + 1..]);
            return Some((
                CompletionTrigger::Argument,
                arg.to_string(),
                arg.to_string(),
                Some(cmd.to_string()),
                0,
            ));
        }
        return Some((CompletionTrigger::Slash, text.to_string(), after.to_string(), None, 0));
    }

    // `@` token（行首或空白后，对齐 TS `extractAtPrefix` + token 边界触发）。
    if let Some((prefix, raw)) = at_token_prefix(text) {
        return Some((CompletionTrigger::At, prefix, raw, None, 20));
    }

    // Tab 强制：非 slash 上下文 → 路径/附件补全（TS forceFileAutocomplete）。
    if force {
        let last = crate::completion::find_last_delimiter(text);
        let start = last.map_or(0, |i| i + 1);
        let token = &text[start..];
        if !token.is_empty() {
            return Some((CompletionTrigger::At, token.to_string(), token.to_string(), None, 0));
        }
    }
    None
}

/// `@` token 前缀（对齐 TS `extractAtPrefix`）：`@"..."` 或 token 边界 `@...`。
fn at_token_prefix(text: &str) -> Option<(String, String)> {
    if let Some(q) = crate::completion::extract_quoted_prefix(text) {
        if let Some(raw) = q.strip_prefix("@\"") {
            let prefix = q.clone();
            return Some((prefix, raw.to_string()));
        }
    }
    let last = crate::completion::find_last_delimiter(text);
    let start = last.map_or(0, |i| i + 1);
    if text.as_bytes().get(start) == Some(&b'@') {
        let prefix = text[start..].to_string();
        return Some((prefix.clone(), prefix[1..].to_string()));
    }
    None
}

/// 输入变化后：若补全上下文成立则发起一次异步请求；不成立则关闭弹窗。
fn completion_request_after_key(model: &mut Model, key: &KeyEvent) -> Option<Cmd> {
    let force = key.code == crossterm::event::KeyCode::Tab;
    let text = model.input.value().to_string();
    let Some((trigger, prefix, query, command, debounce_ms)) = completion_context(&text, force) else {
        if model.completer.visible {
            model.completer.deactivate();
        }
        return None;
    };

    // 命令参数状态变化：命令不存在或没注册参数补全 → 不弹（对齐 TS 返回 null）。
    if trigger == CompletionTrigger::Argument {
        let has = command
            .as_ref()
            .and_then(|name| {
                model
                    .completer
                    .commands
                    .iter()
                    .find(|c| c.insert_text == *name)
            })
            .is_some_and(|c| c.argument_completions.is_some());
        if !has {
            if model.completer.visible {
                model.completer.deactivate();
            }
            return None;
        }
    }

    // 同状态（如方向键/无变化）不重复请求。
    if model.completer.visible
        && model.completer.trigger == Some(trigger)
        && model.completer.prefix == prefix
    {
        return None;
    }

    // Slash 命令候选在内存里，同步算出并回填——避免输入速度快于异步结果
    // 时 Enter 应用到过期选中项（TS 侧 debounce 为 0 且 fuzzy 同步，行为
    // 等价；这里直接同步更稳）。
    if trigger == CompletionTrigger::Slash {
        let items = slash_completion_items(&model.completer.commands, &query);
        if items.is_empty() {
            if model.completer.visible {
                model.completer.deactivate();
            }
            return None;
        }
        model.completer.begin(trigger, &prefix, &query);
        let seq = model.completer.request_seq;
        model.completer.apply_results(seq, items);
        return None;
    }

    model.completer.begin(trigger, &prefix, &query);
    let seq = model.completer.request_seq;
    Some(Cmd::RequestCompletion(CompletionRequest {
        seq,
        trigger,
        prefix,
        query,
        command,
        debounce_ms,
        force,
    }))
}

/// slash 命令候选：fuzzy 过滤（对齐 TS `fuzzyFilter` on command name）。
fn slash_completion_items(commands: &[CompletionCommand], query: &str) -> Vec<CompletionItem> {
    let idx = crate::fuzzy::fuzzy_filter_indices(commands, query, |c| c.insert_text.clone());
    idx.into_iter()
        .map(|(i, _)| {
            let c = &commands[i];
            CompletionItem::new(c.insert_text.clone(), c.label.clone(), c.description.clone())
        })
        .collect()
}

// ============================================================================
// View — TS interactive layout: transcript / status / editor / footer
// ============================================================================

pub fn view(model: &mut Model, frame: &mut Frame) {
    let area = frame.area(); let t = model.theme.clone();
    // ratatui 0.29 的 `Terminal` 每帧不重置当前 buffer：diff 渲染只重绘
    // 变化的 cell，未重绘的 cell 会保留两个帧前写入的内容。正文/工具块/
    // dock 的行高变化或 Span 被裁剪后，被空出的行必须显式清除，否则旧
    // 字符会残留。整屏先 Clear（缓冲写入，diff 仍只输出变化 cell）。
    frame.render_widget(Clear, area);
    if matches!(&model.mode, AppMode::Editor { .. }) {
        let title = if let AppMode::Editor { title, .. } = &model.mode {
            title.clone()
        } else {
            String::new()
        };
        render_fullscreen_editor(model, frame, area, &title, &t);
        return;
    }
    let (status_h, editor_h, footer_h) = dock_heights(model, area);
    let chunks = Layout::new(Direction::Vertical, [Constraint::Min(1), Constraint::Length(status_h), Constraint::Length(editor_h), Constraint::Length(footer_h)]).split(area);
    render_body(model, frame, chunks[0], &t);
    render_status(model, frame, chunks[1], &t);
    render_input(model, frame, chunks[2], &t);
    render_footer(model, frame, chunks[3], &t);
    if model.dialog.is_some() { render_dialog(model, frame, area, &t); return; }
    if let AppMode::Select { list, .. } = &model.mode {
        // Overlays center inside the terminal area (alt screen = the
        // whole visible screen).
        let oa_h = (area.height / 2).clamp(1, area.height.max(1));
        let oa_y = (area.height.saturating_sub(oa_h)) / 2;
        let oa = Rect::new(area.width / 4, oa_y, area.width / 2, oa_h);
        frame.render_widget(Clear, oa); list.render_to_frame(frame, oa, &t);
    }
}

/// Dock heights (status / editor / footer), TS-style: the status area is
/// always two rows (blank + spinner line while busy), the editor grows with
/// the input up to `max(5, 30% of height)` content rows plus its two
/// borders, the footer is two rows. The transcript keeps at least one row.
/// While the completion menu is open it renders inside the editor (TS
/// SelectList), so the editor grows by the menu rows too.
fn dock_heights(model: &Model, area: Rect) -> (u16, u16, u16) {
    let status_h = 2u16;
    let footer_h = 2u16;
    let content = input_layout_rows(&model.input, editor_layout_width(area.width)).0.len() as u16;
    // The editor grows to `max(5, 30% of terminal rows)` — the TS editor
    // measures the screen (in alt-screen mode the frame is the screen).
    let rows = area.height.max(1);
    let max_visible = EDITOR_MIN_ROWS.max(rows * 30 / 100);
    let visible = content.clamp(1, max_visible);
    let completion = completer_rows(&model.completer);
    let editor_h = 1 + visible + completion + 1;
    let available = area.height.saturating_sub(1).saturating_sub(status_h + footer_h);
    let editor_h = editor_h.min(available.max(3));
    (status_h, editor_h.max(3), footer_h)
}

/// Rows the completion menu occupies inside the editor (TS SelectList:
/// up to 5 items, plus a `(n/m)` scroll row, or the no-match row).
fn completer_rows(completer: &Completer) -> u16 {
    if !completer.visible {
        return 0;
    }
    if completer.results.is_empty() {
        return 1; // "No matching commands"
    }
    let total = completer.results.len();
    let shown = total.min(5);
    let scroll = if total > 5 { 1 } else { 0 };
    (shown + scroll) as u16
}

fn editor_layout_width(area_width: u16) -> usize {
    (area_width.saturating_sub(1) as usize).max(1)
}

// ============================================================================
// Status area — spinner + "Working..." (TS statusContainer / Loader)
// ============================================================================

fn render_status(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    if area.height == 0 { return; }
    let busy = model.is_streaming
        || model
            .active_tools
            .iter()
            .any(|t| matches!(t.state, ToolCallState::Running | ToolCallState::Pending));
    // Row 0 is a blank spacer row (TS Loader renders `["", spinner line]`).
    if area.height > 1 {
        let line = if busy {
            let ch = SPINNER[(model.tick / 3) as usize % SPINNER.len()];
            Line::from(vec![
                Span::styled(format!("{ch} "), Style::new().fg(t.accent)),
                Span::styled("Working...", Style::new().fg(t.muted)),
            ])
        } else {
            Line::raw("")
        };
        frame.render_widget(Paragraph::new(line), Rect::new(area.x, area.y + 1, area.width, 1));
    }
}

// ============================================================================
// Body — transcript with TS-style message boxes
// ============================================================================

/// One transcript block (message or tool call). Built by [`render_body`]
/// and consumed by the flat layout + render loop (no folding — TS
/// original renders every block in full).
struct BlockView {
    role: &'static str,
    md_lines: Vec<Line<'static>>,
    /// Plain text lines for system notices (rendered muted, no markdown).
    plain_lines: Vec<String>,
    tool_name: String,
    tool_args: String,
    /// Session cwd — the read/grep/edit renderers display paths shortened
    /// against it (TS `shortenPath`).
    cwd: String,
    tool_state: Option<ToolCallState>,
    tool_output: String,
    tool_truncation: Option<ToolTruncation>,
    /// Execution timer (TS bash renderer startedAt/endedAt).
    tool_started_at: Option<std::time::Instant>,
    tool_ended_at: Option<std::time::Instant>,
    /// Assistant thinking content (TS `thinking` blocks) — rendered in
    /// `thinkingText` italic below the text.
    thinking_lines: Vec<Line<'static>>,
    /// Terminal stop reason of an assistant message (TS post-content
    /// notices: truncated/aborted/error).
    stop_reason: Option<StopReason>,
    error_message: Option<String>,
}

/// Startup header lines (TS `builtInHeader` ExpandableText): the logo,
/// one-line keybinding hints (compact) or the full list (expanded), and
/// the onboarding line. Key names are dim, descriptions muted — the same
/// split the TS `keyHint` helper produces.
fn header_lines(expanded: bool, t: &Theme) -> Vec<Line<'static>> {
    let hint = |key: &str, desc: &str| {
        Line::from(vec![
            Span::styled(key.to_string(), Style::new().fg(t.dim)),
            Span::styled(format!(" {desc}"), Style::new().fg(t.muted)),
        ])
    };
    let logo = Line::from(vec![
        Span::styled("Pi", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(" v{}", env!("CARGO_PKG_VERSION")),
            Style::new().fg(t.dim),
        ),
    ]);
    let onboarding = Line::from(Span::styled(
        "Pi can explain its own features and look up its docs. Ask it how to use or extend Pi.",
        Style::new().fg(t.dim),
    ));
    if !expanded {
        let compact = Line::from(vec![
            Span::styled("escape", Style::new().fg(t.dim)),
            Span::styled(" interrupt", Style::new().fg(t.muted)),
            Span::styled(" · ", Style::new().fg(t.muted)),
            Span::styled("ctrl+c/ctrl+d", Style::new().fg(t.dim)),
            Span::styled(" clear/exit", Style::new().fg(t.muted)),
            Span::styled(" · ", Style::new().fg(t.muted)),
            Span::styled("/", Style::new().fg(t.dim)),
            Span::styled(" commands", Style::new().fg(t.muted)),
            Span::styled(" · ", Style::new().fg(t.muted)),
            Span::styled("!", Style::new().fg(t.dim)),
            Span::styled(" bash", Style::new().fg(t.muted)),
            Span::styled(" · ", Style::new().fg(t.muted)),
            Span::styled("ctrl+o", Style::new().fg(t.dim)),
            Span::styled(" more", Style::new().fg(t.muted)),
        ]);
        let press = Line::from(Span::styled(
            "Press ctrl+o to show full startup help and loaded resources.",
            Style::new().fg(t.dim),
        ));
        return vec![logo, compact, press, Line::raw(""), onboarding];
    }
    vec![
        logo,
        hint("escape", "to interrupt"),
        hint("ctrl+c", "to clear"),
        hint("ctrl+c twice", "to exit"),
        hint("ctrl+d", "to exit (empty)"),
        hint("ctrl+z", "to suspend"),
        hint("ctrl+k", "to delete to end"),
        hint("shift+tab", "to cycle thinking level"),
        hint("ctrl+p/shift+ctrl+p", "to cycle models"),
        hint("ctrl+l", "to select model"),
        hint("ctrl+o", "to expand tools"),
        hint("ctrl+t", "to expand thinking"),
        hint("ctrl+g", "for external editor"),
        hint("/", "for commands"),
        hint("!", "to run bash"),
        hint("!!", "to run bash (no context)"),
        hint("alt+enter", "to queue follow-up"),
        hint("alt+up", "to edit all queued messages"),
        hint("ctrl+v", "to paste image (with text fallback)"),
        hint("drop files", "to attach"),
        Line::raw(""),
        onboarding,
    ]
}

/// Build the transcript blocks in TS chatContainer order: header first,
/// then messages and tool calls interleaved by their shared block-id
/// sequence (a tool call sits right after the assistant message that
/// requested it).
fn build_blocks(model: &mut Model, wrap_w: usize, t: &Theme) -> Vec<BlockView> {
    let mut blocks: Vec<BlockView> = Vec::new();
    // Startup header first (TS documentContainer: headerContainer above
    // chatContainer — it scrolls away as content grows).
    if model.show_header {
        blocks.push(BlockView {
            role: "header",
            md_lines: header_lines(model.tool_output_expanded, t),
            plain_lines: Vec::new(),
            tool_name: String::new(),
            tool_args: String::new(),
            cwd: String::new(),
            tool_state: None,
            tool_output: String::new(),
            tool_truncation: None,
            tool_started_at: None,
            tool_ended_at: None,
            thinking_lines: Vec::new(),
            stop_reason: None,
            error_message: None,
        });
    }
    // Tools and messages share one block-id sequence (allocated in event
    // order), so sorting by id reproduces the TS chatContainer ordering.
    let mut pending: Vec<(u64, BlockView)> = Vec::new();
    for tool in &model.active_tools {
        pending.push((
            tool.id,
            BlockView {
                role: "tool",
                md_lines: Vec::new(),
                plain_lines: Vec::new(),
                tool_name: tool.name.clone(),
                tool_args: tool.args.clone(),
                cwd: model.cwd.clone(),
                tool_state: Some(tool.state.clone()),
                tool_output: tool.output.clone(),
                tool_truncation: tool.truncation.clone(),
                tool_started_at: tool.started_at,
                tool_ended_at: tool.ended_at,
                thinking_lines: Vec::new(),
                stop_reason: None,
                error_message: None,
            },
        ));
    }
    for msg in &mut model.messages {
        let lines = msg.md.render(wrap_w).to_vec();
        let role = if msg.role == "user" { "user" } else if msg.role == "system" { "system" } else { "assistant" };
        let thinking_lines = if role == "assistant" && !msg.thinking.is_empty() {
            msg.thinking_md.render(wrap_w).to_vec()
        } else {
            Vec::new()
        };
        pending.push((
            msg.id,
            BlockView {
                role,
                md_lines: lines,
                plain_lines: if msg.role == "system" {
                    msg.text.lines().map(str::to_string).collect()
                } else {
                    Vec::new()
                },
                tool_name: String::new(),
                tool_args: String::new(),
                cwd: String::new(),
                tool_state: None,
                tool_output: String::new(),
                tool_truncation: None,
                tool_started_at: None,
                tool_ended_at: None,
                thinking_lines,
                stop_reason: msg.stop_reason,
                error_message: msg.error_message.clone(),
            },
        ));
    }
    pending.sort_by_key(|(id, _)| *id);
    blocks.extend(pending.into_iter().map(|(_, b)| b));
    blocks
}

/// Inter-block blank rows, mirroring the TS `addMessageToChat` spacers.
///
/// Returns `(gaps, leads)`:
/// - `gaps[i]`: a trailing blank after each non-assistant block. TS boxes
///   (user / tool) are followed by the assistant response, which itself
///   opens with an internal `Spacer(1)` when it has visible content.
/// - `leads[i]`: a leading blank before a user block that directly follows
///   an assistant block. TS inserts `new Spacer(1)` *before* a user message
///   whenever the chat already has content — i.e. a blank line at the turn
///   boundary (end of the previous assistant message -> start of the next
///   user message). Without it, consecutive turns render back to back with
///   no separation, unlike the TS original.
fn block_gaps(blocks: &[BlockView]) -> (Vec<u16>, Vec<u16>) {
    let gaps: Vec<u16> = blocks
        .iter()
        .map(|b| if b.role == "assistant" { 0 } else { 1 })
        .collect();
    let leads: Vec<u16> = std::iter::once(0)
        .chain(blocks.windows(2).map(|w| {
            if w[1].role == "user" && w[0].role == "assistant" {
                1
            } else {
                0
            }
        }))
        .collect();
    (gaps, leads)
}

fn render_body(model: &mut Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let wrap_w = (area.width as usize).saturating_sub((BOX_PAD_X * 2 + 2) as usize).max(10);
    let blocks = build_blocks(model, wrap_w, t);
    let expanded = model.tool_output_expanded;
    let heights: Vec<u16> = blocks
        .iter()
        .map(|b| block_height(b, expanded, wrap_w))
        .collect();
    let (gaps, leads) = block_gaps(&blocks);

    // Scroll-window rendering (TS ScrollView semantics): `scroll_offset` is
    // the viewport-top offset in content rows (0 = top, max = follow
    // bottom). The content is laid out top-down; every row renderer clips
    // to `area` (`render_body_row` / `render_boxed_row`), so rows above the
    // viewport (negative `ly`) and below it are skipped automatically —
    // this is the internal scrollback: the transcript lives in the model,
    // not the terminal.
    let total_h: u16 = heights
        .iter()
        .zip(&gaps)
        .zip(&leads)
        .map(|((h, g), l)| h + g + l)
        .sum();
    let max_scroll = (total_h as usize).saturating_sub(area.height as usize);
    // Clamp + overscroll re-engage: a scroll-down that lands at the bottom
    // re-enables follow on this frame (grok `follow_by_overscroll`), so a
    // fast PageDown ending there keeps streaming content pinned to bottom.
    // Follow state mirrors the viewport position (TS ScrollView keeps
    // scrollTop == maxScrollTop while following), so ScrollUp from the
    // bottom starts at the real bottom rather than at 0.
    let scroll = if model.auto_scroll {
        model.scroll_offset = max_scroll;
        max_scroll
    } else {
        let clamped = model.scroll_offset.min(max_scroll);
        if model.scroll_offset >= max_scroll {
            model.auto_scroll = true;
        }
        clamped
    };
    let mut ly = area.top() as i32 - scroll as i32;
    for (idx, item) in blocks.iter().enumerate() {
        let h = (heights[idx] + gaps[idx] + leads[idx]) as i32;
        if h <= 0 {
            continue;
        }
        // Render at the block's top offset by its leading gap (the TS blank
        // at the assistant->user turn boundary).
        let render_y = ly + leads[idx] as i32;
        match item.role {
            "tool" => _ = render_tool_block(frame, area, item, expanded, t, render_y, wrap_w),
            "user" => _ = render_user_block(frame, area, item, t, render_y),
            "system" => _ = render_system_block(frame, area, item, t, render_y),
            "header" => _ = render_header_block(frame, area, item, render_y),
            _ => _ = render_assistant_block(frame, area, item, t, render_y),
        }
        ly += h;
    }

    if blocks.is_empty() {
        // 空转录提示行。view() 开头已整屏 Clear，这里无需再擦除。
        frame.render_widget(Paragraph::new(Line::from(Span::styled(" No messages yet. Type and press Enter.", Style::new().fg(t.muted)))), Rect::new(area.x + BOX_PAD_X, area.y + 1, area.width.saturating_sub(2), 1));
    }
}

/// Render one row into the body if it lies inside the body area.
fn render_body_row(frame: &mut Frame, area: Rect, y: i32, widget: impl ratatui::widgets::Widget) {
    if y >= area.top() as i32 && y < area.bottom() as i32 {
        frame.render_widget(widget, Rect::new(area.x, y as u16, area.width, 1));
    }
}

/// Which TS tool renderer applies (`core/tools/{bash,read,grep,edit}.ts`);
/// tools without a registered renderer use the generic fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolRenderer {
    Bash,
    Read,
    Grep,
    Edit,
    Fallback,
}

fn tool_renderer(b: &BlockView) -> ToolRenderer {
    match b.tool_name.as_str() {
        "bash" => ToolRenderer::Bash,
        "read" => ToolRenderer::Read,
        "grep" => ToolRenderer::Grep,
        "edit" => ToolRenderer::Edit,
        _ => ToolRenderer::Fallback,
    }
}

/// Bash `command` display arg (TS `str()` in render-utils.ts: a non-string
/// value is invalid → `[invalid arg]`, missing/empty → `...`).
enum BashCommandArg {
    /// Non-string `command` value (TS `str()` returns null → `[invalid arg]`).
    Invalid,
    /// String value (possibly empty — TS renders empty/missing as `...`).
    Text(String),
}

/// Extract `command` / `timeout` from the bash args JSON (TS `bashSchema`).
///
/// `timeout` keeps the exact JSON number (fractional seconds allowed) and is
/// `None` when missing or falsy — TS `formatBashCall` uses a truthy check
/// (`timeout ? \` (timeout ${timeout}s)\` : ""`), so `0` renders no suffix
/// and `1.5` renders ` (timeout 1.5s)`.
fn bash_call_args(args: &str) -> (BashCommandArg, Option<f64>) {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
    let command = match v.get("command") {
        Some(c) if c.is_string() => BashCommandArg::Text(c.as_str().unwrap_or("").to_string()),
        Some(_) => BashCommandArg::Invalid,
        None => BashCommandArg::Text(String::new()),
    };
    let timeout = v
        .get("timeout")
        .and_then(|t| t.as_f64())
        .filter(|t| *t != 0.0 && t.is_finite());
    (command, timeout)
}

/// Visible display width of a text run (CJK wide glyphs count two columns;
/// tabs are already expanded by the caller). Equivalent to TS `visibleWidth`.
fn visible_width(text: &str) -> usize {
    text.chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// Break an over-long token into pieces each fitting `width` columns,
/// never splitting a wide glyph (TS `breakLongWord`).
fn break_long_token(token: &str, width: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut start = 0usize;
    let mut col = 0usize;
    for (idx, ch) in token.char_indices() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w > width && col > 0 {
            pieces.push(token[start..idx].to_string());
            start = idx;
            col = 0;
        }
        col += w;
    }
    pieces.push(token[start..].to_string());
    pieces
}

/// Word-wrap tool output into display-width visual lines, mirroring the TS
/// `Text` renderer (`wrapTextWithAnsi` + `truncateToVisualLines`):
///
/// - tabs expand to 3 spaces (TS `Text.render`),
/// - lines break at word boundaries; an over-long token breaks at the
///   column limit (CJK-aware),
/// - every visual line is right-trimmed,
/// - empty input yields no lines (TS `truncateToVisualLines` returns `[]`).
fn visual_lines(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for raw_line in text.split('\n') {
        let line = raw_line.replace('\t', "   ");
        if visible_width(&line) <= width {
            out.push(line.trim_end().to_string());
            continue;
        }
        // Tokenize into whitespace / non-whitespace runs (TS
        // `splitIntoTokensWithAnsi` on ANSI-free text).
        let mut tokens: Vec<&str> = Vec::new();
        let mut rest: &str = &line;
        while !rest.is_empty() {
            let is_ws = rest.starts_with(char::is_whitespace);
            let idx = rest
                .find(|c: char| c.is_whitespace() != is_ws)
                .unwrap_or(rest.len());
            tokens.push(&rest[..idx]);
            rest = &rest[idx..];
        }
        let mut current = String::new();
        let mut current_w = 0usize;
        let mut wrapped: Vec<String> = Vec::new();
        for token in tokens {
            let token_w = visible_width(token);
            let is_ws = token.chars().all(|c| c.is_whitespace());
            if token_w > width && !is_ws {
                // Flush the current line, then break the long token; the
                // last piece becomes the current line (TS `breakLongWord`).
                if !current.is_empty() {
                    wrapped.push(current.trim_end().to_string());
                    current.clear();
                }
                let pieces = break_long_token(token, width);
                if let Some((last, rest)) = pieces.split_last() {
                    for p in rest {
                        wrapped.push(p.clone());
                    }
                    current = last.clone();
                    current_w = visible_width(&current);
                }
                continue;
            }
            if current_w + token_w > width && current_w > 0 {
                wrapped.push(current.trim_end().to_string());
                if is_ws {
                    current.clear();
                    current_w = 0;
                } else {
                    current = token.to_string();
                    current_w = token_w;
                }
            } else {
                current.push_str(token);
                current_w += token_w;
            }
        }
        if !current.is_empty() {
            wrapped.push(current.trim_end().to_string());
        }
        out.extend(wrapped);
    }
    out
}

/// First `limit` raw lines of `text`, word-wrapped into display rows, plus
/// the number of raw lines hidden beyond the limit. TS tool renderers
/// (read/grep/fallback) count the preview budget in *raw* lines but the
/// `Text` component word-wraps each one at render width.
fn preview_wrapped(text: &str, wrap_w: usize, limit: usize) -> (Vec<String>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let shown = total.min(limit);
    let mut rows = Vec::new();
    for line in lines.iter().take(shown) {
        rows.extend(visual_lines(line, wrap_w));
    }
    (rows, total - shown)
}

/// Bash display text: TS `rebuildBashResultRenderComponent` trims the
/// output and strips the model-facing truncation footer
/// (`\n\n[Showing ... Full output: path]`) when the truncation metadata
/// carries the full output path — the warning line is re-rendered from
/// `details` instead of showing the footer twice.
fn bash_display_text(b: &BlockView) -> String {
    let mut output = b.tool_output.trim().to_string();
    if let Some(t) = &b.tool_truncation {
        if t.truncated && t.full_output_path.is_some() && output.ends_with(']') {
            if let Some(footer_start) = output.rfind("\n\n[") {
                let footer = &output[footer_start..];
                let path = t.full_output_path.as_deref().unwrap_or("");
                if footer.contains(path) {
                    output = output[..footer_start].trim_end().to_string();
                }
            }
        }
    }
    output
}

/// Warning rows for the truncation metadata (blank separator + the
/// `[Full output: ...]` / `[Truncated: ...]` line) — TS bash/read
/// renderers append `\n` + warning text before the timer.
fn bash_warning_rows(b: &BlockView) -> u16 {
    match &b.tool_truncation {
        Some(t) if t.truncated || t.full_output_path.is_some() => 2,
        _ => 0,
    }
}

/// The warning line text (TS `warnings.join(". ")` wrapped in brackets).
fn bash_warning_text(b: &BlockView) -> Option<String> {
    let t = b.tool_truncation.as_ref()?;
    if !t.truncated && t.full_output_path.is_none() {
        return None;
    }
    let mut warnings: Vec<String> = Vec::new();
    if let Some(path) = &t.full_output_path {
        warnings.push(format!("Full output: {path}"));
    }
    if t.truncated {
        if t.truncated_by.as_deref() == Some("lines") {
            warnings.push(format!(
                "Truncated: showing {} of {} lines",
                t.output_lines, t.total_lines
            ));
        } else {
            // TS falls back to DEFAULT_MAX_BYTES when maxBytes is missing.
            let max_bytes = if t.max_bytes != 0 { t.max_bytes } else { DEFAULT_MAX_BYTES };
            warnings.push(format!(
                "Truncated: {} lines shown ({} limit)",
                t.output_lines,
                format_size(max_bytes)
            ));
        }
    }
    Some(format!("[{}]", warnings.join(". ")))
}

/// Whether the read block shows its content: TS `formatReadResult` returns
/// "" when collapsed, but always renders when the tool errored.
fn read_content_visible(b: &BlockView, expanded: bool) -> bool {
    (expanded || matches!(b.tool_state, Some(ToolCallState::Failed)))
        && !b.tool_output.trim().is_empty()
}

/// read truncation warnings (TS `formatReadResult`): `[First line exceeds
/// X limit]`, `[Truncated: showing N of M lines (N line limit)]`, or
/// `[Truncated: N lines shown (X limit)]`.
fn read_warning_text(b: &BlockView) -> Option<String> {
    let t = b.tool_truncation.as_ref()?;
    if !t.truncated {
        return None;
    }
    if t.first_line_exceeds_limit {
        let max_bytes = if t.max_bytes != 0 { t.max_bytes } else { DEFAULT_MAX_BYTES };
        return Some(format!("[First line exceeds {} limit]", format_size(max_bytes)));
    }
    if t.truncated_by.as_deref() == Some("lines") {
        let max_lines = if t.max_lines != 0 { t.max_lines } else { DEFAULT_MAX_LINES };
        return Some(format!(
            "[Truncated: showing {} of {} lines ({} line limit)]",
            t.output_lines, t.total_lines, max_lines
        ));
    }
    let max_bytes = if t.max_bytes != 0 { t.max_bytes } else { DEFAULT_MAX_BYTES };
    Some(format!(
        "[Truncated: {} lines shown ({} limit)]",
        t.output_lines,
        format_size(max_bytes)
    ))
}

/// grep truncation warnings (TS `formatGrepResult`): `[Truncated: ...]`
/// with the parts joined by ", ".
fn grep_warning_text(b: &BlockView) -> Option<String> {
    let t = b.tool_truncation.as_ref()?;
    let mut warnings: Vec<String> = Vec::new();
    if let Some(ml) = t.match_limit_reached {
        warnings.push(format!("{ml} matches limit"));
    }
    if t.truncated {
        let max_bytes = if t.max_bytes != 0 { t.max_bytes } else { DEFAULT_MAX_BYTES };
        warnings.push(format!("{} limit", format_size(max_bytes)));
    }
    if t.lines_truncated == Some(true) {
        warnings.push("some lines truncated".to_string());
    }
    if warnings.is_empty() {
        None
    } else {
        Some(format!("[Truncated: {}]", warnings.join(", ")))
    }
}

/// Bash output preview (TS `BASH_PREVIEW_LINES` = 5): the *tail* visual
/// lines are kept when not expanded; `skipped` counts the hidden leading
/// lines (shown as an `... (N earlier lines, ctrl+o to expand)` hint).
/// The model-facing truncation footer is stripped first (TS
/// `rebuildBashResultRenderComponent`).
fn bash_preview(b: &BlockView, wrap_w: usize, expanded: bool) -> (usize, usize) {
    let visual = visual_lines(&bash_display_text(b), wrap_w);
    if expanded || visual.len() <= BASH_PREVIEW_LINES {
        (visual.len(), 0)
    } else {
        (BASH_PREVIEW_LINES, visual.len() - BASH_PREVIEW_LINES)
    }
}

/// Height of a block. Matches the renderers row for row: user blocks are a
/// `Box(padX=1, padY=1)` (top pad + content + bottom pad), tool blocks the
/// same plus a title row and the (preview-truncated) output, assistant /
/// system blocks are plain lines with no padding. Messages are never
/// truncated — the transcript scrolls (TS ScrollView), and only tool
/// output has the TS preview budget when not expanded.
fn block_height(b: &BlockView, expanded: bool, wrap_w: usize) -> u16 {
    match b.role {
        "header" => b.md_lines.len() as u16,
        "tool" => {
            match tool_renderer(b) {
                ToolRenderer::Bash => {
                    // bash renderer: title + (blank + hint + tail preview
                    // when output non-empty) + warnings + blank +
                    // Elapsed/Took line.
                    let (_shown, skipped) = bash_preview(b, wrap_w, expanded);
                    let hint = if skipped > 0 { 1 } else { 0 };
                    let warns = bash_warning_rows(b);
                    let out_rows = if bash_display_text(b).is_empty() {
                        0
                    } else {
                        1 + hint as u16 + _shown as u16
                    };
                    BOX_PAD_Y + 1 + out_rows + warns + 1 + 1 + BOX_PAD_Y
                }
                ToolRenderer::Read => {
                    // read renderer: title only when collapsed and not an
                    // error (TS `formatReadResult` returns ""); when
                    // visible: blank + up to 10 wrapped raw lines + more
                    // hint + truncation warnings.
                    let content = if read_content_visible(b, expanded) {
                        let (rows, remaining) =
                            preview_wrapped(b.tool_output.trim_end(), wrap_w, 10);
                        let more = if remaining > 0 { 1 } else { 0 };
                        let warns = if read_warning_text(b).is_some() { 2 } else { 0 };
                        1 + rows.len() as u16 + more + warns
                    } else {
                        0
                    };
                    BOX_PAD_Y + 1 + content + BOX_PAD_Y
                }
                ToolRenderer::Grep => {
                    // grep renderer: title + (blank + up to 15 wrapped raw
                    // lines + hint + warnings when output non-empty).
                    let mut content = 0u16;
                    if !b.tool_output.trim().is_empty() {
                        let limit = if expanded { usize::MAX } else { 15 };
                        let (rows, remaining) = preview_wrapped(b.tool_output.trim(), wrap_w, limit);
                        let more = if remaining > 0 { 1 } else { 0 };
                        let warns = if grep_warning_text(b).is_some() { 2 } else { 0 };
                        content = 1 + rows.len() as u16 + more + warns;
                    }
                    BOX_PAD_Y + 1 + content + BOX_PAD_Y
                }
                ToolRenderer::Edit => {
                    // edit renderer: title + blank + diff text.
                    let total = b.tool_output.lines().count();
                    BOX_PAD_Y + 1 + 1 + total as u16 + BOX_PAD_Y
                }
                ToolRenderer::Fallback => {
                    // fallback renderer: title + (blank + wrapped args) +
                    // (blank + wrapped output preview + hint).
                    let mut content = 0u16;
                    if !b.tool_args.is_empty() {
                        let args_rows: usize = b
                            .tool_args
                            .lines()
                            .map(|l| visual_lines(l, wrap_w).len())
                            .sum();
                        content += 1 + args_rows as u16;
                    }
                    if !b.tool_output.is_empty() {
                        let limit = if expanded { usize::MAX } else { FALLBACK_PREVIEW_LINES };
                        let (rows, remaining) = preview_wrapped(&b.tool_output, wrap_w, limit);
                        let more = if remaining > 0 { 1 } else { 0 };
                        // TS `formatToolExecution` separates args and output
                        // with a blank line.
                        let blank = if b.tool_args.is_empty() { 0 } else { 1 };
                        content += blank + rows.len() as u16 + more;
                    }
                    BOX_PAD_Y + 1 + content + BOX_PAD_Y
                }
            }
        }
        "user" => BOX_PAD_Y + b.md_lines.len() as u16 + BOX_PAD_Y,
        "system" => b.plain_lines.len() as u16,
        _ => {
            // Assistant: thinking section FIRST (blank + thinking lines +
            // trailing blank — TS renders content in order, thinking before
            // text), then text lines, then stop-reason notice (blank +
            // notice line).
            let mut h = 0u16;
            if !b.thinking_lines.is_empty() {
                h += 1 + b.thinking_lines.len() as u16 + 1;
            }
            h += b.md_lines.len() as u16;
            if b.stop_reason.is_some() {
                h += 2; // blank separator + notice line
            }
            h
        }
    }
}

/// One full-width background row (empty content) — box top/bottom padding.
fn render_bg_row(frame: &mut Frame, area: Rect, y: i32, bg: Color) {
    if y >= area.top() as i32 && y < area.bottom() as i32 {
        frame.render_widget(
            Paragraph::new(Line::raw("")).style(Style::new().bg(bg)),
            Rect::new(area.x, y as u16, area.width, 1),
        );
    }
}

/// One content row inside a background box: `BOX_PAD_X` left padding, the
/// given spans, background fills the rest of the row (TS
/// `applyBackgroundToLine`).
fn render_boxed_row(frame: &mut Frame, area: Rect, y: i32, bg: Color, mut spans: Vec<Span<'static>>) {
    if y >= area.top() as i32 && y < area.bottom() as i32 {
        let mut line = Line::raw(" ".repeat(BOX_PAD_X as usize));
        line.spans.append(&mut spans);
        frame.render_widget(
            Paragraph::new(line).style(Style::new().bg(bg)),
            Rect::new(area.x, y as u16, area.width, 1),
        );
    }
}

/// Startup header block (TS `builtInHeader`): logo + keybinding hints +
/// onboarding, rendered at the top of the transcript.
fn render_header_block(frame: &mut Frame, area: Rect, item: &BlockView, mut ly: i32) -> i32 {
    for line in &item.md_lines {
        render_body_row(frame, area, ly, Paragraph::new(line.clone()));
        ly += 1;
    }
    ly
}
/// Tool box. Two paths, mirroring the TS original:
///
/// - **bash** (`core/tools/bash.ts` renderer): `$ {command}` bold title
///   (+ muted ` (timeout Ns)`), a blank line, then the output — when not
///   expanded the *tail* 5 visual lines with a leading
///   `... (N earlier lines, ctrl+o to expand)` hint, then a muted
///   `Elapsed {x.x}s` (running, re-rendered every tick) / `Took {x.x}s`
///   (finished) line.
/// - **fallback** (no registered renderer): bold tool-name title, blank
///   line + args JSON, the first 10 output lines with a trailing
///   `... (N more lines, ctrl+o to expand)` hint, no timer.
///
/// Output is plain `toolOutput` color in both paths (the TS renderers do
/// not diff-colorize). Returns the next row.
fn render_tool_block(frame: &mut Frame, area: Rect, item: &BlockView, expanded: bool, t: &Theme, mut ly: i32, wrap_w: usize) -> i32 {
    let bg = tool_bg(t, item.tool_state.as_ref());
    // Box top padding.
    render_bg_row(frame, area, ly, bg);
    ly += 1;
    match tool_renderer(item) {
        ToolRenderer::Bash => ly = render_bash_tool_block(frame, area, item, expanded, t, ly, wrap_w, bg),
        ToolRenderer::Read => ly = render_read_tool_block(frame, area, item, expanded, t, ly, wrap_w, bg),
        ToolRenderer::Grep => ly = render_grep_tool_block(frame, area, item, expanded, t, ly, wrap_w, bg),
        ToolRenderer::Edit => ly = render_edit_tool_block(frame, area, item, expanded, t, ly, bg),
        ToolRenderer::Fallback => ly = render_fallback_tool_block(frame, area, item, expanded, t, ly, wrap_w, bg),
    }
    // Bottom padding.
    render_bg_row(frame, area, ly, bg);
    ly + 1
}

/// Bash renderer rows (title through the timer line).
fn render_bash_tool_block(frame: &mut Frame, area: Rect, item: &BlockView, expanded: bool, t: &Theme, mut ly: i32, wrap_w: usize, bg: Color) -> i32 {
    // Title: `$ {command}` bold (+ `...` in toolOutput when empty/missing,
    // `[invalid arg]` in error when non-string), muted ` (timeout Ns)`
    // suffix (TS `formatBashCall`).
    let (command, timeout) = bash_call_args(&item.tool_args);
    let title_bold = Style::new().fg(t.tool_title).add_modifier(Modifier::BOLD);
    let mut spans = match &command {
        BashCommandArg::Invalid => vec![
            Span::styled("$ ", title_bold),
            Span::styled(
                "[invalid arg]",
                Style::new().fg(t.error).add_modifier(Modifier::BOLD),
            ),
        ],
        BashCommandArg::Text(c) if c.is_empty() => vec![
            Span::styled("$ ", title_bold),
            Span::styled(
                "...",
                Style::new().fg(t.tool_output).add_modifier(Modifier::BOLD),
            ),
        ],
        BashCommandArg::Text(c) => vec![Span::styled(format!("$ {c}"), title_bold)],
    };
    if let Some(secs) = timeout {
        spans.push(Span::styled(format!(" (timeout {secs}s)"), Style::new().fg(t.muted)));
    }
    render_boxed_row(frame, area, ly, bg, spans);
    ly += 1;
    // Blank line, then the output — only when the trimmed output is
    // non-empty (TS renderResult prepends a newline inside the output Text).
    let output = bash_display_text(item);
    if !output.is_empty() {
        render_boxed_row(frame, area, ly, bg, vec![]);
        ly += 1;
        let (shown, skipped) = bash_preview(item, wrap_w, expanded);
        if skipped > 0 {
            // Leading hint (TS: `["", hint, ...tail]`).
            render_boxed_row(
                frame,
                area,
                ly,
                bg,
                vec![
                    Span::styled(format!("... ({skipped} earlier lines, "), Style::new().fg(t.muted)),
                    Span::styled("ctrl+o", Style::new().fg(t.dim)),
                    Span::styled(" to expand)", Style::new().fg(t.muted)),
                ],
            );
            ly += 1;
        }
        let visual = visual_lines(&output, wrap_w);
        let start = visual.len() - shown;
        for line in visual.into_iter().skip(start) {
            render_boxed_row(frame, area, ly, bg, vec![Span::styled(line, Style::new().fg(t.tool_output))]);
            ly += 1;
        }
    }
    // Truncation warnings (TS: `\n` + warning text, before the timer).
    if let Some(warn) = bash_warning_text(item) {
        render_boxed_row(frame, area, ly, bg, vec![]);
        ly += 1;
        render_boxed_row(
            frame,
            area,
            ly,
            bg,
            vec![Span::styled(warn, Style::new().fg(t.warning))],
        );
        ly += 1;
    }
    // Timer: `Elapsed {x.x}s` while running (tick-driven re-render), else
    // `Took {x.x}s` (TS `formatDuration` = `(ms/1000).toFixed(1)` + "s").
    let finished = matches!(item.tool_state, Some(ToolCallState::Done | ToolCallState::Failed));
    let ms: u128 = if finished {
        item.tool_ended_at
            .zip(item.tool_started_at)
            .map(|(e, s)| e.duration_since(s).as_millis())
            .unwrap_or(0)
    } else {
        item.tool_started_at.map(|s| s.elapsed().as_millis()).unwrap_or(0)
    };
    render_boxed_row(frame, area, ly, bg, vec![]);
    ly += 1;
    render_boxed_row(
        frame,
        area,
        ly,
        bg,
        vec![Span::styled(
            format!("{} {}", if finished { "Took" } else { "Elapsed" }, format_duration(ms)),
            Style::new().fg(t.muted),
        )],
    );
    ly + 1
}

/// Shorten a path for display: `$HOME` → `~` (TS `shortenPath`).
fn shorten_path(path: &str, cwd: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && path.starts_with(&home) {
        return format!("~{}", &path[home.len()..]);
    }
    if path == cwd {
        return ".".to_string();
    }
    path.to_string()
}

/// Fallback renderer rows (no registered tool renderer — TS
/// `formatToolExecution`): bold tool-name title, blank + args JSON, first
/// 10 output lines + trailing `... (N more lines, ctrl+o to expand)`.
fn render_fallback_tool_block(frame: &mut Frame, area: Rect, item: &BlockView, expanded: bool, t: &Theme, mut ly: i32, wrap_w: usize, bg: Color) -> i32 {
    // Title row.
    render_boxed_row(
        frame,
        area,
        ly,
        bg,
        vec![Span::styled(item.tool_name.clone(), Style::new().fg(t.tool_title).add_modifier(Modifier::BOLD))],
    );
    ly += 1;
    // Args (TS fallback: blank line + JSON in the default text color,
    // word-wrapped by the Text renderer).
    if !item.tool_args.is_empty() {
        render_boxed_row(frame, area, ly, bg, vec![]);
        ly += 1;
        for line in item.tool_args.lines() {
            for row in visual_lines(line, wrap_w) {
                render_boxed_row(frame, area, ly, bg, vec![Span::raw(row)]);
                ly += 1;
            }
        }
    }
    // Output rows (TS `formatToolExecution` separates args and output with
    // a blank line; the output previews the first 10 raw lines, each
    // word-wrapped like the Text renderer).
    if !item.tool_output.is_empty() {
        if !item.tool_args.is_empty() {
            render_boxed_row(frame, area, ly, bg, vec![]);
            ly += 1;
        }
        let limit = if expanded { usize::MAX } else { FALLBACK_PREVIEW_LINES };
        let (rows, remaining) = preview_wrapped(&item.tool_output, wrap_w, limit);
        for row in rows {
            render_boxed_row(frame, area, ly, bg, vec![Span::styled(row, Style::new().fg(t.tool_output))]);
            ly += 1;
        }
        if remaining > 0 {
            render_boxed_row(
                frame,
                area,
                ly,
                bg,
                vec![
                    Span::styled(format!("... ({remaining} more lines, "), Style::new().fg(t.muted)),
                    Span::styled("ctrl+o", Style::new().fg(t.dim)),
                    Span::styled(" to expand)", Style::new().fg(t.muted)),
                ],
            );
            ly += 1;
        }
    }
    ly
}

/// Extract `file_path ?? path`, `offset`, `limit` from the read args JSON.
fn read_call_args(args: &str) -> (Option<String>, Option<u64>, Option<u64>) {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
    let path = v
        .get("file_path")
        .and_then(|p| p.as_str())
        .or_else(|| v.get("path").and_then(|p| p.as_str()))
        .map(str::to_string);
    let offset = v.get("offset").and_then(|o| o.as_u64());
    let limit = v.get("limit").and_then(|l| l.as_u64());
    (path, offset, limit)
}

/// Read renderer (TS `formatReadCall`/`formatReadResult`): the title is
/// `read {path}{:range}` — `read` bold, path accent, range warning — and
/// the content only renders when expanded or the tool errored (collapsed
/// success shows just the title, matching TS `formatReadResult` returning
/// "" unless expanded). Content lines are word-wrapped like the TS Text
/// renderer, and truncation warnings render below the preview.
fn render_read_tool_block(frame: &mut Frame, area: Rect, item: &BlockView, expanded: bool, t: &Theme, mut ly: i32, wrap_w: usize, bg: Color) -> i32 {
    let (path, offset, limit) = read_call_args(&item.tool_args);
    let path_display = match path {
        Some(p) if !p.is_empty() => shorten_path(&p, &item.cwd),
        _ => "...".to_string(),
    };
    let mut spans = vec![
        Span::styled("read", Style::new().fg(t.tool_title).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {path_display}"), Style::new().fg(t.accent)),
    ];
    if offset.is_some() || limit.is_some() {
        let start_line = offset.unwrap_or(1);
        let end_line = limit.map(|l| start_line + l - 1);
        let range = match end_line {
            Some(e) => format!(":{start_line}-{e}"),
            None => format!(":{start_line}"),
        };
        spans.push(Span::styled(range, Style::new().fg(t.warning)));
    }
    render_boxed_row(frame, area, ly, bg, spans);
    ly += 1;
    if read_content_visible(item, expanded) {
        render_boxed_row(frame, area, ly, bg, vec![]);
        ly += 1;
        let (rows, remaining) = preview_wrapped(item.tool_output.trim_end(), wrap_w, 10);
        for row in rows {
            render_boxed_row(frame, area, ly, bg, vec![Span::styled(row, Style::new().fg(t.tool_output))]);
            ly += 1;
        }
        if remaining > 0 {
            render_boxed_row(
                frame,
                area,
                ly,
                bg,
                vec![
                    Span::styled(format!("... ({remaining} more lines, "), Style::new().fg(t.muted)),
                    Span::styled("ctrl+o", Style::new().fg(t.dim)),
                    Span::styled(" to expand)", Style::new().fg(t.muted)),
                ],
            );
            ly += 1;
        }
        // read truncation warnings (TS `formatReadResult`).
        if let Some(warn) = read_warning_text(item) {
            render_boxed_row(frame, area, ly, bg, vec![]);
            ly += 1;
            render_boxed_row(
                frame,
                area,
                ly,
                bg,
                vec![Span::styled(warn, Style::new().fg(t.warning))],
            );
            ly += 1;
        }
    }
    ly
}

/// Grep renderer (TS `formatGrepCall`/`formatGrepResult`): title
/// `grep /{pattern}/ in {path} ({glob}) limit {n}`, the output previewed
/// to 15 wrapped raw lines, and the truncation warning
/// `[Truncated: ...]` when the match/byte/line limits were hit.
fn render_grep_tool_block(frame: &mut Frame, area: Rect, item: &BlockView, expanded: bool, t: &Theme, mut ly: i32, wrap_w: usize, bg: Color) -> i32 {
    let v: serde_json::Value = serde_json::from_str(&item.tool_args).unwrap_or(serde_json::Value::Null);
    let pattern = v.get("pattern").and_then(|p| p.as_str()).map(str::to_string);
    let path = v
        .get("path")
        .and_then(|p| p.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| ".".to_string());
    let glob = v.get("glob").and_then(|g| g.as_str()).map(str::to_string);
    let limit = v.get("limit").and_then(|l| l.as_u64());

    let mut spans = vec![
        Span::styled("grep", Style::new().fg(t.tool_title).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(" /{}/", pattern.unwrap_or_default()),
            Style::new().fg(t.accent),
        ),
        Span::styled(format!(" in {}", shorten_path(&path, &item.cwd)), Style::new().fg(t.tool_output)),
    ];
    if let Some(g) = glob {
        spans.push(Span::styled(format!(" ({g})"), Style::new().fg(t.tool_output)));
    }
    if let Some(n) = limit {
        spans.push(Span::styled(format!(" limit {n}"), Style::new().fg(t.tool_output)));
    }
    render_boxed_row(frame, area, ly, bg, spans);
    ly += 1;
    let output = item.tool_output.trim();
    if !output.is_empty() {
        render_boxed_row(frame, area, ly, bg, vec![]);
        ly += 1;
        let limit = if expanded { usize::MAX } else { 15 };
        let (rows, remaining) = preview_wrapped(output, wrap_w, limit);
        for row in rows {
            render_boxed_row(frame, area, ly, bg, vec![Span::styled(row, Style::new().fg(t.tool_output))]);
            ly += 1;
        }
        if remaining > 0 {
            render_boxed_row(
                frame,
                area,
                ly,
                bg,
                vec![
                    Span::styled(format!("... ({remaining} more lines, "), Style::new().fg(t.muted)),
                    Span::styled("ctrl+o", Style::new().fg(t.dim)),
                    Span::styled(" to expand)", Style::new().fg(t.muted)),
                ],
            );
            ly += 1;
        }
    }
    // grep truncation warnings (TS `formatGrepResult`).
    if let Some(warn) = grep_warning_text(item) {
        render_boxed_row(frame, area, ly, bg, vec![]);
        ly += 1;
        render_boxed_row(
            frame,
            area,
            ly,
            bg,
            vec![Span::styled(warn, Style::new().fg(t.warning))],
        );
        ly += 1;
    }
    ly
}

/// Edit renderer (TS `formatEditCall`): title `edit {path}`; the diff
/// text renders with diff syntax highlighting (TS `renderDiff`:
/// `-`/`+`/context lines in toolDiffRemoved/toolDiffAdded/toolDiffContext,
/// single-line modifications get intra-line word highlighting).
fn render_edit_tool_block(frame: &mut Frame, area: Rect, item: &BlockView, _expanded: bool, t: &Theme, mut ly: i32, bg: Color) -> i32 {
    let v: serde_json::Value = serde_json::from_str(&item.tool_args).unwrap_or(serde_json::Value::Null);
    let path = v
        .get("file_path")
        .and_then(|p| p.as_str())
        .or_else(|| v.get("path").and_then(|p| p.as_str()))
        .map(str::to_string)
        .unwrap_or_else(|| "...".to_string());
    render_boxed_row(
        frame,
        area,
        ly,
        bg,
        vec![
            Span::styled("edit", Style::new().fg(t.tool_title).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {}", shorten_path(&path, &item.cwd)), Style::new().fg(t.accent)),
        ],
    );
    ly += 1;
    if !item.tool_output.is_empty() {
        render_boxed_row(frame, area, ly, bg, vec![]);
        ly += 1;
        let lines: Vec<&str> = item.tool_output.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let consumed = render_edit_diff_line(frame, area, ly, bg, &lines, i, t);
            ly += consumed as i32;
            i += consumed;
        }
    }
    ly
}

fn tool_bg(t: &Theme, state: Option<&ToolCallState>) -> Color {
    match state {
        Some(ToolCallState::Failed) => t.tool_error_bg,
        Some(ToolCallState::Done) => t.tool_success_bg,
        _ => t.tool_pending_bg,
    }
}

/// User message: full-width box (`userMessageBg`, TS
/// `UserMessageComponent`) with markdown content — never truncated (the
/// transcript scrolls instead).
fn render_user_block(frame: &mut Frame, area: Rect, item: &BlockView, t: &Theme, mut ly: i32) -> i32 {
    let bg = t.user_message_bg;
    render_bg_row(frame, area, ly, bg);
    ly += 1;
    for line in &item.md_lines {
        let mut l = Line::raw(" ".repeat(BOX_PAD_X as usize));
        l.spans.extend(line.spans.clone());
        render_body_row(frame, area, ly, Paragraph::new(l).style(Style::new().bg(bg)));
        ly += 1;
    }
    render_bg_row(frame, area, ly, bg);
    ly + 1
}

/// Assistant message: plain markdown rows with the TS `outputPad` (1-cell
/// horizontal margin), no background — never truncated (the transcript
/// scrolls). Thinking content renders below the text in `thinkingText`
/// italic (TS `AssistantMessageComponent`), and a terminal stop reason
/// renders a TS-style notice in the error color.
fn render_assistant_block(frame: &mut Frame, area: Rect, item: &BlockView, t: &Theme, mut ly: i32) -> i32 {
    // Thinking section FIRST (TS `AssistantMessageComponent.updateContent`
    // renders `message.content` in order — the model emits thinking blocks
    // before the text, so thinking renders above the body). Blank separator
    // + thinking lines in thinkingText italic (TS `Markdown(thinking, pad,
    // 0, { color: thinkingText, italic: true })`).
    if !item.thinking_lines.is_empty() {
        render_body_row(frame, area, ly, Paragraph::new(Line::raw("")));
        ly += 1;
        for line in &item.thinking_lines {
            let mut l = Line::raw("");
            l.spans.extend(line.spans.iter().map(|s| {
                Span::styled(
                    s.content.clone(),
                    s.style
                        .fg(t.thinking_text)
                        .add_modifier(Modifier::ITALIC),
                )
            }));
            render_body_row(frame, area, ly, Paragraph::new(l));
            ly += 1;
        }
        // TS: Spacer(1) after the thinking run when body text follows.
        render_body_row(frame, area, ly, Paragraph::new(Line::raw("")));
        ly += 1;
    }
    for line in &item.md_lines {
        render_body_row(frame, area, ly, Paragraph::new(line.clone()));
        ly += 1;
    }
    // Stop-reason notice (TS: Spacer + Text in error color).
    if let Some(reason) = item.stop_reason {
        let msg = match reason {
            StopReason::Length => "Response was truncated before completion.".to_string(),
            StopReason::Aborted => item
                .error_message
                .clone()
                .filter(|m| m != "Request was aborted")
                .unwrap_or_else(|| "Operation aborted".to_string()),
            StopReason::Error => format!(
                "Error: {}",
                item.error_message.clone().unwrap_or_else(|| "Unknown error".to_string())
            ),
        };
        render_body_row(frame, area, ly, Paragraph::new(Line::raw("")));
        ly += 1;
        render_body_row(frame, area, ly, Paragraph::new(Line::from(Span::styled(msg, Style::new().fg(t.error)))));
        ly += 1;
    }
    ly
}

/// System notice: plain text rows in muted (TS status notices are plain
/// `Text` in dim/muted) — never truncated.
fn render_system_block(frame: &mut Frame, area: Rect, item: &BlockView, t: &Theme, mut ly: i32) -> i32 {
    for line in &item.plain_lines {
        render_body_row(frame, area, ly, Paragraph::new(Line::from(Span::styled(line.clone(), Style::new().fg(t.muted)))));
        ly += 1;
    }
    ly
}

// ============================================================================
// Editor (chat input) — TS bordered editor
// ============================================================================

/// Wrap the input into display rows at the editor's layout width and return
/// `(rows, cursor_row, cursor_col)`. Wrapping is per display column
/// (CJK-aware), never splitting a glyph; the cursor lands in the row that
/// contains its byte offset.
fn input_layout_rows(input: &Input, layout_width: usize) -> (Vec<String>, usize, usize) {
    let layout_width = layout_width.max(1);
    let text = input.value();
    let cursor = input.cursor_pos();
    let mut rows: Vec<String> = Vec::new();
    let mut cursor_row = 0usize;
    let mut cursor_col = 0usize;
    let mut line_start = 0usize;
    let mut row_idx = 0usize;
    for line in text.split('\n') {
        let line_end = line_start + line.len();
        let mut within = 0usize;
        for (row_text, bytes) in wrap_line(line, layout_width) {
            let row_start = line_start + within;
            let row_end = row_start + bytes;
            if cursor >= row_start && cursor <= row_end {
                cursor_row = row_idx;
                cursor_col = unicode_width::UnicodeWidthStr::width(&line[within..(cursor - line_start)]);
            }
            rows.push(row_text);
            within += bytes;
            row_idx += 1;
        }
        line_start = line_end + 1;
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    (rows, cursor_row, cursor_col)
}

/// Greedy per-column wrap of one logical line; returns `(row_text, bytes)`
/// pairs. Wide glyphs that don't fit start their own (necessarily
/// over-wide) row rather than being split.
fn wrap_line(line: &str, width: usize) -> Vec<(String, usize)> {
    if line.is_empty() {
        return vec![(String::new(), 0)];
    }
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut col = 0usize;
    for (idx, ch) in line.char_indices() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w > width && col > 0 {
            out.push((line[start..idx].to_string(), idx - start));
            start = idx;
            col = 0;
        }
        col += w;
    }
    out.push((line[start..].to_string(), line.len() - start));
    out
}

fn render_input(model: &mut Model, frame: &mut Frame, area: Rect, t: &Theme) {
    // TS editor: `─` top border, content rows, `─` bottom border
    // (borderMuted); content scrolls to keep the cursor visible. The
    // completion menu renders inline between the content and the bottom
    // border (TS SelectList inside the editor).
    let layout_width = editor_layout_width(area.width);
    let (rows, cursor_row, cursor_col) = input_layout_rows(&model.input, layout_width);
    let completion = completer_rows(&model.completer);
    let content_rows = (area.height.saturating_sub(2).saturating_sub(completion)).max(1) as usize;
    let max_scroll = rows.len().saturating_sub(content_rows);
    let scroll = cursor_row.saturating_sub(content_rows.saturating_sub(1)).min(max_scroll);

    let border_style = Style::new().fg(t.border_muted);
    frame.render_widget(Paragraph::new(Line::from(Span::styled("─".repeat(area.width as usize), border_style))), Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(Paragraph::new(Line::from(Span::styled("─".repeat(area.width as usize), border_style))), Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1));

    let mut content_used = 0usize;
    for (i, row) in rows.iter().enumerate().skip(scroll).take(content_rows) {
        let y = area.y + 1 + (i - scroll) as u16;
        if y >= area.y + area.height.saturating_sub(1) { break; }
        frame.render_widget(Paragraph::new(Line::raw(row.clone())), Rect::new(area.x, y, area.width, 1));
        content_used += 1;
    }

    if model.completer.visible && completion > 0 {
        let comp_area = Rect::new(area.x, area.y + 1 + content_used as u16, area.width, completion);
        model.completer.render_rows(frame, comp_area, t);
    }

    let cursor_visible = cursor_row - scroll;
    let cx = (area.x + cursor_col as u16).min(area.x + area.width.saturating_sub(1));
    let cy = area.y + 1 + cursor_visible as u16;
    frame.set_cursor_position((cx, cy));
}

// ============================================================================
// Footer — TS two-line footer
// ============================================================================

/// Truncate a string to a display width, appending a suffix when cut (TS
/// `truncateToWidth(text, width, suffix)`).
pub(crate) fn truncate_to_width_suffix(s: &str, width: usize, suffix: &str) -> String {
    let w = unicode_width::UnicodeWidthStr::width(s);
    if w <= width {
        return s.to_string();
    }
    let sw = unicode_width::UnicodeWidthStr::width(suffix);
    let mut out = String::new();
    let mut col = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + cw + sw > width { break; }
        out.push(ch);
        col += cw;
    }
    format!("{out}{suffix}")
}

/// Truncate a string to a display width, appending "..." when cut.
pub(crate) fn truncate_to_width(s: &str, width: usize) -> String {
    truncate_to_width_suffix(s, width, "...")
}

/// Format token counts for the compact footer display (TS
/// `formatTokens()`).
fn footer_tokens(count: u64) -> String {
    format_tokens(count)
}

/// TS `FooterComponent`: line 1 is the working directory (with the git
/// branch and session name) in dim; line 2 is the stats (`↑in ↓out Rcache
/// Wcache CH% $cost ctx%/window (auto)`, context colorized by threshold)
/// with the model name right-aligned (provider prefix when known, thinking
/// level when the model reasons).
fn render_footer(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let width = area.width as usize;
    if area.height > 0 {
        // Line 1: `~/path (branch) • sessionName` in dim.
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let cwd: String = if model.cwd.is_empty() {
            "~".into()
        } else {
            model.cwd.replace(&home, "~")
        };
        let mut pwd = cwd;
        if let Some(branch) = &model.git_branch {
            pwd = format!("{pwd} ({branch})");
        }
        if let Some(name) = &model.session_name {
            pwd = format!("{pwd} \u{2022} {name}");
        }
        let pwd = truncate_to_width(&pwd, width);
        frame.render_widget(Paragraph::new(Line::from(Span::styled(pwd, Style::new().fg(t.dim)))), Rect::new(area.x, area.y, area.width, 1));
    }
    if area.height > 1 {
    // Line 2: stats left (dim, context % colorized by threshold) +
    // right-aligned model label (dim).
    let u = &model.usage_totals;
    let mut left_spans: Vec<Span<'static>> = Vec::new();
    let dim = |s: String| Span::styled(s, Style::new().fg(t.dim));
        // TS joins the stats parts with a single space.
        let push_part = |spans: &mut Vec<Span<'static>>, s: String| {
            if !spans.is_empty() {
                spans.push(Span::raw(" "));
            }
            spans.push(dim(s));
        };
        if u.input > 0 {
            push_part(&mut left_spans, format!("\u{2191}{}", footer_tokens(u.input)));
        }
        if u.output > 0 {
            push_part(&mut left_spans, format!("\u{2193}{}", footer_tokens(u.output)));
        }
        if u.cache_read > 0 {
            push_part(&mut left_spans, format!("R{}", footer_tokens(u.cache_read)));
        }
        if u.cache_write > 0 {
            push_part(&mut left_spans, format!("W{}", footer_tokens(u.cache_write)));
        }
        if u.cache_read > 0 || u.cache_write > 0 {
            if let Some(rate) = u.cache_hit_rate {
                push_part(&mut left_spans, format!("CH{rate:.1}%"));
            }
        }
        if u.cost > 0.0 {
            push_part(&mut left_spans, format!("${:.3}", u.cost));
        }
        // Context display: `{pct}%/{window} (auto)` — colorized by
        // threshold (error >90, warning >70), `?/{window}` when unknown.
        let pct = model.context_usage_pct.min(100.0);
        let ctx_col = if pct > CTX_ERROR_PCT as f64 {
            t.error
        } else if pct > CTX_WARNING_PCT as f64 {
            t.warning
        } else {
            t.dim
        };
        let window = footer_tokens(model.context_window);
        let auto = if model.auto_compact { " (auto)" } else { "" };
        let ctx_display = if model.context_usage_known {
            format!("{pct:.1}%/{window}{auto}")
        } else {
            format!("?/{window}{auto}")
        };
        if !left_spans.is_empty() {
            left_spans.push(Span::raw(" "));
        }
        left_spans.push(Span::styled(ctx_display, Style::new().fg(ctx_col)));

        // Right side: `(provider) model • thinking` when the model reasons.
        let model_name = if model.model_name.is_empty() {
            "no-model".to_string()
        } else {
            model.model_name.clone()
        };
        let mut right_side = model_name.clone();
        if model.reasoning {
            let level = model.thinking_level.clone().unwrap_or_else(|| "off".to_string());
            right_side = if level == "off" {
                format!("{model_name} \u{2022} thinking off")
            } else {
                format!("{model_name} \u{2022} {level}")
            };
        }
        if let Some(provider) = &model.provider {
            let with_provider = format!("({provider}) {right_side}");
            let left_w: usize = left_spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
            if left_w + 2 + unicode_width::UnicodeWidthStr::width(with_provider.as_str()) <= width {
                right_side = with_provider;
            }
        }

        let left_w: usize = left_spans.iter().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref())).sum();
        let right_w = unicode_width::UnicodeWidthStr::width(right_side.as_str());
        let min_pad = 2usize;
        let (right, right_w) = if left_w + min_pad + right_w <= width {
            (right_side, right_w)
        } else if width > left_w + min_pad {
            let truncated = truncate_to_width_suffix(&right_side, width - left_w - min_pad, "");
            let tw = unicode_width::UnicodeWidthStr::width(truncated.as_str());
            (truncated, tw)
        } else {
            (String::new(), 0)
        };
        let padding = " ".repeat(width.saturating_sub(left_w + right_w));
        left_spans.push(Span::raw(padding));
        left_spans.push(Span::styled(right, Style::new().fg(t.dim)));
        let line = Line::from(left_spans);
        frame.render_widget(Paragraph::new(line), Rect::new(area.x, area.y + 1, area.width, 1));
    }
}

// ============================================================================
// Dialog / Select
// ============================================================================

fn render_dialog(model: &Model, frame: &mut Frame, area: Rect, t: &Theme) {
    let d = match &model.dialog { Some(d) => d, None => return };
    frame.render_widget(Clear, area);
    let dw = (area.width / 3 * 2).max(40).min(area.width.saturating_sub(4));
    let dh = 7u16 + d.message.lines().count() as u16;
    // Center inside the visible screen (alt screen = the whole area).
    let dh = dh.min(area.height.max(3));
    let da = Rect::new((area.width - dw) / 2, (area.height.saturating_sub(dh)) / 2, dw, dh);
    frame.render_widget(Clear, da);
    // Bordered modal window (TS modals draw a border around the overlay).
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.border))
            .title(format!(" {} ", d.title))
            .title_style(Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        da,
    );
    let inner = da.inner(ratatui::layout::Margin::new(1, 1));
    let mut y = inner.y;
    for line in d.message.lines() {
        if y < inner.y + inner.height { frame.render_widget(Paragraph::new(Line::from(Span::raw(line.to_string()))), Rect::new(inner.x, y, inner.width, 1)); y += 1; }
    }
    let ba = Rect::new(inner.x, da.y + dh - 2, inner.width, 1);
    let tw: usize = d.buttons.iter().map(|b| b.label.len() + 2 + if matches!(b.action, DialogAction::ConfirmAlways) { 2 } else { 0 }).sum();
    let sp = (inner.width as usize).saturating_sub(tw) / (d.buttons.len() + 1).max(1);
    let mut spans = vec![Span::raw(" ".repeat(sp))];
    for (i, btn) in d.buttons.iter().enumerate() {
        let s = if i == d.selected {
            Style::new().fg(t.text).bg(t.selected_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.muted)
        };
        let lbl = if matches!(btn.action, DialogAction::ConfirmAlways) { format!(" {} [A] ", btn.label) } else { format!(" {} ", btn.label) };
        spans.push(Span::styled(lbl, s)); spans.push(Span::raw(" ".repeat(sp)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), ba);
}

fn render_fullscreen_editor(model: &mut Model, frame: &mut Frame, area: Rect, title: &str, t: &Theme) {
    let AppMode::Editor { editor, .. } = &mut model.mode else { return };
    // Render via the vendored textarea widget (wrapped-line viewport,
    // selection, scrollbar) inside the title block. Block first so the
    // textarea never paints over the borders.
    frame.render_widget(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(format!(" {title} ")).border_style(Style::new().fg(t.border_muted)), area);
    let inner = area.inner(ratatui::layout::Margin::new(1, 1));
    frame.render_widget_ref(editor.textarea(), inner);
    if let Some((x, y)) = editor.textarea().cursor_pos(inner) {
        frame.set_cursor_position((inner.x + x, inner.y + y));
    }
}

// ============================================================================
// Main loop
// ============================================================================

/// Simple alt-screen event loop (ratatui draws into the alternate screen
/// buffer): render the model, then process keys until quit. `terminal` must
/// already be started (`Terminal::start`).
pub async fn run(
    mut model: Model,
    mut terminal: crate::terminal::Terminal,
    mut input_rx: tokio::sync::mpsc::UnboundedReceiver<KeyEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::time::{sleep, Duration};
    loop {
        terminal.ratatui_terminal().draw(|frame| view(&mut model, frame))?;
        tokio::select! {
            Some(key) = input_rx.recv() => {
                let cmds = update(&mut model, Msg::Key(key));
                for cmd in cmds {
                    if matches!(cmd, Cmd::Quit) {
                        return Ok(());
                    }
                }
            }
            _ = sleep(Duration::from_millis(50)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;
    use crate::theme;

    // ============================================================
    // Edit tool diff parsing (TS `parseDiffLine` / `renderDiff`)
    // ============================================================

    #[test]
    fn parse_diff_line_handles_padded_line_numbers() {
        // `+123 content` / `-123 content` / ` 123 content` (generateDiffString
        // pads line numbers to equal width).
        assert_eq!(parse_diff_line("+123 abc"), Some(('+', "123".into(), "abc")));
        assert_eq!(parse_diff_line("-123 abc"), Some(('-', "123".into(), "abc")));
        assert_eq!(parse_diff_line(" 123 abc"), Some((' ', "123".into(), "abc")));
        // Wide padding (3-digit padStart) is kept verbatim.
        assert_eq!(parse_diff_line("-  1 abc"), Some(('-', "  1".into(), "abc")));
        // Line number may be missing (`- content`).
        assert_eq!(parse_diff_line("- abc"), Some(('-', String::new(), "abc")));
        // Content may be empty (` ` context line).
        assert_eq!(parse_diff_line(" 123 "), Some((' ', "123".into(), "")));
    }

    #[test]
    fn parse_diff_line_rejects_non_diff_lines() {
        // `+foo` has no separator whitespace after the number run.
        assert_eq!(parse_diff_line("+foo"), None);
        assert_eq!(parse_diff_line("plain text"), None);
        assert_eq!(parse_diff_line("--- a/foo"), None);
        assert_eq!(parse_diff_line("@@ -1,2 +1,2 @@"), None);
        assert_eq!(parse_diff_line(""), None);
    }

    #[test]
    fn replace_tabs_renders_three_spaces() {
        assert_eq!(replace_tabs("a\tb"), "a   b");
        assert_eq!(replace_tabs("no tabs"), "no tabs");
    }

    #[test]
    fn intra_line_spans_highlight_changed_words() {
        // "hello world" → "hello rust": only "rust" is inverted; "hello "
        // is plain (equal part).
        let (removed, added) = intra_line_spans(
            "hello world",
            "hello rust",
            Style::new().fg(theme::Theme::default().diff_removed),
            Style::new().fg(theme::Theme::default().diff_added),
        );
        let text = |s: &[Span]| s.iter().map(|sp| sp.content.as_ref()).collect::<String>();
        assert_eq!(text(&removed), "hello world");
        assert_eq!(text(&added), "hello rust");
        // The changed token carries the REVERSED modifier; the equal prefix does not.
        let inverted = |s: &[Span]| {
            s.iter()
                .filter(|sp| sp.style.add_modifier.contains(Modifier::REVERSED))
                .map(|sp| sp.content.as_ref())
                .collect::<String>()
        };
        assert_eq!(inverted(&removed), "world", "changed removed token inverted");
        assert_eq!(inverted(&added), "rust", "changed added token inverted");
        // Leading whitespace of the first changed token stays plain (TS strips it).
        let (removed, _) = intra_line_spans(
            "  foo",
            "  bar",
            Style::new().fg(theme::Theme::default().diff_removed),
            Style::new().fg(theme::Theme::default().diff_added),
        );
        assert_eq!(text(&removed), "  foo");
        assert_eq!(inverted(&removed), "foo", "indentation not highlighted");
    }

    /// Messages must route their text through the markdown pipeline, so
    /// markdown syntax is parsed (emphasis markers stripped) and semantic
    /// spans carry the corresponding style.
    #[test]
    fn messages_render_through_markdown_pipeline() {
        let mut model = Model::new(120, 80);
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
        let mut model = Model::new(120, 80);
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

    /// Tool rows are keyed by call id, so two calls of the same tool never
    /// corrupt each other: output/state updates hit exactly the row they
    /// belong to (regression: rows used to be matched by name, so a second
    /// `bash` stole the first one's ToolEnd/output).
    #[test]
    fn same_name_tools_keep_independent_state_and_output() {
        let mut model = Model::new(120, 80);
        // First bash: streams two snapshots, then finishes.
        update(&mut model, Msg::ToolStart("tc-1".into(), "bash".into(), String::new()));
        update(&mut model, Msg::SetToolOutput("tc-1".into(), "bash".into(), "step1\n".into()));
        update(&mut model, Msg::SetToolOutput("tc-1".into(), "bash".into(), "step1\nstep2\n".into()));
        update(&mut model, Msg::ToolEnd("tc-1".into(), "bash".into(), false));
        // Second bash call starts while the first one's row exists.
        update(&mut model, Msg::ToolStart("tc-2".into(), "bash".into(), String::new()));
        update(&mut model, Msg::SetToolOutput("tc-2".into(), "bash".into(), "other\n".into()));

        let first = model.active_tools.iter().find(|t| t.call_id == "tc-1").expect("first");
        let second = model.active_tools.iter().find(|t| t.call_id == "tc-2").expect("second");
        // First bash: Done, snapshot replaced (no duplication), second untouched.
        assert!(matches!(first.state, ToolCallState::Done), "first bash done");
        assert_eq!(first.output, "step1\nstep2\n", "first output is the last snapshot, not doubled");
        assert!(matches!(second.state, ToolCallState::Running), "second bash still running");
        assert_eq!(second.output, "other\n", "second output lands on its own row");
    }

    /// TS tool-output preview: a tool with more than `FALLBACK_PREVIEW_LINES`
    /// (10) output lines renders the first 10 plus the TS hint
    /// `... (N more lines, ctrl+o to expand)`; Ctrl+O (app.tools.expand)
    /// renders the full output with no hint.
    #[test]
    fn tool_output_previews_10_lines_and_expands() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        let output: String = (1..=14).map(|i| format!("line{i}\n")).collect();
        // Tool without a registered renderer → TS fallback path.
        update(&mut model, Msg::ToolStart("tc-1".into(), "write".into(), String::new()));
        update(&mut model, Msg::SetToolOutput("tc-1".into(), "write".into(), output.clone()));
        update(&mut model, Msg::ToolEnd("tc-1".into(), "write".into(), false));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // The finished tool renders as a full box (never a one-row fold).
        // Locate by the tool box background (header text can contain the
        // same letters); the finished tool uses the success background.
        let title_row = (0..20)
            .find(|&y| buf[(1, y)].bg == theme::TOOL_SUCCESS_BG && buf[(1, y)].symbol() == "w")
            .expect("tool title");

        assert!(buf[(1, title_row)].modifier.contains(Modifier::BOLD), "title bold");
        // First 10 output lines visible (box rows: col 0 = padding, col 1
        // = first char — plain toolOutput color, no prefix).
        assert_eq!(buf[(1, title_row + 1)].symbol(), "l", "output line 1");
        let tenth = title_row + 10;
        assert_eq!(buf[(1, tenth)].symbol(), "l", "output line 10");
        // Hint row (TS): `... (4 more lines, ctrl+o to expand)` — muted
        // text, dim key.
        assert_eq!(buf[(1, tenth + 1)].symbol(), ".", "hint ellipsis start");
        assert_eq!(buf[(1, tenth + 1)].fg, theme::MUTED, "hint muted");
        assert_eq!(buf[(2, tenth + 1)].symbol(), ".", "hint ellipsis");
        assert_eq!(buf[(3, tenth + 1)].symbol(), ".", "hint ellipsis");
        assert_eq!(buf[(5, tenth + 1)].symbol(), "(", "hint paren");
        assert_eq!(buf[(6, tenth + 1)].symbol(), "4", "hint remaining count");
        assert_eq!(buf[(20, tenth + 1)].symbol(), "c", "hint key ctrl+o");
        assert_eq!(buf[(20, tenth + 1)].fg, theme::DIM, "hint key dim");
        assert_eq!(buf[(27, tenth + 1)].symbol(), "t", "hint to expand");
        assert_eq!(buf[(36, tenth + 1)].symbol(), ")", "hint closes");

        // Ctrl+O expands: full output (line 14 visible), no hint row.
        update(&mut model, Msg::ToggleToolExpansion);
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        let title_row = (0..24)
            .find(|&y| buf[(1, y)].bg == theme::TOOL_SUCCESS_BG && buf[(1, y)].symbol() == "w")
            .expect("title after expand");
        assert_eq!(buf[(1, title_row + 14)].symbol(), "l", "output line 14 after expand");
        assert_ne!(buf[(1, title_row + 15)].symbol(), ".", "no hint after expand");
    }

    /// TS original renders every message in full (no folding/truncation) —
    /// the transcript scrolls instead of collapsing long content.
    #[test]
    fn long_messages_render_in_full() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 40);
        model.context_usage_known = true;
        // Blank-line-separated rows: markdown renders each row as its own
        // line (adjacent lines merge into one paragraph — TS Markdown
        // behaves the same).
        let long: String = (1..=60).map(|i| format!("row{i}\n\n")).collect();
        update(&mut model, Msg::NewMessage("assistant".into(), long));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 40)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // The block is never truncated: the first line's second char is at
        // col 1 (assistant text has no box padding).
        let first_row = (0..30).find(|&y| buf[(0, y)].symbol() == "r").expect("first line");
        assert_eq!(buf[(1, first_row)].symbol(), "o", "row1 second char");
        // No fold/truncation hint anywhere: every row of the message is in
        // the layout (scrolling reveals the rest).
        assert_eq!(model.messages[0].md.render(88).len(), 119, "60 rows + 59 blank separators");
        for y in 0..30u16 {
            let row: String = (0..100u16).map(|x| buf[(x, y)].symbol().to_string()).collect();
            assert!(!row.contains("more"), "no truncation hint on row {y}: {row}");
        }
    }

    /// The input editor layout wraps by display column (CJK = 2) and
    /// reports the cursor row/col matching the byte offset. At width 6,
    /// "aaaa bbbb cccc" wraps to ["aaaa b", "bbbb c", "cc"].
    #[test]
    fn input_layout_tracks_cursor_across_wrapped_rows() {
        let mut input = Input::new();
        input.insert_str("aaaa bbbb cccc");
        let (rows, row, col) = input_layout_rows(&input, 6);
        assert_eq!(rows.len(), 3, "three wrapped rows: {rows:?}");
        assert_eq!((row, col), (2, 2), "cursor at end of the buffer (last row)");

        // Move the cursor to byte offset 4: row 0, column 4.
        input.move_home();
        for _ in 0..4 { input.move_right(); }
        let (rows, row, col) = input_layout_rows(&input, 6);
        assert_eq!(rows.len(), 3);
        assert_eq!((row, col), (0, 4), "cursor in the first row");
        assert_eq!(rows[0], "aaaa b", "greedy per-column wrap");
    }

    /// Render a small transcript through `view` into a test buffer and
    /// assert the TS-original layout pixels: the user message is a
    /// full-width `userMessageBg` box with a pad row above and below, the
    /// running tool call is a `toolPendingBg` box with the bold title, the
    /// editor draws `─` borders in `borderMuted`, and the footer right-
    /// aligns the model name. Guards the restyle against regressions.
    #[test]
    fn view_renders_ts_style_boxes_border_and_footer() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.model_name = "mock-model".into();
        model.cwd = "/tmp".into();
        model.context_usage_pct = 42.0;
        model.context_usage_known = true;
        update(&mut model, Msg::NewMessage("user".into(), "hello".into()));
        update(&mut model, Msg::ToolStart("tc-bash".into(), "read".into(), String::new()));
        update(&mut model, Msg::SetToolOutput("tc-bash".into(), "read".into(), "done\n".into()));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // ── Dock: status (2) + editor (3) + footer (2) at the bottom. ──
        // Editor borders are `─` in borderMuted.
        // Find the editor borders dynamically: the bottom-most full-width
        // `─` line in borderMuted is the editor's bottom border (the footer
        // below it never draws `─`); the top border sits two rows above.
        let bottom_border = (0..30)
            .rev()
            .find(|&y| buf[(50, y)].symbol() == "─" && buf[(50, y)].fg == theme::BORDER_MUTED && buf[(0, y)].symbol() == "─")
            .expect("editor bottom border");
        let top_border = bottom_border - 2;
        assert_eq!(buf[(50, top_border)].symbol(), "─", "editor top border");
        assert_eq!(
            buf[(50, top_border)].fg,
            theme::BORDER_MUTED,
            "editor border color"
        );

        // ── Footer: line 2 right-aligns the model name. ──
        // Scan the row backwards to the start of the right-aligned label.
        let mut model_col = 0u16;
        for x in (0..100).rev() {
            if buf[(x, 29)].symbol() != " " {
                model_col = x.saturating_sub("mock-model".len() as u16 - 1);
                break;
            }
        }
        assert_eq!(
            buf[(model_col, 29)].symbol(),
            "m",
            "model right-aligned in footer line 2"
        );
        assert_eq!(buf[(0, 29)].symbol(), "4", "context % on the footer left");

        // ── User message box (bottom-up: it sits above the tool). ──
        let hello_row = (0..24).find(|&y| buf[(1, y)].symbol() == "h").expect("user row");
        assert_eq!(buf[(1, hello_row)].bg, theme::USER_MESSAGE_BG, "user text on userMessageBg");
        assert_eq!(
            buf[(1, hello_row - 1)].bg,
            theme::USER_MESSAGE_BG,
            "box top padding"
        );
        assert_eq!(
            buf[(1, hello_row + 1)].bg,
            theme::USER_MESSAGE_BG,
            "box bottom padding"
        );

        // ── Tool box: running → toolPendingBg with the bold title. ──
        let tool_row = (0..24)
            .find(|&y| buf[(1, y)].bg == theme::TOOL_PENDING_BG && buf[(1, y)].symbol() == "r")
            .expect("tool title row");
        assert_eq!(buf[(1, tool_row)].bg, theme::TOOL_PENDING_BG, "running tool uses toolPendingBg");
        assert!(buf[(1, tool_row)].modifier.contains(Modifier::BOLD), "tool title bold");
        assert_eq!(buf[(1, tool_row + 1)].bg, theme::TOOL_PENDING_BG, "tool box pad");
    }

    /// The slash-command menu renders TS-style (the original SelectList):
    /// inline rows inside the editor — no border/title/background, `→ `
    /// accent prefix on the selected row, accent selected text, muted
    /// descriptions in an aligned column, and a `(n/m)` scroll row when
    /// the list scrolls.
    #[test]
    fn slash_menu_renders_ts_style_rows() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.model_name = "mock-model".into();
        model.completer.set_commands(vec![
            crate::components::CompletionCommand::new("/help", "Show commands", "help"),
            crate::components::CompletionCommand::new("/new", "Start a new session", "new"),
            crate::components::CompletionCommand::new("/name <name>", "Set the session name", "name"),
            crate::components::CompletionCommand::new("/model <provider>/<id>", "Switch model", "model"),
            crate::components::CompletionCommand::new("/reload", "Reload extensions", "reload"),
            crate::components::CompletionCommand::new("/quit", "Quit", "quit"),
        ]);
        model.completer.begin(crate::components::CompletionTrigger::Slash, "", "");
        model.completer.apply_results(1, vec![
            CompletionItem::new("help", "/help", "Show commands"),
            CompletionItem::new("new", "/new", "Start a new session"),
            CompletionItem::new("name", "/name <name>", "Set the session name"),
            CompletionItem::new("model", "/model <provider>/<id>", "Switch model"),
            CompletionItem::new("reload", "/reload", "Reload extensions"),
            CompletionItem::new("quit", "/quit", "Quit"),
        ]);

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // The menu lives inside the editor: find the first menu row (the
        // `→ ` prefix of the selected item) between the editor borders.
        let menu_row = (0..30)
            .find(|&y| buf[(0, y)].symbol() == "\u{2192}")
            .expect("menu row");
        // Selected row: `→ /help` in accent, description muted.
        assert_eq!(buf[(0, menu_row)].symbol(), "\u{2192}", "accent arrow prefix");
        assert_eq!(buf[(0, menu_row)].fg, theme::ACCENT, "arrow in accent");
        assert_eq!(buf[(2, menu_row)].symbol(), "/", "label follows the prefix");
        assert_eq!(buf[(2, menu_row)].fg, theme::ACCENT, "selected label in accent");
        // Description in muted, aligned after the primary column (widest
        // label `/model <provider>/<id>` = 22 chars + 2 gap = 24).
        let desc_col = 2 + 24;
        assert_eq!(buf[(desc_col, menu_row)].symbol(), "S", "description column");
        assert_eq!(buf[(desc_col, menu_row)].fg, theme::MUTED, "description muted");
        // Unselected row: two-space prefix, default text color.
        let next_row = menu_row + 1;
        assert_eq!(buf[(0, next_row)].symbol(), " ", "unselected prefix is spaces");
        assert_eq!(buf[(2, next_row)].symbol(), "/", "unselected label");
        assert_eq!(buf[(2, next_row)].fg, theme::TEXT, "unselected label in text color");
        // Scroll info row `(1/7)` in muted (7 items, 5 visible).
        let scroll_row = menu_row + 5;
        assert_eq!(buf[(2, scroll_row)].symbol(), "(", "scroll info row");
        assert_eq!(buf[(2, scroll_row)].fg, theme::MUTED, "scroll info muted");
        // No border/title: the editor top border sits above the content row
        // and the bottom border below the scroll info (the menu is inline).
        assert_eq!(buf[(50, menu_row - 2)].symbol(), "\u{2500}", "editor top border above menu");
        assert_eq!(buf[(50, scroll_row + 1)].symbol(), "\u{2500}", "editor bottom border below menu");
    }

    /// Tab 应用选中补全（对齐 TS `tui.input.tab`）：Tab 不是"下一个"，
    /// 而是把选中项插进输入框并关闭弹窗；Down 才是"下一个"。
    #[test]
    fn tab_applies_selected_completion() {
        let mut model = Model::new(100, 30);
        model.completer.set_commands(vec![
            crate::components::CompletionCommand::new("/new", "Start a new session", "new"),
        ]);
        model.input.set_value("/n");
        model.completer.begin(CompletionTrigger::Slash, "/n", "n");
        model.completer.apply_results(1, vec![CompletionItem::new("new", "/new", "Start a new session")]);
        let tab = KeyEvent::new(crossterm::event::KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
        let _cmds = update(&mut model, Msg::Key(tab));
        assert_eq!(model.input.value(), "/new ", "Tab 应用选中项并加空格");
        assert!(!model.completer.visible, "应用后弹窗关闭");
    }

    /// Tab 在无弹窗时触发补全（对齐 TS `handleTabCompletion`）：行首 `/`
    /// → slash 命令列表（同步），而不是插入 4 个空格。
    #[test]
    fn tab_without_popup_triggers_slash_completion() {
        let mut model = Model::new(100, 30);
        model.completer.set_commands(vec![
            crate::components::CompletionCommand::new("/model <provider>/<id>", "Switch model", "model"),
        ]);
        model.input.set_value("/mo");
        let tab = KeyEvent::new(crossterm::event::KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
        let _cmds = update(&mut model, Msg::Key(tab));
        assert!(model.completer.visible, "Tab 打开 slash 补全");
        assert_eq!(model.completer.query, "mo");
        assert_eq!(model.completer.results.len(), 1, "fuzzy mo → /model");
        assert_eq!(model.input.value(), "/mo", "不插入空格");
    }

    // ── 补全触发上下文（对齐 TS editor.ts 触发规则）────────────────────

    /// 行首 `/` 触发 slash 命令补全：候选同步算出（fuzzy），弹窗立即可见，
    /// 不需要异步请求（对齐 TS 的 fuzzyFilter；同步实现避免 Enter 竞态）。
    #[test]
    fn typing_slash_triggers_command_completion_request() {
        let mut model = Model::new(100, 30);
        model.completer.set_commands(vec![
            crate::components::CompletionCommand::new("/model <provider>/<id>", "Switch model", "model"),
            crate::components::CompletionCommand::new("/new", "Start a new session", "new"),
        ]);
        let cmds = update(&mut model, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Char('/'), crossterm::event::KeyModifiers::NONE)));
        assert!(model.completer.visible);
        assert_eq!(cmds.len(), 0, "slash 补全同步完成，无异步请求: {cmds:?}");
        assert_eq!(model.completer.results.len(), 2, "全部命令列出");
        // 继续输入 query 做 fuzzy 过滤。
        update(&mut model, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Char('m'), crossterm::event::KeyModifiers::NONE)));
        assert_eq!(model.completer.results.len(), 1, "fuzzy 'm' 只留 /model");
        assert_eq!(model.completer.results[0].value, "model");
    }

    /// `@` 在 token 边界（行首/空白后）触发附件补全，带 20ms debounce。
    #[test]
    fn at_at_token_boundary_triggers_with_debounce() {
        let mut model = Model::new(100, 30);
        let cmds = update(&mut model, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Char('@'), crossterm::event::KeyModifiers::NONE)));
        assert!(model.completer.visible, "@ 打开文件补全");
        let req = cmds.iter().find_map(|c| match c {
            Cmd::RequestCompletion(r) => Some(r),
            _ => None,
        });
        let req = req.expect("at 请求");
        assert_eq!(req.trigger, CompletionTrigger::At);
        assert_eq!(req.debounce_ms, 20, "附件补全 debounce 对齐 TS ATTACHMENT_AUTOCOMPLETE_DEBOUNCE_MS");
    }

    /// `@` 在词中间不触发（对齐 TS token 边界）。
    #[test]
    fn at_mid_word_does_not_trigger() {
        let mut model = Model::new(100, 30);
        for ch in ['a', 'b', 'c', '@'] {
            update(&mut model, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Char(ch), crossterm::event::KeyModifiers::NONE)));
        }
        assert!(!model.completer.visible, "abc@ 不应触发 @ 补全");
        assert_eq!(model.input.value(), "abc@");
    }

    /// `/cmd `（空格后）触发命令参数补全；未注册参数补全的命令不触发。
    #[test]
    fn slash_argument_completion_only_when_registered() {
        let mut model = Model::new(100, 30);
        let with_args = crate::components::CompletionCommand::new("/model <provider>/<id>", "Switch model", "model")
            .with_argument_completions(std::sync::Arc::new(|_p: String| {
                Box::pin(async { Some(Vec::new()) })
            }));
        model.completer.set_commands(vec![
            with_args,
            crate::components::CompletionCommand::new("/name <name>", "Set the session name", "name"),
        ]);
        // `/cmd g` → Argument 请求。
        let mut last = Vec::new();
        for ch in ['/', 'm', 'o', 'd', 'e', 'l', ' ', 'g'] {
            last = update(&mut model, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Char(ch), crossterm::event::KeyModifiers::NONE)));
        }
        let req = last.iter().find_map(|c| match c {
            Cmd::RequestCompletion(r) => Some(r),
            _ => None,
        });
        let req = req.expect("model 参数请求");
        assert_eq!(req.trigger, CompletionTrigger::Argument);
        assert_eq!(req.command.as_deref(), Some("model"));
        assert_eq!(req.query, "g");

        // `/name x` → 无参数补全注册 → 弹窗关闭、无请求。
        let mut model2 = Model::new(100, 30);
        model2.completer.set_commands(vec![
            crate::components::CompletionCommand::new("/new <name>", "Set the session name", "name"),
        ]);
        for ch in ['/', 'n', 'a', 'm', 'e', ' ', 'x'] {
            update(&mut model2, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Char(ch), crossterm::event::KeyModifiers::NONE)));
        }
        assert!(!model2.completer.visible, "无参数补全不弹窗");
    }

    /// Tab 在非 slash 上下文触发强制路径补全（对齐 TS handleTabCompletion）。
    #[test]
    fn tab_forces_path_completion() {
        let mut model = Model::new(100, 30);
        for ch in ['s', 'r', 'c'] {
            update(&mut model, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Char(ch), crossterm::event::KeyModifiers::NONE)));
        }
        let cmds = update(&mut model, Msg::Key(KeyEvent::new(crossterm::event::KeyCode::Tab, crossterm::event::KeyModifiers::NONE)));
        assert!(model.completer.visible, "Tab 打开路径补全");
        let req = cmds.iter().find_map(|c| match c {
            Cmd::RequestCompletion(r) => Some(r),
            _ => None,
        });
        let req = req.expect("path 请求");
        assert_eq!(req.trigger, CompletionTrigger::At);
        assert!(req.force, "Tab = force");
        assert_eq!(req.prefix, "src");
        assert_eq!(req.debounce_ms, 0);
    }
    /// The footer stats line matches the TS FooterComponent: token totals
    /// (`↑in ↓out Rcache Wcache`), cost, and the context display
    /// `{pct}%/{window} (auto)` colorized by threshold; the model label is
    /// right-aligned with the provider prefix and thinking level when the
    /// model reasons.
    #[test]
    fn footer_renders_ts_stats_and_model_label() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.model_name = "mock-model".into();
        model.provider = Some("openai".into());
        model.reasoning = true;
        model.thinking_level = Some("low".into());
        model.context_usage_pct = 42.5;
        model.context_usage_known = true;
        model.context_window = 200_000;
        model.usage_totals = UsageTotals {
            input: 1_234,
            output: 56_789,
            cache_read: 10_000,
            cache_write: 2_000,
            cost: 0.1234,
            cache_hit_rate: Some(80.0),
        };

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Line 2 (y=29): stats start with `↑1.2k` (formatTokens(1234)).
        assert_eq!(buf[(0, 29)].symbol(), "\u{2191}", "input arrow");
        assert_eq!(buf[(1, 29)].symbol(), "1", "input tokens");
        assert_eq!(buf[(2, 29)].symbol(), ".", "one decimal k");
        assert_eq!(buf[(3, 29)].symbol(), "2", "input tokens k");
        assert_eq!(buf[(4, 29)].symbol(), "k", "input tokens k suffix");
        // `↓57k` follows (formatTokens(56789) = 57k, rounded).
        assert_eq!(buf[(6, 29)].symbol(), "\u{2193}", "output arrow");
        assert_eq!(buf[(7, 29)].symbol(), "5", "output tokens");
        assert_eq!(buf[(8, 29)].symbol(), "7", "output tokens");
        assert_eq!(buf[(9, 29)].symbol(), "k", "output tokens k suffix");
        // `R10k W2.0k CH80.0% $0.123` then the context display.
        assert_eq!(buf[(11, 29)].symbol(), "R", "cache read");
        assert_eq!(buf[(16, 29)].symbol(), "W", "cache write");
        assert_eq!(buf[(18, 29)].symbol(), ".", "cache write decimal");
        assert_eq!(buf[(22, 29)].symbol(), "C", "cache hit rate");
        assert_eq!(buf[(24, 29)].symbol(), "8", "cache hit rate pct");
        assert_eq!(buf[(30, 29)].symbol(), "$", "cost");
        // Context display `42.5%/200k (auto)` — starts after the cost.
        let ctx_start = 30 + "$0.123".len() + 1;
        assert_eq!(buf[(ctx_start as u16, 29)].symbol(), "4", "context pct");
        assert_eq!(buf[(ctx_start as u16 + 4, 29)].symbol(), "%", "context pct %");
        assert_eq!(buf[(ctx_start as u16 + 5, 29)].symbol(), "/", "context window sep");
        assert_eq!(buf[(ctx_start as u16 + 6, 29)].symbol(), "2", "context window");
        assert_eq!(buf[(ctx_start as u16 + 9, 29)].symbol(), "k", "context window k");
        // ` (auto)` suffix.
        assert_eq!(buf[(ctx_start as u16 + 10, 29)].symbol(), " ", "auto pad");
        assert_eq!(buf[(ctx_start as u16 + 11, 29)].symbol(), "(", "auto indicator");
        assert_eq!(buf[(ctx_start as u16 + 16, 29)].symbol(), ")", "auto indicator end");

        // Right-aligned model label: `(openai) mock-model • low`.
        let right_label = "(openai) mock-model \u{2022} low";
        let mut model_col = 0u16;
        for x in (0..100).rev() {
            if buf[(x, 29)].symbol() != " " {
                model_col = x.saturating_sub(right_label.chars().count() as u16 - 1);
                break;
            }
        }
        assert_eq!(buf[(model_col, 29)].symbol(), "(", "provider prefix");
        assert_eq!(buf[(model_col + 9, 29)].symbol(), "m", "model name");
        assert_eq!(buf[(model_col + 20, 29)].symbol(), "\u{2022}", "thinking bullet");
        assert_eq!(buf[(model_col + 22, 29)].symbol(), "l", "thinking level");
    }

    /// Footer line 1 appends the session name after the branch:
    /// `~/path (branch) • name` (TS `pwd • sessionName`).
    #[test]
    fn footer_line1_renders_session_name() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.cwd = "/tmp".into();
        model.git_branch = Some("main".into());
        model.session_name = Some("My Session".into());

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Line 1 (y=28): `/tmp (main) • My Session` in dim.
        assert_eq!(buf[(0, 28)].symbol(), "/", "pwd starts");
        assert_eq!(buf[(5, 28)].symbol(), "(", "branch paren");
        assert_eq!(buf[(10, 28)].symbol(), ")", "branch close");
        assert_eq!(buf[(12, 28)].symbol(), "\u{2022}", "session bullet");
        assert_eq!(buf[(14, 28)].symbol(), "M", "session name");
        assert_eq!(buf[(0, 28)].fg, theme::DIM, "pwd line dim");
    }

    /// The startup header renders the TS compact text: logo (accent bold +
    /// dim version), the one-line hints, the press hint and the onboarding
    /// line. Ctrl+O (ToggleToolExpansion) switches to the expanded list.
    #[test]
    fn startup_header_renders_compact_and_expands() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Logo row: `Pi` in accent bold, ` v1.83.1` dim.
        assert_eq!(buf[(0, 0)].symbol(), "P", "logo P");
        assert_eq!(buf[(0, 0)].fg, theme::ACCENT, "logo accent");
        assert!(buf[(0, 0)].modifier.contains(Modifier::BOLD), "logo bold");
        assert_eq!(buf[(3, 0)].symbol(), "v", "version prefix");
        assert_eq!(buf[(3, 0)].fg, theme::DIM, "version dim");

        // Compact hints row: `escape interrupt · ctrl+c/ctrl+d clear/exit
        // · / commands · ! bash · ctrl+o more`.
        assert_eq!(buf[(0, 1)].symbol(), "e", "escape key");
        assert_eq!(buf[(0, 1)].fg, theme::DIM, "key dim");
        assert_eq!(buf[(7, 1)].symbol(), "i", "interrupt desc");
        assert_eq!(buf[(7, 1)].fg, theme::MUTED, "desc muted");
        assert_eq!(buf[(17, 1)].symbol(), "\u{00b7}", "separator");
        assert_eq!(buf[(19, 1)].symbol(), "c", "ctrl+c key");
        assert_eq!(buf[(46, 1)].symbol(), "/", "slash key");
        assert_eq!(buf[(59, 1)].symbol(), "!", "bang key");
        assert_eq!(buf[(68, 1)].symbol(), "c", "ctrl+o key");

        // Press hint + onboarding.
        assert_eq!(buf[(0, 2)].symbol(), "P", "press hint");
        assert_eq!(buf[(0, 4)].symbol(), "P", "onboarding line");

        // Ctrl+O expands: the full keybinding list appears.
        update(&mut model, Msg::ToggleToolExpansion);
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(0, 1)].symbol(), "e", "expanded: escape to interrupt");
        assert_eq!(buf[(0, 2)].symbol(), "c", "expanded: ctrl+c to clear");
        assert_eq!(buf[(0, 3)].symbol(), "c", "expanded: ctrl+c twice to exit");
        assert_eq!(buf[(0, 4)].symbol(), "c", "expanded: ctrl+d to exit (empty)");
        assert_eq!(buf[(0, 5)].symbol(), "c", "expanded: ctrl+z to suspend");
        assert_eq!(buf[(0, 6)].symbol(), "c", "expanded: ctrl+k to delete to end");
        assert_eq!(buf[(0, 7)].symbol(), "s", "expanded: shift+tab");
        assert_eq!(buf[(0, 8)].symbol(), "c", "expanded: ctrl+p");
        assert_eq!(buf[(0, 9)].symbol(), "c", "expanded: ctrl+l");
        assert_eq!(buf[(0, 10)].symbol(), "c", "expanded: ctrl+o");
        assert_eq!(buf[(0, 11)].symbol(), "c", "expanded: ctrl+t");
        assert_eq!(buf[(0, 12)].symbol(), "c", "expanded: ctrl+g");
        assert_eq!(buf[(0, 13)].symbol(), "/", "expanded: slash");
        assert_eq!(buf[(0, 14)].symbol(), "!", "expanded: bang");
        assert_eq!(buf[(0, 15)].symbol(), "!", "expanded: double bang");
        assert_eq!(buf[(0, 16)].symbol(), "a", "expanded: alt+enter");
        assert_eq!(buf[(0, 17)].symbol(), "a", "expanded: alt+up");
        assert_eq!(buf[(0, 18)].symbol(), "c", "expanded: ctrl+v");
        assert_eq!(buf[(0, 19)].symbol(), "d", "expanded: drop files");
        assert_eq!(buf[(0, 21)].symbol(), "P", "expanded: onboarding");
    }

    /// Tool calls render their args below the title (TS fallback: blank
    /// line + `JSON.stringify(args, null, 2)` in the default text color).
    #[test]
    fn tool_block_renders_args_json() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart(
                "tc-1".into(),
                "write".into(),
                "{\n  \"path\": \"a.txt\"\n}".into(),
            ),
        );

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Title row: `write` bold on toolPendingBg.
        let title_row = (0..24).find(|&y| buf[(1, y)].symbol() == "w").expect("title row");
        assert!(buf[(1, title_row)].modifier.contains(Modifier::BOLD), "title bold");
        // Blank separator row.
        assert_eq!(buf[(1, title_row + 1)].symbol(), " ", "blank after title");
        // Args row: `{` in the default text color (no fg override).
        assert_eq!(buf[(1, title_row + 2)].symbol(), "{", "args row");
        assert_eq!(buf[(1, title_row + 2)].fg, Color::Reset, "args in default text color");
        assert_eq!(buf[(1, title_row + 3)].symbol(), " ", "args indent");
        assert_eq!(buf[(3, title_row + 3)].symbol(), "\"", "args second row");
        assert_eq!(buf[(1, title_row + 4)].symbol(), "}", "args last row");
    }

    /// The bash tool uses the TS bash renderer: `$ {command}` bold title
    /// (+ muted timeout suffix), tail-5-line preview with a leading
    /// `... (N earlier lines, ctrl+o to expand)` hint when output is
    /// skipped, and the muted `Took {x.x}s` timer line once finished.
    #[test]
    fn bash_tool_renders_dollar_title_tail_preview_and_timer() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        let output: String = (1..=9).map(|i| format!("out{i}\n")).collect();
        update(
            &mut model,
            Msg::ToolStart(
                "tc-b".into(),
                "bash".into(),
                "{\n  \"command\": \"echo hi\",\n  \"timeout\": 30\n}".into(),
            ),
        );
        update(&mut model, Msg::SetToolOutput("tc-b".into(), "bash".into(), output));
        update(&mut model, Msg::ToolEnd("tc-b".into(), "bash".into(), false));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Title: `$ echo hi` bold + muted ` (timeout 30s)`.
        let title_row = (0..20)
            .find(|&y| buf[(1, y)].bg == theme::TOOL_SUCCESS_BG && buf[(1, y)].symbol() == "$")
            .expect("bash title");
        // `$ echo hi` — col 1 = `$`, col 3 = first command char.
        assert_eq!(buf[(3, title_row)].symbol(), "e", "$ command");
        assert!(buf[(1, title_row)].modifier.contains(Modifier::BOLD), "title bold");
        assert_eq!(buf[(11, title_row)].symbol(), "(", "timeout suffix");
        assert_eq!(buf[(11, title_row)].fg, theme::MUTED, "timeout muted");
        assert_eq!(buf[(22, title_row)].symbol(), "s", "timeout unit");

        // No args JSON for bash (the command lives in the title).
        assert_eq!(buf[(1, title_row + 1)].symbol(), " ", "blank line");

        // Tail preview: 9 visual lines → last 5 kept, 4 earlier skipped.
        // Rows: blank + hint + 5 lines.
        assert_eq!(buf[(1, title_row + 2)].symbol(), ".", "earlier hint");
        assert_eq!(buf[(1, title_row + 2)].fg, theme::MUTED, "hint muted");
        assert_eq!(buf[(23, title_row + 2)].symbol(), "c", "hint key");
        assert_eq!(buf[(1, title_row + 3)].symbol(), "o", "tail line 1 (out5)");
        assert_eq!(buf[(1, title_row + 7)].symbol(), "o", "tail line 5 (out9)");
        assert_eq!(buf[(1, title_row + 8)].symbol(), " ", "blank before timer");

        // Timer: `Took 0.0s` muted (finished instantly in the test).
        assert_eq!(buf[(1, title_row + 9)].symbol(), "T", "Took label");
        assert_eq!(buf[(1, title_row + 9)].fg, theme::MUTED, "timer muted");

        // Ctrl+O expands: all 9 lines, no hint, no skipped count.
        update(&mut model, Msg::ToggleToolExpansion);
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        let title_row = (0..20)
            .find(|&y| buf[(1, y)].bg == theme::TOOL_SUCCESS_BG && buf[(1, y)].symbol() == "$")
            .expect("bash title expanded");
        assert_eq!(buf[(1, title_row + 2)].symbol(), "o", "expanded line 1 (out1)");
        assert_eq!(buf[(1, title_row + 10)].symbol(), "o", "expanded line 9 (out9)");
        assert_eq!(buf[(2, title_row + 3)].symbol(), "u", "expanded line 2 second char");
    }

    /// A running bash tool shows the live `Elapsed {x.x}s` line (the tick
    /// re-renders it); once finished it switches to `Took {x.x}s`.
    #[test]
    fn bash_tool_shows_elapsed_while_running() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart("tc-b".into(), "bash".into(), "{\"command\": \"sleep 1\"}".into()),
        );

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Running → `Elapsed` (pending background).
        let title_row = (0..20)
            .find(|&y| buf[(1, y)].bg == theme::TOOL_PENDING_BG && buf[(1, y)].symbol() == "$")
            .expect("bash title");
        let timer = (0..30u16)
            .find(|&y| buf[(1, y)].symbol() == "E" && buf[(1, y)].fg == theme::MUTED)
            .expect("elapsed line");
        assert!(timer > title_row, "timer below the title");
        // `Elapsed 0.0s` — the unit `s` sits after the one-decimal value.
        assert_eq!(buf[(12, timer)].symbol(), "s", "elapsed seconds unit");
    }

    /// Tool calls interleave with messages in event order (TS
    /// chatContainer): a tool call sits *after* the assistant message that
    /// requested it — not all tools above all messages. The transcript
    /// order is user → assistant → tool → user.
    #[test]
    fn tool_blocks_interleave_with_messages_in_event_order() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 60);
        model.context_usage_known = true;
        update(&mut model, Msg::NewMessage("user".into(), "first".into()));
        update(&mut model, Msg::NewMessage("assistant".into(), "calling tool".into()));
        update(&mut model, Msg::ToolStart("tc-1".into(), "bash".into(), "{\"command\": \"echo hi\"}".into()));
        update(&mut model, Msg::SetToolOutput("tc-1".into(), "bash".into(), "hi\n".into()));
        update(&mut model, Msg::ToolEnd("tc-1".into(), "bash".into(), false));
        update(&mut model, Msg::NewMessage("user".into(), "second".into()));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 60)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Scan the transcript rows (after the header) for the block order.
        let row_of = |needle: &str, bg: ratatui::style::Color| -> u16 {
            (6..60u16)
                .find(|&y| buf[(1, y)].symbol() == needle && buf[(1, y)].bg == bg)
                .unwrap_or(u16::MAX)
        };
        let user1 = row_of("f", theme::USER_MESSAGE_BG);
        let tool = row_of("$", theme::TOOL_SUCCESS_BG);
        let user2 = row_of("s", theme::USER_MESSAGE_BG);
        // The assistant text sits between user1 and the tool (no box bg).
        let assistant = (6..60u16)
            .find(|&y| buf[(0, y)].symbol() == "c")
            .unwrap_or(u16::MAX);
        assert!(user1 < assistant, "user1 before assistant");
        assert!(assistant < tool, "assistant before its tool call");
        assert!(tool < user2, "tool before user2 — interleaved, not above");
    }

    /// The read renderer (TS `formatReadCall`): title `read {path}{:range}`
    /// with the path in accent and the range in warning; the content only
    /// renders when expanded (TS `formatReadResult` returns "" unless
    /// expanded).
    #[test]
    fn read_tool_shows_title_only_until_expanded() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        model.cwd = "/work".into();
        update(
            &mut model,
            Msg::ToolStart(
                "tc-r".into(),
                "read".into(),
                "{\n  \"file_path\": \"/work/src/main.rs\",\n  \"offset\": 10,\n  \"limit\": 5\n}".into(),
            ),
        );
        update(&mut model, Msg::SetToolOutput("tc-r".into(), "read".into(), "line1\nline2\n".into()));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Title: `read /work/src/main.rs:10-14` — "read" bold, path
        // accent, range warning.
        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "r" && buf[(1, y)].bg == theme::TOOL_PENDING_BG)
            .expect("read title");
        assert!(buf[(1, title_row)].modifier.contains(Modifier::BOLD), "read bold");
        assert_eq!(buf[(6, title_row)].symbol(), "/", "path starts");
        assert_eq!(buf[(6, title_row)].fg, theme::ACCENT, "path accent");
        assert_eq!(buf[(23, title_row)].symbol(), ":", "range starts");
        assert_eq!(buf[(23, title_row)].fg, theme::WARNING, "range warning");

        // Collapsed: no output content (title only).
        assert_eq!(buf[(1, title_row + 1)].symbol(), " ", "no content collapsed");
        assert_ne!(buf[(1, title_row + 1)].symbol(), "l", "content hidden");

        // Expanded: content shows.
        update(&mut model, Msg::ToggleToolExpansion);
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "r" && buf[(1, y)].bg == theme::TOOL_PENDING_BG)
            .expect("read title expanded");
        assert_eq!(buf[(1, title_row + 2)].symbol(), "l", "content visible when expanded");
    }

    /// The grep renderer (TS `formatGrepCall`): `grep /{pattern}/ in
    /// {path}` — "grep" bold, pattern accent, ` in path` in toolOutput.
    #[test]
    fn grep_tool_renders_pattern_call_line() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart(
                "tc-g".into(),
                "grep".into(),
                "{\n  \"pattern\": \"TODO\",\n  \"path\": \"src\"\n}".into(),
            ),
        );
        update(&mut model, Msg::SetToolOutput("tc-g".into(), "grep".into(), "src/main.rs:1:TODO\n".into()));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "g" && buf[(1, y)].bg == theme::TOOL_PENDING_BG)
            .expect("grep title");
        assert_eq!(buf[(6, title_row)].symbol(), "/", "pattern slashes");
        assert_eq!(buf[(6, title_row)].fg, theme::ACCENT, "pattern accent");
        assert_eq!(buf[(13, title_row)].symbol(), "i", "in path");
        assert_eq!(buf[(13, title_row)].fg, theme::TOOL_OUTPUT, "in path toolOutput");
        // Output renders below a blank line (grep shows output collapsed).
        assert_eq!(buf[(1, title_row + 2)].symbol(), "s", "grep result line");
    }

    /// The bash truncation metadata renders the TS warning line
    /// `[Truncated: showing N of M lines]` / `[Full output: path]`.
    #[test]
    fn bash_tool_renders_truncation_warning() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart("tc-b".into(), "bash".into(), "{\"command\": \"make\"}".into()),
        );
        update(&mut model, Msg::SetToolOutput("tc-b".into(), "bash".into(), "out\n".into()));
        update(
            &mut model,
            Msg::SetToolTruncation(
                "tc-b".into(),
                Some(ToolTruncation {
                    truncated: true,
                    truncated_by: Some("lines".into()),
                    output_lines: 100,
                    total_lines: 500,
                    max_lines: 2000,
                    max_bytes: 51200,
                    full_output_path: Some("/tmp/pi-out.txt".into()),
                    ..ToolTruncation::default()
                }),
            ),
        );
        update(&mut model, Msg::ToolEnd("tc-b".into(), "bash".into(), false));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "$" && buf[(1, y)].bg == theme::TOOL_SUCCESS_BG)
            .expect("bash title");
        // Warning line: `[Full output: /tmp/pi-out.txt. Truncated: showing
        // 100 of 500 lines]` in the warning color.
        let warn_row = (0..30u16)
            .find(|&y| buf[(1, y)].symbol() == "[" && buf[(1, y)].fg == theme::WARNING)
            .expect("warning line");
        assert!(warn_row > title_row, "warning below the title");
        assert_eq!(buf[(2, warn_row)].symbol(), "F", "Full output");
        assert_eq!(buf[(30, warn_row)].symbol(), ".", "joined warnings");
        assert_eq!(buf[(32, warn_row)].symbol(), "T", "Truncated");
        // Timer still present below the warning.
        let took_row = (0..30u16)
            .find(|&y| buf[(1, y)].symbol() == "T" && buf[(1, y)].fg == theme::MUTED)
            .expect("Took line");
        assert!(took_row > warn_row, "timer below the warning");
    }

    /// TS `rebuildBashResultRenderComponent` strips the model-facing
    /// truncation footer (`\n\n[Showing ... Full output: path]`) from the
    /// displayed output and re-renders the warning from `details` — without
    /// the strip the footer would show twice (once as output text, once as
    /// the warning line).
    #[test]
    fn bash_tool_strips_truncation_footer_from_output() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        let footer = "\n\n[Showing lines 6-10 of 10. Full output: /tmp/pi-bash-x.log]";
        update(
            &mut model,
            Msg::ToolStart("tc-b".into(), "bash".into(), "{\"command\": \"make\"}".into()),
        );
        update(
            &mut model,
            Msg::SetToolOutput(
                "tc-b".into(),
                "bash".into(),
                format!("alpha\nbeta{footer}"),
            ),
        );
        update(
            &mut model,
            Msg::SetToolTruncation(
                "tc-b".into(),
                Some(ToolTruncation {
                    truncated: true,
                    truncated_by: Some("lines".into()),
                    output_lines: 5,
                    total_lines: 10,
                    max_lines: 2000,
                    max_bytes: 51200,
                    full_output_path: Some("/tmp/pi-bash-x.log".into()),
                    ..ToolTruncation::default()
                }),
            ),
        );
        update(&mut model, Msg::ToolEnd("tc-b".into(), "bash".into(), false));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "$")
            .expect("bash title");
        // The footer must not appear as output text: `S` (Showing) would be
        // its first char, but the only `[` in the block is the warning line.
        let output_row = title_row + 2;
        assert_eq!(buf[(1, output_row)].symbol(), "a", "alpha first output line");
        let footer_start = (0..30u16).find(|&y| buf[(1, y)].symbol() == "S");
        assert_eq!(footer_start, None, "footer stripped from output");
        // The warning line still renders from details (one `[` row only).
        let warn_row = (0..30u16)
            .find(|&y| buf[(1, y)].symbol() == "[" && buf[(1, y)].fg == theme::WARNING)
            .expect("warning line");
        assert_eq!(buf[(2, warn_row)].symbol(), "F", "warning starts Full output");
    }

    /// Tool output is sanitized for display exactly like the TS renderers'
    /// `getTextOutput`: ANSI escape codes stripped, `\r` removed, binary
    /// control characters dropped (the raw tool result keeps ANSI for the
    /// model — only the TUI display cleans it).
    #[test]
    fn tool_output_strips_ansi_and_control_chars() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart("tc-b".into(), "bash".into(), "{\"command\": \"echo hi\"}".into()),
        );
        // `\x1b[31mred\x1b[0m`, a CRLF, and a raw NUL control char.
        update(
            &mut model,
            Msg::SetToolOutput("tc-b".into(), "bash".into(), "\u{1b}[31mred\u{1b}[0m\r\n\u{0}ok".into()),
        );

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "$" && buf[(1, y)].bg == theme::TOOL_PENDING_BG)
            .expect("bash title");
        // Output starts with the visible text `red` (no `\x1b[` garbage).
        assert_eq!(buf[(1, title_row + 2)].symbol(), "r", "ANSI stripped");
        assert_eq!(buf[(2, title_row + 2)].symbol(), "e", "second char");
        assert_eq!(buf[(3, title_row + 2)].symbol(), "d", "third char");
        // The CRLF collapsed to a single newline → `ok` on the next row
        // (no raw `\r` cell).
        assert_eq!(buf[(1, title_row + 3)].symbol(), "o", "CRLF collapsed");
        assert_eq!(buf[(2, title_row + 3)].symbol(), "k", "NUL removed");
    }

    /// Word-wrap matches the TS `Text` renderer: lines break at word
    /// boundaries (not mid-word), long tokens break at the column limit,
    /// tabs expand to 3 spaces, and every line is right-trimmed.
    #[test]
    fn visual_lines_wrap_at_word_boundaries() {
        // `aaaa bbbb` at width 4: TS `wrapTextWithAnsi` keeps the space
        // with the first line → `["aaaa","bbbb"]` (the old greedy
        // per-column wrap produced `["aaaa"," bbb","b"]`).
        assert_eq!(visual_lines("aaaa bbbb", 4), vec!["aaaa", "bbbb"]);
        // Long token breaks at the column limit (CJK wide glyph = 2 cols):
        // chunks of ≤6 columns → "你好世"(6) / "界hell"(6) / "o"(1).
        assert_eq!(visual_lines("你好世界hello", 6), vec!["你好世", "界hell", "o"]);
        // Tabs expand to 3 spaces (TS `Text.render`).
        assert_eq!(visual_lines("a\tb", 20), vec!["a   b"]);
        // Trailing whitespace is trimmed.
        assert_eq!(visual_lines("foo   ", 20), vec!["foo"]);
        // Empty input → no lines (TS `truncateToVisualLines`).
        assert!(visual_lines("", 20).is_empty());
    }

    /// `formatBashCall` arg handling: a non-string `command` renders
    /// `[invalid arg]` in the error color, an empty/missing command renders
    /// `...` in the toolOutput color, and a fractional timeout keeps its
    /// exact value while `0` renders no suffix (TS truthy check).
    #[test]
    fn bash_title_handles_invalid_command_and_timeout() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart(
                "tc-1".into(),
                "bash".into(),
                "{\"command\": 123, \"timeout\": 0}".into(),
            ),
        );
        update(
            &mut model,
            Msg::ToolStart(
                "tc-2".into(),
                "bash".into(),
                "{\"command\": \"\", \"timeout\": 1.5}".into(),
            ),
        );

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        let rows: Vec<u16> = (0..24u16)
            .filter(|&y| buf[(1, y)].symbol() == "$")
            .collect();
        assert_eq!(rows.len(), 2, "two bash titles");
        // tc-1: `[invalid arg]` in error color; timeout 0 → no suffix.
        let r1 = rows[0];
        assert_eq!(buf[(3, r1)].symbol(), "[", "invalid arg bracket");
        assert_eq!(buf[(3, r1)].fg, theme::ERROR, "invalid arg error color");
        assert_eq!(buf[(16, r1)].symbol(), " ", "no timeout suffix for 0");
        // tc-2: `...` in toolOutput color; fractional timeout 1.5 kept.
        let r2 = rows[1];
        assert_eq!(buf[(3, r2)].symbol(), ".", "empty command ellipsis");
        assert_eq!(buf[(3, r2)].fg, theme::TOOL_OUTPUT, "ellipsis toolOutput");
        assert_eq!(buf[(7, r2)].symbol(), "(", "timeout suffix");
        assert_eq!(buf[(17, r2)].symbol(), ".", "fractional timeout dot");
        assert_eq!(buf[(19, r2)].symbol(), "s", "timeout unit");
    }

    /// A running bash tool with no output renders the timer without a
    /// stray blank after the title (TS only prepends the newline inside the
    /// output Text when output exists), and whitespace-only output renders
    /// no output rows either.
    #[test]
    fn bash_tool_no_output_has_no_extra_blank() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart("tc-b".into(), "bash".into(), "{\"command\": \"true\"}".into()),
        );
        update(
            &mut model,
            Msg::SetToolOutput("tc-b".into(), "bash".into(), "\n  \n".into()),
        );
        update(&mut model, Msg::ToolEnd("tc-b".into(), "bash".into(), false));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "$")
            .expect("bash title");
        // Whitespace-only output: no blank/output rows, timer directly
        // after the title (title + blank + timer).
        assert_eq!(buf[(1, title_row + 1)].symbol(), " ", "blank before timer");
        assert_eq!(buf[(1, title_row + 2)].symbol(), "T", "Took directly after");
    }

    /// read truncation warnings (TS `formatReadResult`): expanded read with
    /// `details.truncation` renders `[Truncated: showing N of M lines (N
    /// line limit)]` in the warning color.
    #[test]
    fn read_tool_renders_truncation_warning() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart(
                "tc-r".into(),
                "read".into(),
                "{\"file_path\": \"/tmp/a.rs\"}".into(),
            ),
        );
        update(&mut model, Msg::SetToolOutput("tc-r".into(), "read".into(), "line1\nline2\n".into()));
        update(
            &mut model,
            Msg::SetToolTruncation(
                "tc-r".into(),
                Some(ToolTruncation {
                    truncated: true,
                    truncated_by: Some("lines".into()),
                    output_lines: 2,
                    total_lines: 500,
                    max_lines: 2000,
                    max_bytes: 51200,
                    full_output_path: None,
                    ..ToolTruncation::default()
                }),
            ),
        );
        update(&mut model, Msg::ToolEnd("tc-r".into(), "read".into(), false));

        // Collapsed: no content, no warning (TS `formatReadResult` = "").
        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        let _title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "r" && buf[(1, y)].bg == theme::TOOL_SUCCESS_BG)
            .expect("read title");
        let warn = (0..30u16).find(|&y| buf[(1, y)].symbol() == "[" && buf[(1, y)].fg == theme::WARNING);
        assert_eq!(warn, None, "no warning while collapsed");

        // Expanded: content + warning line.
        update(&mut model, Msg::ToggleToolExpansion);
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        let title_row = (0..20u16)
            .find(|&y| buf[(1, y)].symbol() == "r" && buf[(1, y)].bg == theme::TOOL_SUCCESS_BG)
            .expect("read title expanded");
        assert_eq!(buf[(1, title_row + 2)].symbol(), "l", "content visible");
        let warn_row = (0..30u16)
            .find(|&y| buf[(1, y)].symbol() == "[" && buf[(1, y)].fg == theme::WARNING)
            .expect("warning line expanded");
        assert_eq!(buf[(2, warn_row)].symbol(), "T", "Truncated");
        assert_eq!(buf[(36, warn_row)].symbol(), "(", "line limit paren");
    }

    /// grep truncation warnings (TS `formatGrepResult`): `[Truncated: N
    /// matches limit, ...]` joined with ", " in the warning color.
    #[test]
    fn grep_tool_renders_truncation_warning() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(
            &mut model,
            Msg::ToolStart(
                "tc-g".into(),
                "grep".into(),
                "{\"pattern\": \"TODO\", \"path\": \"src\"}".into(),
            ),
        );
        update(&mut model, Msg::SetToolOutput("tc-g".into(), "grep".into(), "src/a.rs:1:TODO\n".into()));
        update(
            &mut model,
            Msg::SetToolTruncation(
                "tc-g".into(),
                Some(ToolTruncation {
                    truncated: false,
                    truncated_by: None,
                    output_lines: 0,
                    total_lines: 0,
                    max_lines: 0,
                    max_bytes: 0,
                    full_output_path: None,
                    match_limit_reached: Some(100),
                    ..ToolTruncation::default()
                }),
            ),
        );
        update(&mut model, Msg::ToolEnd("tc-g".into(), "grep".into(), false));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        let warn_row = (0..30u16)
            .find(|&y| buf[(1, y)].symbol() == "[" && buf[(1, y)].fg == theme::WARNING)
            .expect("grep warning");
        // `[Truncated: 100 matches limit]`
        assert_eq!(buf[(2, warn_row)].symbol(), "T", "Truncated");
        assert_eq!(buf[(13, warn_row)].symbol(), "1", "match count 1");
        assert_eq!(buf[(14, warn_row)].symbol(), "0", "match count 0");
        assert_eq!(buf[(15, warn_row)].symbol(), "0", "match count 0");
    }

    /// `stripAnsi` (TS utils/ansi.ts) removes CSI and OSC sequences but
    /// keeps plain text; `sanitize_output_text` additionally drops binary
    /// control characters and `\r` (TS `getTextOutput`).
    #[test]
    fn strip_ansi_and_sanitize_match_ts() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("\u{1b}[38;5;196mhi\u{1b}[m"), "hi");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{07}plain"), "plain");
        assert_eq!(strip_ansi("no codes"), "no codes");
        assert_eq!(sanitize_output_text("\u{1b}[31mred\u{1b}[0m\r\n\u{0}ok"), "red\nok");
    }

    /// ratatui 0.29 的 `Terminal` 每帧**不重置 buffer**：diff 渲染只重绘
    /// 变化的 cell，未重绘的 cell 保留两个帧前写入的内容。当一个 Span/行
    /// 被裁剪（内容变短或整行消失）后，旧内容会在 A/B 两个 buffer 里
    /// 交替残留，导致屏幕上旧字符反复"回魂"。本测试复现：bash 输出从
    /// 3 行缩成 1 行后，第 3 帧仍不能出现被裁剪掉的旧行。
    #[test]
    fn body_clears_stale_cells_after_output_shrinks() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(&mut model, Msg::ToolStart("tc-b".into(), "bash".into(), "{\"command\": \"make\"}".into()));
        update(
            &mut model,
            Msg::SetToolOutput("tc-b".into(), "bash".into(), "alpha\nbeta\ngamma\n".into()),
        );
        update(&mut model, Msg::ToolEnd("tc-b".into(), "bash".into(), false));

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");

        // Frame 1: full output (3 rows) visible.
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        let beta_row = (0..30u16)
            .find(|&y| buf[(1, y)].symbol() == "b" && buf[(2, y)].symbol() == "e")
            .expect("beta row frame 1");

        // Output shrinks to one line (snapshot replacement semantics).
        update(&mut model, Msg::SetToolOutput("tc-b".into(), "bash".into(), "alpha\n".into()));

        // Frame 2: stale rows must be erased.
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(1, beta_row)].symbol(), " ", "beta erased frame 2");

        // Frame 3: the erased rows must STAY erased (the old buffer must not
        // resurface the clipped content).
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(1, beta_row)].symbol(), " ", "beta stays erased frame 3");
    }

    /// 暴力残留检查：把同一最终状态渲染进（a）经历过多次增量绘制的
    /// live terminal 与（b）只绘制一次的全新 terminal，逐行比较。若 diff
    /// 渲染在正文/工具块留下了旧 Buffer 内容，两者会不一致。
    #[test]
    fn body_frames_match_fresh_render_no_residue() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        fn build() -> (Model, RatTerminal<TestBackend>) {
            let mut model = Model::new(70, 24);
            model.context_usage_known = true;
            update(&mut model, Msg::NewMessage("user".into(), "**bold** and `code` line with 中文".into()));
            update(&mut model, Msg::NewMessage("assistant".into(), "Start".into()));
            update(&mut model, Msg::StreamText(" streaming more words here to wrap across several rows and re-flow as it grows".into()));
            update(&mut model, Msg::StreamEnd);
            update(&mut model, Msg::ToolStart("tc-1".into(), "bash".into(), "{\"command\":\"make\"}".into()));
            update(&mut model, Msg::SetToolOutput("tc-1".into(), "bash".into(), "alpha\nbeta\ngamma\n".into()));
            update(&mut model, Msg::ToolEnd("tc-1".into(), "bash".into(), false));
            let terminal = RatTerminal::new(TestBackend::new(70, 24)).expect("backend");
            (model, terminal)
        }

        fn grid(term: &RatTerminal<TestBackend>) -> Vec<String> {
            let buf = term.backend().buffer();
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
                .collect()
        }

        // Live: draw a few frames with mutations between draws.
        let (mut live, mut term) = build();
        let mut seq = vec![
            Msg::SetToolOutput("tc-1".into(), "bash".into(), "alpha\n".into()),
            Msg::StreamText(" MORE".into()),
            Msg::ToggleToolExpansion,
            Msg::SetToolOutput("tc-1".into(), "bash".into(), "alpha\nbeta\n".into()),
            Msg::ToggleToolExpansion,
            Msg::ScrollDown(3),
            Msg::ScrollToBottom,
        ];
        term.draw(|f| view(&mut live, f)).expect("draw1");
        for m in seq.drain(..) {
            update(&mut live, m);
            term.draw(|f| view(&mut live, f)).expect("draw");
        }

        // Fresh: same message sequence, drawn once.
        let (mut fresh, mut fterm) = build();
        update(&mut fresh, Msg::SetToolOutput("tc-1".into(), "bash".into(), "alpha\n".into()));
        update(&mut fresh, Msg::StreamText(" MORE".into()));
        update(&mut fresh, Msg::ToggleToolExpansion);
        update(&mut fresh, Msg::SetToolOutput("tc-1".into(), "bash".into(), "alpha\nbeta\n".into()));
        update(&mut fresh, Msg::ToggleToolExpansion);
        update(&mut fresh, Msg::ScrollDown(3));
        update(&mut fresh, Msg::ScrollToBottom);
        fterm.draw(|f| view(&mut fresh, f)).expect("fresh draw");

        let (g1, g2) = (grid(&term), grid(&fterm));
        for (y, (a, b)) in g1.iter().zip(g2.iter()).enumerate() {
            assert_eq!(a, b, "row {y} residue: live={a:?} fresh={b:?}");
        }
        assert_eq!(g1.len(), g2.len());
    }

    /// 用户报告的残留场景：markdown 正文里混排正文 + 代码块，流式增量
    /// 渲染。每个中间帧都与"同一状态全新渲染一次"逐行比对，检查旧 Buffer
    /// 用户报告的残留场景：markdown 正文里混排正文 + 代码块，流式增量
    /// 渲染。每个中间帧都与"同一状态全新渲染一次"逐行比对，检查旧 Buffer
    /// 内容（尤其是被裁剪的超长代码行 / 高亮 span）是否残留。
    #[test]
    fn mixed_text_code_streaming_matches_fresh_render() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        const W: u16 = 60;
        const H: u16 = 24;

        // 同一状态全新渲染一次，返回逐行字符串。
        fn fresh_grid(deltas: &[&str]) -> Vec<String> {
            let mut model = Model::new(W, H);
            model.context_usage_known = true;
            update(&mut model, Msg::NewMessage("user".into(), "please".into()));
            update(&mut model, Msg::NewMessage("assistant".into(), String::new()));
            for d in deltas {
                update(&mut model, Msg::StreamText((*d).into()));
            }
            update(&mut model, Msg::StreamEnd);
            let mut term = RatTerminal::new(TestBackend::new(W, H)).expect("backend");
            term.draw(|f| view(&mut model, f)).expect("fresh draw");
            let buf = term.backend().buffer();
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
                .collect()
        }

        // 一次真实的流式序列：正文 + fenced rust 代码块 + 超长代码行。
        let deltas = [
            "Let me show you",
            " how to use ",
            "`std::fs`.\n\n",
            "```rust\n",
            "fn main() { println!(\"this is a deliberately long line that far exceeds the terminal width and gets clipped\"); }\n",
            "let x = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];\n",
            "```\n\n",
            "Done with 中文 and *emphasis*.",
        ];

        let mut model = Model::new(W, H);
        model.context_usage_known = true;
        update(&mut model, Msg::NewMessage("user".into(), "please".into()));
        update(&mut model, Msg::NewMessage("assistant".into(), String::new()));

        let mut term = RatTerminal::new(TestBackend::new(W, H)).expect("backend");
        term.draw(|f| view(&mut model, f)).expect("draw0");

        let mut seen: Vec<&str> = Vec::new();
        for d in deltas {
            update(&mut model, Msg::StreamText(d.into()));
            seen.push(d);
            term.draw(|f| view(&mut model, f)).expect("draw");
            let live = {
                let buf = term.backend().buffer();
                (0..buf.area.height)
                    .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
                    .collect::<Vec<String>>()
            };
            let fresh = fresh_grid(&seen);
            for (y, (a, b)) in live.iter().zip(fresh.iter()).enumerate() {
                assert_eq!(a, b, "step {d:?} row {y} residue:\n live={a:?}\n fresh={b:?}");
            }
        }
    }



    /// resize 残留检查：同一模型先以 60 列渲染（正文 + 代码块 + 超长代码
    /// 行被裁剪），再以 40 列渲染——结果必须与"全新模型直接以 40 列渲染"
    /// 完全一致，不能残留 60 列布局下的旧内容。
    #[test]
    fn body_resize_repaints_clean_at_new_width() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let md = "Text before the code block.\n\n```rust\nfn main() { println!(\"a long code line that overflows the narrower terminal and gets clipped\"); }\n```\n\nDone.";
        let build = |w: u16| {
            let mut model = Model::new(w, 20);
            model.context_usage_known = true;
            update(&mut model, Msg::NewMessage("assistant".into(), md.into()));
            model
        };
        let grid = |term: &RatTerminal<TestBackend>| -> Vec<String> {
            let buf = term.backend().buffer();
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
                .collect()
        };

        // 同一个 model：先画 60 列，再画 40 列。
        let mut model = build(60);
        let mut t60 = RatTerminal::new(TestBackend::new(60, 20)).expect("backend");
        t60.draw(|f| view(&mut model, f)).expect("draw 60");
        let mut t40 = RatTerminal::new(TestBackend::new(40, 20)).expect("backend");
        t40.draw(|f| view(&mut model, f)).expect("draw 40");

        // 全新模型直接以 40 列渲染。
        let mut fresh = build(40);
        let mut tfresh = RatTerminal::new(TestBackend::new(40, 20)).expect("backend");
        tfresh.draw(|f| view(&mut fresh, f)).expect("fresh 40");

        let (a, b) = (grid(&t40), grid(&tfresh));
        for (y, (x1, x2)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(x1, x2, "resize residue at row {y}:\n live(40)={x1:?}\n fresh(40)={x2:?}");
        }
    }

    /// 剪贴板快捷键（对齐 TS）：Ctrl+C 清空编辑器（500ms 内连按两次退出）、
    /// Ctrl+D 空输入退出（非空时 delete-char-forward）、Ctrl+V 从系统剪贴板
    /// 粘贴文本、Ctrl+X 复制最后一条 assistant 消息。这些应用级快捷键只
    /// 在下栏编辑器聚焦（Chat 模式）时生效——对话框/选择器各自处理自己的键。
    #[test]
    fn clipboard_shortcuts_match_ts() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);

        let mut model = Model::new(100, 30);
        model.input.set_value("hello");
        // Ctrl+C 第一次：清空编辑器、关闭补全弹窗，不退出。
        model.completer.visible = true;
        let cmds = handle_key(&mut model, ctrl('c'));
        assert!(cmds.is_empty(), "first ctrl+c must not quit");
        assert_eq!(model.input.value(), "", "ctrl+c clears the editor");
        assert!(!model.completer.visible, "ctrl+c closes the completer (TS setText cancels autocomplete)");
        // 500ms 内再按：退出。
        let cmds = handle_key(&mut model, ctrl('c'));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Quit)), "second ctrl+c within 500ms quits");

        // Ctrl+D：空输入退出；非空输入是 delete-char-forward（删除光标后的
        // 字符）——TS CustomEditor 空编辑器才触发 app.exit，非空时交给编辑器
        // 的 deleteCharForward（默认绑定 ctrl+d）。
        let mut model = Model::new(100, 30);
        model.input.set_value("abcde");
        model.input.move_left();
        model.input.move_left(); // cursor 在 'c' 与 'd' 之间
        let cmds = handle_key(&mut model, ctrl('d'));
        assert!(cmds.is_empty(), "ctrl+d with non-empty input must not quit");
        assert_eq!(model.input.value(), "abce", "ctrl+d deletes the char after the cursor");
        model.input.clear();
        let cmds = handle_key(&mut model, ctrl('d'));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Quit)), "ctrl+d with empty input quits");

        // Ctrl+V：不插入字面 'v'，且不 panic（剪贴板读取失败时静默忽略）。
        let mut model = Model::new(100, 30);
        model.input.set_value("");
        let cmds = handle_key(&mut model, ctrl('v'));
        assert!(cmds.is_empty());
        assert_ne!(model.input.value(), "v", "ctrl+v must not insert a literal v");

        // Ctrl+X：请求宿主复制最后一条 assistant 消息（TS `app.message.copy`）。
        let mut model = Model::new(100, 30);
        let cmds = handle_key(&mut model, ctrl('x'));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::CopyLastMessage)), "ctrl+x requests copy");

        // 门控：对话框（AppMode::Editor）打开时，Ctrl+C 不清聊天输入、
        // Ctrl+D 不退出——键交给对话框的 textarea 原生处理。
        let mut model = Model::new(100, 30);
        model.input.set_value("chat");
        update(&mut model, Msg::OpenEditor("rename".into(), "dialog".into()));
        let cmds = handle_key(&mut model, ctrl('c'));
        assert!(cmds.is_empty());
        assert_eq!(model.input.value(), "chat", "ctrl+c in dialog mode leaves the chat input alone");
        let cmds = handle_key(&mut model, ctrl('d'));
        assert!(!cmds.iter().any(|c| matches!(c, Cmd::Quit)), "ctrl+d in dialog mode must not quit");
        let cmds = handle_key(&mut model, ctrl('x'));
        assert!(!cmds.iter().any(|c| matches!(c, Cmd::CopyLastMessage)), "ctrl+x in dialog mode is not app.message.copy");
    }

    /// `formatDuration` matches TS `(ms/1000).toFixed(1)`: one decimal,
    /// rounding half away from zero (1250ms → 1.25 → "1.3s", while Rust's
    /// default `{:.1}` rounds half to even and would give "1.2s").
    #[test]
    fn format_duration_matches_ts_tofixed() {
        assert_eq!(format_duration(0), "0.0s");
        assert_eq!(format_duration(1250), "1.3s");
        assert_eq!(format_duration(1249), "1.2s");
        assert_eq!(format_duration(999), "1.0s");
    }

    /// Assistant thinking content renders below the text in thinkingText
    /// italic, and a length stop reason renders the TS notice in error
    /// color.
    #[test]
    fn assistant_renders_thinking_and_stop_reason() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;

        let mut model = Model::new(100, 30);
        model.context_usage_known = true;
        update(&mut model, Msg::NewMessage("assistant".into(), "Answer text".into()));
        update(
            &mut model,
            Msg::MessageEnd {
                thinking: "Let me think".into(),
                stop_reason: Some(StopReason::Length),
                error_message: None,
            },
        );

        let mut terminal = RatTerminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|frame| view(&mut model, frame)).expect("draw");
        let buf = terminal.backend().buffer();

        // Thinking FIRST (TS renders content in order — thinking before
        // text): blank separator, then thinking in thinkingText italic.
        let thinking_row = (0..24).find(|&y| buf[(0, y)].symbol() == "L").expect("thinking row");
        assert_eq!(buf[(0, thinking_row)].fg, theme::THINKING_TEXT, "thinking color");
        assert!(buf[(0, thinking_row)].modifier.contains(Modifier::ITALIC), "thinking italic");
        // Trailing blank, then the text row.
        let text_row = (0..24).find(|&y| buf[(0, y)].symbol() == "A").expect("text row");
        assert!(text_row > thinking_row, "text below thinking");
        // Blank separator, then the length notice in error color.
        let notice_row = text_row + 2;
        assert_eq!(buf[(0, notice_row)].symbol(), "R", "notice text");
        assert_eq!(buf[(0, notice_row)].fg, theme::ERROR, "notice error color");
    }

    /// 内部 scrollback：内容超过视口时默认跟随底部（显示最新内容）；
    /// ScrollUp 后显示更早内容（视口上移）；继续 ScrollUp 到底后不再
    /// 变化；滚回底部恢复跟随；新消息在跟随态下追加到可见底部。
    #[test]
    fn internal_scrollback_follows_bottom_and_scrolls_up() {
        let mut model = Model::new(60, 12);
        // 造 20 条消息，让内容高度远超视口。
        for i in 0..20 {
            update(&mut model, Msg::NewMessage("assistant".into(), format!("line-{i:02}")));
        }
        // 强制低高度视口（6 行正文区）。
        // 跟随底部：可见窗口应显示最新内容（line-19），看不到最早消息。
        let text = render_text(&mut model, 60, 12);
        assert!(text.contains("line-19"), "follow bottom shows newest: {text:?}");
        assert!(!text.contains("line-00"), "follow bottom hides oldest: {text:?}");

        // 向上滚 6 行：line-13 应可见（line-19 被推出视口外）。
        update(&mut model, Msg::ScrollUp(6));
        eprintln!("after ScrollUp(6): offset={} auto={}", model.scroll_offset, model.auto_scroll);
        let text = render_text(&mut model, 60, 12);
        eprintln!("scrolled text:\n{text}");
        assert!(text.contains("line-13"), "scroll up shows earlier: {text:?}");
        assert!(!text.contains("line-19"), "scroll up hides newest: {text:?}");

        // 继续向上滚到顶：最早内容（header）出现，最新内容被推出。
        update(&mut model, Msg::ScrollUp(100));
        let text = render_text(&mut model, 60, 12);
        assert!(
            text.contains("Pi v1.83"),
            "scroll to top shows oldest (header): {text:?}"
        );
        assert!(!text.contains("line-19"), "scroll to top hides newest: {text:?}");

        // 向下滚回底部：恢复跟随（overscroll re-engage）。
        update(&mut model, Msg::ScrollDown(100));
        let text = render_text(&mut model, 60, 12);
        assert!(text.contains("line-19"), "scroll down re-engages follow: {text:?}");

        // 跟随态下新消息追加到可见底部。
        update(&mut model, Msg::NewMessage("assistant".into(), "line-20".into()));
        let text = render_text(&mut model, 60, 12);
        assert!(text.contains("line-20"), "follow appends new content: {text:?}");
    }

    /// 内容不足一屏时：不滚动，全部可见。
    #[test]
    fn internal_scrollback_fits_viewport_shows_all() {
        let mut model = Model::new(60, 12);
        for i in 0..3 {
            update(&mut model, Msg::NewMessage("assistant".into(), format!("msg-{i}")));
        }
        let text = render_text(&mut model, 60, 12);
        for i in 0..3 {
            assert!(text.contains(&format!("msg-{i}")), "msg-{i} visible: {text:?}");
        }
    }

    /// 渲染辅助：TestBackend 画当前视图，返回非空行拼接文本。
    fn render_text(model: &mut Model, w: u16, h: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal as RatTerminal;
        let mut terminal = RatTerminal::new(TestBackend::new(w, h)).expect("backend");
        terminal.draw(|frame| view(model, frame)).expect("draw");
        let buf = terminal.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 构造一个最小 BlockView（默认字段为空）。
    fn bv(role: &'static str) -> BlockView {
        BlockView {
            role,
            md_lines: Vec::new(),
            plain_lines: Vec::new(),
            tool_name: String::new(),
            tool_args: String::new(),
            cwd: String::new(),
            tool_state: None,
            tool_output: String::new(),
            tool_truncation: None,
            tool_started_at: None,
            tool_ended_at: None,
            thinking_lines: Vec::new(),
            stop_reason: None,
            error_message: None,
        }
    }

    /// 对齐 TS：上一轮 assistant 结束与新一轮 user 开始之间必须有一个空行。
    /// 这个空行是 user 块的前置空行（仅当它紧跟一个 assistant 块时），
    /// 而当前逻辑原有的 trailing gap 只把空行放在 user 块之后，导致轮次边界无分隔。
    #[test]
    fn block_gaps_blank_at_turn_boundary() {
        // 一次正常对话的块顺序：user -> assistant -> user(新轮) -> assistant。
        let blocks = [bv("user"), bv("assistant"), bv("user"), bv("assistant")];
        let (gaps, leads) = block_gaps(&blocks);
        // trailing gaps：每个非 assistant 块（user）之后一个空行（分隔 box 与其后的回复）。
        assert_eq!(gaps, vec![1, 0, 1, 0]);
        // leading gaps：只有紧跟 assistant 的 user 块（新轮次开头）获得前置空行。
        assert_eq!(leads, vec![0, 0, 1, 0]);
    }

    /// 首个 user 块（前面没有 assistant）不应获得轮次边界空行。
    #[test]
    fn block_gaps_first_user_has_no_leading_blank() {
        let blocks = [bv("user"), bv("assistant")];
        let (_gaps, leads) = block_gaps(&blocks);
        assert_eq!(leads, vec![0, 0]);
    }
}
