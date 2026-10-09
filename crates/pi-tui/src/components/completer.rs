//! Autocomplete engine for `/` commands, `/cmd` arguments and `@` file paths.
//!
//! 对齐 TS `packages/tui/src/autocomplete.ts` + `editor.ts` 的补全语义：
//! - 候选由宿主异步计算（slash 命令 fuzzy / 命令参数 / `@` 文件走查），通过
//!   `Msg::CompletionResults` 回填；`request_seq` 丢弃过期结果（等价 TS 的
//!   AbortController + requestId）。
//! - 弹窗渲染对齐 TS `SelectList`（`→ ` 前缀、描述列、`(n/m)` 滚动、
//!   `No matching commands`、`max_visible` 可配）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// What triggered the current completion popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionTrigger {
    /// `/command` 命令名补全。
    Slash,
    /// `/command <arg>` 命令参数补全。
    Argument,
    /// `@` 附件文件补全。
    At,
}

/// A single completion candidate.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// 应用时插入的文本（替换掉 `prefix`）。
    pub value: String,
    /// 列表里显示的 label。
    pub label: String,
    pub description: String,
}

impl CompletionItem {
    pub fn new(value: impl Into<String>, label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.into(),
        }
    }
}

/// 命令参数补全回调（对齐 TS `getArgumentCompletions`）。
pub type ArgumentCompletionsFn = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Option<Vec<CompletionItem>>> + Send>> + Send + Sync,
>;

/// 补全菜单里的一个命令。字段对齐 TS `SlashCommand`（`autocomplete.ts`）：
/// `name` 既是插入文本又是展示 label，`description` 为原始描述，`argumentHint`
/// 在展示时拼到描述前面（`hint — desc`）。
#[derive(Clone)]
pub struct CompletionCommand {
    /// 命令名（不带 `/`），既是插入文本也是菜单 label（TS `name`）。
    pub name: String,
    /// 原始描述（不含 argumentHint），对齐 TS `description`。
    pub description: String,
    /// 参数提示（如 `<provider/model>`），对齐 TS `argumentHint`。
    pub argument_hint: Option<String>,
    /// 参数补全（`/cmd <arg>` 上下文），对齐 TS `getArgumentCompletions`。
    pub argument_completions: Option<ArgumentCompletionsFn>,
}

impl CompletionCommand {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            argument_hint: None,
            argument_completions: None,
        }
    }

    pub fn with_argument_hint(mut self, hint: impl Into<String>) -> Self {
        self.argument_hint = Some(hint.into());
        self
    }

    pub fn with_argument_completions(mut self, f: ArgumentCompletionsFn) -> Self {
        self.argument_completions = Some(f);
        self
    }

    /// 菜单 label（TS `label: name`）。
    pub fn display_label(&self) -> &str {
        &self.name
    }

    /// 菜单描述（TS `fullDesc = hint ? (desc ? `${hint} — ${desc}` : hint) : desc`）。
    pub fn display_description(&self) -> String {
        match (&self.argument_hint, self.description.is_empty()) {
            (Some(hint), false) => format!("{hint} — {}", self.description),
            (Some(hint), true) => hint.clone(),
            (None, _) => self.description.clone(),
        }
    }
}

/// A completion request the host should resolve asynchronously.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionRequest {
    pub seq: u64,
    pub trigger: CompletionTrigger,
    /// 被替换的完整前缀（如 `/mod`、`/model ma`、`@src/ma`）。
    pub prefix: String,
    /// 查询串。
    pub query: String,
    /// `Argument` 补全时的命令名（用于找到参数补全回调）。
    pub command: Option<String>,
    /// 附件（`@`/路径）补全的 debounce；slash/参数为 0。
    pub debounce_ms: u64,
    /// Tab 强制路径补全（对齐 TS `force`）。
    pub force: bool,
}

/// Autocomplete engine state.
pub struct Completer {
    pub commands: Vec<CompletionCommand>,
    /// 当前候选列表（宿主异步计算后回填）。
    pub results: Vec<CompletionItem>,
    pub trigger: Option<CompletionTrigger>,
    /// 被替换的前缀（TS `autocompletePrefix`）。
    pub prefix: String,
    pub query: String,
    pub selected: usize,
    pub visible: bool,
    /// TS `autocompleteMaxVisible`（默认 5，clamp 3..=20）。
    pub max_visible: usize,
    /// 最新一次补全请求的序号（丢弃过期结果）。
    pub request_seq: u64,
    /// 当前 `results` 对应的 query（apply_results 时记录；用于判断结果是否
    /// 与正在输入的文本一致——避免 Enter/Tab 应用到过期结果，等价 TS
    /// 串行请求管线 + `isStale` 语义）。
    pub results_query: String,
    /// TS `SelectListLayoutOptions` 主列宽下/上限（`SLASH_COMMAND_SELECT_LIST_LAYOUT`
    /// 为 12/32；路径/参数列表用默认 32/32）。由 `begin` 根据前缀是否以
    /// `/` 开头选定。
    pub primary_min: usize,
    pub primary_max: usize,
}

impl Completer {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            results: Vec::new(),
            trigger: None,
            prefix: String::new(),
            query: String::new(),
            selected: 0,
            visible: false,
            max_visible: 5,
            request_seq: 0,
            results_query: String::new(),
            primary_min: 32,
            primary_max: 32,
        }
    }

    pub fn set_commands(&mut self, cmds: Vec<CompletionCommand>) {
        self.commands = cmds;
    }

    pub fn set_max_visible(&mut self, max_visible: usize) {
        self.max_visible = max_visible.clamp(3, 20);
    }

    /// 开始一次补全（宿主已判定该触发）：记录触发/前缀，序号 +1。
    pub fn begin(&mut self, trigger: CompletionTrigger, prefix: &str, query: &str) {
        self.trigger = Some(trigger);
        self.prefix = prefix.to_string();
        self.query = query.to_string();
        self.selected = 0;
        self.visible = true;
        self.request_seq += 1;
        // TS `createAutocompleteList`: 以 `/` 开头的前缀（命令名与命令参数）
        // 用 `SLASH_COMMAND_SELECT_LIST_LAYOUT`（主列 12..32），其余
        // （路径/附件）用默认 32..32。
        if prefix.starts_with('/') {
            self.primary_min = 12;
            self.primary_max = 32;
        } else {
            self.primary_min = 32;
            self.primary_max = 32;
        }
    }

    /// 回填候选；仅当 `seq` 是最新请求（未过期）时生效。
    pub fn apply_results(&mut self, seq: u64, items: Vec<CompletionItem>) {
        if seq != self.request_seq {
            return;
        }
        self.results = items;
        self.selected = 0;
        self.visible = true;
        self.results_query = self.query.clone();
    }

    /// 结果是否与当前查询一致（新鲜）——应用选中项前检查，避免把过期结果
    /// 插进输入（对齐 TS 串行请求管线在 Enter 时应用的是当前文本的结果）。
    pub fn has_fresh_results(&self) -> bool {
        self.visible && !self.results.is_empty() && self.results_query == self.query
    }

    pub fn deactivate(&mut self) {
        self.visible = false;
        self.trigger = None;
        self.prefix.clear();
        self.query.clear();
        self.results.clear();
        self.results_query.clear();
        self.selected = 0;
    }

    pub fn next(&mut self) {
        if self.visible && !self.results.is_empty() {
            self.selected = (self.selected + 1) % self.results.len();
        }
    }

    pub fn prev(&mut self) {
        if self.visible && !self.results.is_empty() {
            self.selected = if self.selected == 0 { self.results.len() - 1 } else { self.selected - 1 };
        }
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        if !self.visible { return None; }
        self.results.get(self.selected)
    }

    /// 应用选中项：返回替换后的完整输入文本（不修改内部状态）。
    /// 对齐 TS `applyCompletion`：slash 命令补全后加空格；参数补全不加；
    /// `@` 文件补全非目录加空格、目录不加（继续补全）。
    pub fn apply_selected(&self, input_value: &str) -> Option<String> {
        let item = self.selected_item()?;
        Some(apply_completion(input_value, self.trigger?, &self.prefix, item))
    }

    /// Render the completion list TS-style (the original `SelectList` used
    /// for the editor autocomplete): plain inline rows — no border, no
    /// title, no background. The selected row is entirely accent (TS
    /// `selectedText` wraps prefix + label + spacing + description);
    /// unselected rows are default text with a muted description in an
    /// aligned second column; a `(n/m)` scroll row appears when the list
    /// scrolls; an open menu with no matches shows `No matching commands`.
    pub fn render_rows(&self, frame: &mut Frame, area: Rect, t: &crate::theme::Theme) {
        if !self.visible || area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let mut row = |y: u16, spans: Vec<Span<'static>>| {
            if y < area.y + area.height {
                frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, y, area.width, 1));
            }
        };

        if self.results.is_empty() {
            row(area.y, vec![Span::styled("  No matching commands", Style::new().fg(t.muted))]);
            return;
        }

        let max_visible = self.max_visible;
        let total = self.results.len();
        // TS `getPrimaryColumnWidth`: widest display value + `PRIMARY_COLUMN_GAP`
        // (2), clamped to the layout bounds (slash 12..32, path 32..32).
        let widest = self
            .results
            .iter()
            .map(|c| unicode_width::UnicodeWidthStr::width(c.label.as_str()) + 2)
            .max()
            .unwrap_or(0);
        let lo = self.primary_min.min(self.primary_max).max(1);
        let hi = self.primary_min.max(self.primary_max).max(1);
        let primary_col = widest.clamp(lo, hi);

        // Selection-centered window (TS `selected - floor(maxVisible/2)`).
        let start = self
            .selected
            .saturating_sub(max_visible / 2)
            .min(total.saturating_sub(max_visible));
        let end = (start + max_visible).min(total);

        for (i, item) in self.results.iter().enumerate().skip(start).take(end - start) {
            let selected = i == self.selected;
            let prefix = if selected { "\u{2192} " } else { "  " };
            let prefix_w = 2usize;
            let desc = normalize_single_line(&item.description);

            // TS `renderItem`: descriptions only render when the terminal is
            // wide enough (`width > 40`) and the remaining space exceeds
            // `MIN_DESCRIPTION_WIDTH` (10).
            let mut drawn = false;
            if !desc.is_empty() && width > 40 {
                let effective = primary_col.min(width.saturating_sub(prefix_w + 4)).max(1);
                let max_primary = effective.saturating_sub(2).max(1);
                let label = crate::app::truncate_to_width_suffix(&item.label, max_primary, "");
                let label_w = unicode_width::UnicodeWidthStr::width(label.as_str());
                let spacing = " ".repeat(effective.saturating_sub(label_w).max(1));
                let desc_start = prefix_w + label_w + spacing.len();
                let remaining = width.saturating_sub(desc_start).saturating_sub(2);
                if remaining > 10 {
                    let desc = crate::app::truncate_to_width_suffix(&desc, remaining, "");
                    let spans = if selected {
                        // Selected: the whole line (prefix + label + spacing +
                        // description) is accent (TS `selectedText`).
                        vec![Span::styled(
                            format!("{prefix}{label}{spacing}{desc}"),
                            Style::new().fg(t.accent),
                        )]
                    } else {
                        vec![
                            Span::styled(format!("{prefix}{label}"), Style::new().fg(t.text)),
                            Span::styled(format!("{spacing}{desc}"), Style::new().fg(t.muted)),
                        ]
                    };
                    row(area.y + (i - start) as u16, spans);
                    drawn = true;
                }
            }

            if !drawn {
                // No description: truncate the label to the row (TS
                // `truncatePrimary` with no ellipsis).
                let max_width = width.saturating_sub(prefix_w + 2);
                let label = crate::app::truncate_to_width_suffix(&item.label, max_width, "");
                let text = format!("{prefix}{label}");
                let style = Style::new().fg(if selected { t.accent } else { t.text });
                row(area.y + (i - start) as u16, vec![Span::styled(text, style)]);
            }
        }

        // Scroll indicator (TS `  (n/m)` in muted, truncated to width - 2).
        if start > 0 || end < total {
            let info = format!("  ({}/{})", self.selected + 1, total);
            let info = crate::app::truncate_to_width_suffix(&info, width.saturating_sub(2), "");
            row(area.y + (end - start) as u16, vec![Span::styled(info, Style::new().fg(t.muted))]);
        }
    }
}

/// TS `normalizeToSingleLine`: collapse runs of `\r`/`\n` to a single space
/// and trim (descriptions are rendered on one row).
fn normalize_single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_break = false;
    for ch in text.chars() {
        if ch == '\r' || ch == '\n' {
            if !in_break {
                out.push(' ');
            }
            in_break = true;
        } else {
            in_break = false;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

/// 应用选中项（对齐 TS `applyCompletion`）。
pub fn apply_completion(input_value: &str, trigger: CompletionTrigger, prefix: &str, item: &CompletionItem) -> String {
    let prefix_len = prefix.chars().count();
    let total_chars: Vec<char> = input_value.chars().collect();
    let cut = total_chars.len().saturating_sub(prefix_len);
    let before: String = total_chars[..cut].iter().collect();
    match trigger {
        CompletionTrigger::Slash => {
            format!("{before}/{} ", item.value)
        }
        CompletionTrigger::Argument => {
            format!("{before}{}", item.value)
        }
        CompletionTrigger::At => {
            // 目录不加空格（继续补全）；文件加空格（TS @ 分支）。
            let suffix = if item.label.ends_with('/') { "" } else { " " };
            format!("{before}{}{suffix}", item.value)
        }
    }
}

impl Default for Completer {
    fn default() -> Self { Self::new() }
}
