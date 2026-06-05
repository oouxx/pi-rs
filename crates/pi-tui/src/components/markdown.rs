use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::tui::Component;
use crate::utils::{apply_background_to_line, visible_width, wrap_text_with_ansi};

pub struct DefaultTextStyle {
    pub color: Option<Box<dyn Fn(&str) -> String + Send + Sync>>,
    pub bg_color: Option<Box<dyn Fn(&str) -> String + Send + Sync>>,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
}

pub struct MarkdownTheme {
    pub heading: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub link: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub link_url: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub code: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub code_block: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub code_block_border: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub quote: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub quote_border: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub hr: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub list_bullet: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub bold: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub italic: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub strikethrough: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub underline: Box<dyn Fn(&str) -> String + Send + Sync>,
    pub highlight_code: Option<Box<dyn Fn(&str, Option<&str>) -> Vec<String> + Send + Sync>>,
    pub code_block_indent: String,
}

impl MarkdownTheme {
    pub fn default_theme() -> Self {
        let reset = "\x1b[0m";
        Self {
            heading: Box::new(move |t| format!("\x1b[1;94m{}{}", t, reset)),
            link: Box::new(move |t| format!("\x1b[34m{}{}", t, reset)),
            link_url: Box::new(move |t| format!("\x1b[2m{}{}", t, reset)),
            code: Box::new(move |t| format!("\x1b[33m{}{}", t, reset)),
            code_block: Box::new(move |t| format!("\x1b[33m{}{}", t, reset)),
            code_block_border: Box::new(move |t| format!("\x1b[2;33m{}{}", t, reset)),
            quote: Box::new(move |t| format!("\x1b[3m{}{}", t, reset)),
            quote_border: Box::new(move |t| format!("\x1b[2m{}{}", t, reset)),
            hr: Box::new(move |t| format!("\x1b[2m{}{}", t, reset)),
            list_bullet: Box::new(move |t| format!("\x1b[36m{}{}", t, reset)),
            bold: Box::new(move |t| format!("\x1b[1m{}{}", t, reset)),
            italic: Box::new(move |t| format!("\x1b[3m{}{}", t, reset)),
            strikethrough: Box::new(move |t| format!("\x1b[9m{}{}", t, reset)),
            underline: Box::new(move |t| format!("\x1b[4m{}{}", t, reset)),
            highlight_code: None,
            code_block_indent: "  ".to_string(),
        }
    }
}

pub struct MarkdownOptions {
    pub preserve_ordered_list_markers: bool,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            preserve_ordered_list_markers: false,
        }
    }
}

fn default_text_prefix(dts: &DefaultTextStyle, theme: &MarkdownTheme) -> String {
    let sentinel = "\x00";
    let mut styled = sentinel.to_string();
    if let Some(ref color) = dts.color {
        styled = color(&styled);
    }
    if dts.bold {
        styled = (theme.bold)(&styled);
    }
    if dts.italic {
        styled = (theme.italic)(&styled);
    }
    if dts.strikethrough {
        styled = (theme.strikethrough)(&styled);
    }
    if dts.underline {
        styled = (theme.underline)(&styled);
    }
    if let Some(pos) = styled.find('\x00') {
        styled[..pos].to_string()
    } else {
        String::new()
    }
}

fn extract_ansi_prefix(styled: &str) -> String {
    let mut pos = 0;
    let bytes = styled.as_bytes();
    while pos < bytes.len() {
        if bytes[pos] == b'\x1b' {
            pos += 1;
            while pos < bytes.len() && !bytes[pos].is_ascii_alphabetic() && bytes[pos] != b'~' {
                pos += 1;
            }
            if pos < bytes.len() {
                pos += 1;
            }
        } else if bytes[pos] == b'\x00' {
            break;
        } else {
            break;
        }
    }
    styled[..pos].to_string()
}

fn collect_text(events: &[Event]) -> String {
    let mut result = String::new();
    for ev in events {
        match ev {
            Event::Text(t) => result.push_str(t),
            Event::Code(t) => result.push_str(t),
            Event::SoftBreak | Event::HardBreak => result.push('\n'),
            _ => {}
        }
    }
    result
}

fn render_inline(events: &[Event], theme: &MarkdownTheme, default_prefix: &str) -> String {
    let mut result = String::new();
    let mut i = 0;
    while i < events.len() {
        let ev = &events[i];
        match ev {
            Event::Text(t) => {
                result.push_str(&format!("{}{}\x1b[0m", default_prefix, t));
                i += 1;
            }
            Event::Code(t) => {
                result.push_str(&(theme.code)(t));
                result.push_str(default_prefix);
                i += 1;
            }
            Event::SoftBreak | Event::HardBreak => {
                result.push(' ');
                i += 1;
            }
            Event::Start(Tag::Strong) => {
                let (end, inner) = find_inner(events, i + 1);
                let inner_text = render_inline(inner, theme, default_prefix);
                result.push_str(&(theme.bold)(&inner_text));
                result.push_str(default_prefix);
                i = end;
            }
            Event::Start(Tag::Emphasis) => {
                let (end, inner) = find_inner(events, i + 1);
                let inner_text = render_inline(inner, theme, default_prefix);
                result.push_str(&(theme.italic)(&inner_text));
                result.push_str(default_prefix);
                i = end;
            }
            Event::Start(Tag::Strikethrough) => {
                let (end, inner) = find_inner(events, i + 1);
                let inner_text = render_inline(inner, theme, default_prefix);
                result.push_str(&(theme.strikethrough)(&inner_text));
                result.push_str(default_prefix);
                i = end;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let (end, inner) = find_inner(events, i + 1);
                let link_text = render_inline(inner, theme, default_prefix);
                let plain_text = collect_text(inner).trim().to_string();
                let href = dest_url.to_string();
                let href_for_comparison = if href.starts_with("mailto:") {
                    href[7..].to_string()
                } else {
                    href.clone()
                };
                let styled_link = (theme.link)(&(theme.underline)(&link_text));
                if plain_text == href || plain_text == href_for_comparison {
                    result.push_str(&styled_link);
                } else {
                    result.push_str(&styled_link);
                    result.push_str(&(theme.link_url)(&format!(" ({})", href)));
                }
                result.push_str(default_prefix);
                i = end;
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Strong)
            | Event::End(TagEnd::Emphasis) | Event::End(TagEnd::Strikethrough) => {
                result.push_str(default_prefix);
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    result
}

fn render_inline_styled(
    events: &[Event],
    theme: &MarkdownTheme,
    style_fn: &dyn Fn(&str) -> String,
    prefix: &str,
) -> String {
    let mut result = String::new();
    let mut i = 0;

    while i < events.len() {
        let ev = &events[i];
        match ev {
            Event::Text(t) => {
                result.push_str(&style_fn(t));
                result.push_str(prefix);
                i += 1;
            }
            Event::Code(t) => {
                result.push_str(&(theme.code)(t));
                result.push_str(prefix);
                i += 1;
            }
            Event::SoftBreak | Event::HardBreak => {
                result.push(' ');
                i += 1;
            }
            Event::Start(Tag::Strong) => {
                let (end, inner) = find_inner(events, i + 1);
                let inner_text = render_inline_styled(inner, theme, style_fn, prefix);
                result.push_str(&(theme.bold)(&inner_text));
                result.push_str(prefix);
                i = end;
            }
            Event::Start(Tag::Emphasis) => {
                let (end, inner) = find_inner(events, i + 1);
                let inner_text = render_inline_styled(inner, theme, style_fn, prefix);
                result.push_str(&(theme.italic)(&inner_text));
                result.push_str(prefix);
                i = end;
            }
            Event::Start(Tag::Strikethrough) => {
                let (end, inner) = find_inner(events, i + 1);
                let inner_text = render_inline_styled(inner, theme, style_fn, prefix);
                result.push_str(&(theme.strikethrough)(&inner_text));
                result.push_str(prefix);
                i = end;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let (end, inner) = find_inner(events, i + 1);
                let link_text = render_inline_styled(inner, theme, style_fn, prefix);
                let plain_text = collect_text(inner).trim().to_string();
                let href = dest_url.to_string();
                let href_for_comparison = if href.starts_with("mailto:") {
                    href[7..].to_string()
                } else {
                    href.clone()
                };
                let styled_link = (theme.link)(&(theme.underline)(&link_text));
                if plain_text == href || plain_text == href_for_comparison {
                    result.push_str(&styled_link);
                } else {
                    result.push_str(&styled_link);
                    result.push_str(&(theme.link_url)(&format!(" ({})", href)));
                }
                result.push_str(prefix);
                i = end;
            }
            Event::End(TagEnd::Link) | Event::End(TagEnd::Strong)
            | Event::End(TagEnd::Emphasis) | Event::End(TagEnd::Strikethrough) => {
                result.push_str(prefix);
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    result
}

pub fn is_image_line(s: &str) -> bool {
    s.contains("\x1b_pi:img")
}

pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<Box<DefaultTextStyle>>,
    theme: MarkdownTheme,
    options: MarkdownOptions,
    cached_text: Option<String>,
    cached_width: Option<u16>,
    cached_lines: Option<Vec<String>>,
}

impl Markdown {
    pub fn new(
        text: String,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<Box<DefaultTextStyle>>,
        options: Option<MarkdownOptions>,
    ) -> Self {
        Self {
            text,
            padding_x,
            padding_y,
            theme,
            default_text_style,
            options: options.unwrap_or_default(),
            cached_text: None,
            cached_width: None,
            cached_lines: None,
        }
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }

    fn default_prefix(&self) -> String {
        match self.default_text_style {
            Some(ref dts) => default_text_prefix(dts, &self.theme),
            None => String::new(),
        }
    }

    fn is_next_block(events: &[Event], pos: usize) -> bool {
        if pos >= events.len() {
            return false;
        }
        match &events[pos] {
            Event::Start(tag) => matches!(
                tag,
                Tag::Paragraph
                    | Tag::Heading { .. }
                    | Tag::CodeBlock(_)
                    | Tag::BlockQuote(_)
                    | Tag::List(_)
                    | Tag::Item
                    | Tag::Table(_)
                    | Tag::HtmlBlock
                    | Tag::DefinitionList
                    | Tag::DefinitionListTitle
                    | Tag::DefinitionListDefinition
            ),
            Event::Rule | Event::Html(_) => true,
            Event::Text(t) => !t.is_empty(),
            _ => false,
        }
    }

    fn render_events(&self, events: &[Event], content_width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut i = 0;
        let n = events.len();
        let prefix = self.default_prefix();

        while i < n {
            match &events[i] {
                Event::Start(tag) => {
                    let (end, inner) = find_inner(events, i + 1);
                    let tag_clone = tag.clone();
                    let mut block_lines = self.render_block(&tag_clone, inner, content_width, &prefix);
                    lines.append(&mut block_lines);
                    i = end;
                    if Self::is_next_block(events, i) {
                        lines.push(String::new());
                    }
                }
                Event::Text(t) => {
                    if !t.is_empty() {
                        lines.push(format!("{}{}\x1b[0m", prefix, t));
                        let next = i + 1;
                        if next < n && Self::is_next_block(events, next) {
                            lines.push(String::new());
                        }
                    }
                    i += 1;
                }
                Event::Code(t) => {
                    lines.push((self.theme.code)(t));
                    let next = i + 1;
                    if next < n && Self::is_next_block(events, next) {
                        lines.push(String::new());
                    }
                    i += 1;
                }
                Event::Rule => {
                    lines.push((self.theme.hr)(&"─".repeat(content_width.min(80))));
                    let next = i + 1;
                    if next < n && Self::is_next_block(events, next) {
                        lines.push(String::new());
                    }
                    i += 1;
                }
                Event::SoftBreak | Event::HardBreak => {
                    i += 1;
                }
                Event::Html(t) => {
                    lines.push(format!("{}{}\x1b[0m", prefix, t));
                    let next = i + 1;
                    if next < n && Self::is_next_block(events, next) {
                        lines.push(String::new());
                    }
                    i += 1;
                }
                Event::End(_) => {
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        }
        lines
    }

    fn render_block(
        &self,
        tag: &Tag,
        inner: &[Event],
        content_width: usize,
        prefix: &str,
    ) -> Vec<String> {
        match tag {
            Tag::Paragraph => {
                let text = render_inline(inner, &self.theme, prefix);
                if text.is_empty() {
                    vec![]
                } else {
                    vec![text]
                }
            }
            Tag::Heading { level, .. } => {
                let text = render_inline_styled(inner, &self.theme, &|s| {
                    if *level == HeadingLevel::H1 {
                        (self
                            .theme
                            .heading)(&(self.theme.bold)(&(self.theme.underline)(s)))
                    } else {
                        (self.theme.heading)(&(self.theme.bold)(s))
                    }
                }, prefix);
                let prefix_str = format!("{} ", "#".repeat(*level as usize));
                let styled = if *level >= HeadingLevel::H3 {
                    format!(
                        "{}{}",
                        (self.theme.heading)(&(self.theme.bold)(&prefix_str)),
                        text
                    )
                } else {
                    text
                };
                vec![styled]
            }
            Tag::BlockQuote(_) => {
                let inner_lines = self.render_events(inner, content_width.saturating_sub(2));
                let quote_style_prefix = {
                    let sentinel = "\x00";
                    let styled = (self.theme.quote)(&(self.theme.italic)(sentinel));
                    extract_ansi_prefix(&styled)
                };
                let mut result: Vec<String> = Vec::new();
                let mut non_empty_found = false;
                for line in inner_lines {
                    if non_empty_found || !line.trim().is_empty() {
                        non_empty_found = true;
                        for wl in wrap_text_with_ansi(&line, content_width.saturating_sub(2)) {
                            let styled = if quote_style_prefix.is_empty() {
                                (self.theme.quote)(&(self.theme.italic)(&wl))
                            } else {
                                let reapplied = wl.replace("\x1b[0m", &format!("\x1b[0m{}", quote_style_prefix));
                                (self.theme.quote)(&(self.theme.italic)(&reapplied))
                            };
                            result.push(format!(
                                "{}{}",
                                (self.theme.quote_border)("│ "),
                                styled
                            ));
                        }
                    }
                }
                while result.last().map_or(false, |l| l.trim().is_empty()) {
                    result.pop();
                }
                result
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        let s = l.to_string();
                        if s.is_empty() {
                            None
                        } else {
                            Some(s)
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                let code_text = collect_text(inner);
                let indent = &self.theme.code_block_indent;
                let header =
                    (self.theme.code_block_border)(&format!("```{}", lang.as_deref().unwrap_or("")));
                let footer = (self.theme.code_block_border)("```");
                let mut lines = vec![header];

                if let Some(ref highlight) = self.theme.highlight_code {
                    for hl in highlight(&code_text, lang.as_deref()) {
                        lines.push(format!("{}{}", indent, hl));
                    }
                } else {
                    for cl in code_text.split('\n') {
                        lines.push(format!("{}{}", indent, (self.theme.code_block)(cl)));
                    }
                }
                lines.push(footer);
                lines
            }
            Tag::List(start_number) => {
                let items = collect_list_items(inner);
                let mut result: Vec<String> = Vec::new();
                for (idx, item_events) in items.iter().enumerate() {
                    let marker = if start_number.is_some() {
                        let num = start_number.unwrap_or(1) + idx as u64;
                        format!("{}. ", num)
                    } else {
                        "- ".to_string()
                    };
                    let bullet = (self.theme.list_bullet)(&marker);
                    let bw = visible_width(&marker);
                    let item_width = content_width.saturating_sub(bw);
                    let inner_lines = self.render_events(item_events, item_width);
                    for (j, line) in inner_lines.iter().enumerate() {
                        for (k, wl) in wrap_text_with_ansi(line, item_width).iter().enumerate() {
                            if j == 0 && k == 0 {
                                result.push(format!("{}{}", bullet, wl));
                            } else {
                                result.push(format!("{}{}", " ".repeat(bw), wl));
                            }
                        }
                    }
                }
                result
            }
            Tag::Item => vec![],
            Tag::Table(alignments) => {
                self.render_table(inner, content_width, alignments, prefix)
            }
            _ => {
                let text = collect_text(inner);
                if text.is_empty() {
                    vec![]
                } else {
                    vec![format!("{}{}\x1b[0m", prefix, text)]
                }
            }
        }
    }

    fn render_table(
        &self,
        events: &[Event],
        available_width: usize,
        _alignments: &[Alignment],
        prefix: &str,
    ) -> Vec<String> {
        let (headers, rows) = collect_table_cells(events);
        let num_cols = headers.len();
        if num_cols == 0 {
            return vec![];
        }

        let border_overhead = 3 * num_cols + 1;
        if available_width <= border_overhead {
            return vec![];
        }
        let available_for_cells = available_width - border_overhead;

        let max_unbroken_word_width = 30;

        let mut natural_widths: Vec<usize> = Vec::with_capacity(num_cols);
        let mut min_word_widths: Vec<usize> = Vec::with_capacity(num_cols);

        for cell_events in &headers {
            let text = render_inline(cell_events, &self.theme, prefix);
            natural_widths.push(visible_width(&text));
            min_word_widths.push(longest_word_width(&text, max_unbroken_word_width).max(1));
        }
        for row in &rows {
            for (i, cell_events) in row.iter().enumerate() {
                if i >= num_cols {
                    break;
                }
                let text = render_inline(cell_events, &self.theme, prefix);
                natural_widths[i] = natural_widths[i].max(visible_width(&text));
                min_word_widths[i] = min_word_widths[i]
                    .max(longest_word_width(&text, max_unbroken_word_width).max(1));
            }
        }

        let mut column_widths = distribute_table_widths(
            &natural_widths,
            &min_word_widths,
            available_for_cells,
        );

        for w in &mut column_widths {
            *w = (*w).max(1);
        }

        let mut lines: Vec<String> = Vec::new();

        let top_border: String = {
            let cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
            format!("┌─{}─┐", cells.join("─┬─"))
        };
        lines.push(top_border);

        let header_cell_lines: Vec<Vec<String>> = headers
            .iter()
            .enumerate()
            .map(|(col_idx, cell_events)| {
                let w = column_widths.get(col_idx).copied().unwrap_or(1).max(1);
                let text = render_inline(cell_events, &self.theme, prefix);
                wrap_text_with_ansi(&text, w)
            })
            .collect();
        let header_line_count = header_cell_lines
            .iter()
            .map(|c| c.len())
            .max()
            .unwrap_or(1);

        for line_idx in 0..header_line_count {
            let row_parts: Vec<String> = header_cell_lines
                .iter()
                .enumerate()
                .map(|(col_idx, cell_lines)| {
                    let text = cell_lines
                        .get(line_idx)
                        .map_or_else(String::new, |s| s.to_string());
                    let pad = column_widths[col_idx].saturating_sub(visible_width(&text));
                    (self.theme.bold)(&format!("{}{}", text, " ".repeat(pad)))
                })
                .collect();
            lines.push(format!("│ {} │", row_parts.join(" │ ")));
        }

        let sep_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        lines.push(format!("├─{}─┤", sep_cells.join("─┼─")));

        for row in &rows {
            let row_cell_lines: Vec<Vec<String>> = row
                .iter()
                .enumerate()
                .map(|(col_idx, cell_events)| {
                    let w = column_widths.get(col_idx).copied().unwrap_or(1).max(1);
                    let text = render_inline(cell_events, &self.theme, prefix);
                    wrap_text_with_ansi(&text, w)
                })
                .collect();
            let row_line_count = row_cell_lines.iter().map(|c| c.len()).max().unwrap_or(1);

            for line_idx in 0..row_line_count {
                let row_parts: Vec<String> = row_cell_lines
                    .iter()
                    .enumerate()
                    .map(|(col_idx, cell_lines)| {
                        let text = cell_lines
                            .get(line_idx)
                            .map_or_else(String::new, |s| s.to_string());
                        let pad = column_widths[col_idx].saturating_sub(visible_width(&text));
                        format!("{}{}", text, " ".repeat(pad))
                    })
                    .collect();
                lines.push(format!("│ {} │", row_parts.join(" │ ")));
            }
        }

        let bottom_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        lines.push(format!("└─{}─┘", bottom_cells.join("─┴─")));

        lines
    }

    fn content_width(&self, total_width: usize) -> usize {
        total_width.saturating_sub(self.padding_x * 2).max(1)
    }
}

fn find_inner<'a>(events: &'a [Event<'a>], start: usize) -> (usize, &'a [Event<'a>]) {
    let mut depth = 1;
    let mut end = start;
    while end < events.len() && depth > 0 {
        match &events[end] {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        if depth > 0 {
            end += 1;
        }
    }
    (end + 1, &events[start..end])
}

fn collect_list_items<'a>(events: &'a [Event<'a>]) -> Vec<Vec<Event<'a>>> {
    let mut items: Vec<Vec<Event>> = Vec::new();
    let mut current: Vec<Event> = Vec::new();
    let mut depth = 0;
    let mut in_item = false;

    for ev in events {
        match ev {
            Event::Start(Tag::Item) => {
                if in_item {
                    current.push(ev.clone());
                }
                depth += 1;
                in_item = true;
            }
            Event::End(TagEnd::Item) => {
                depth -= 1;
                if depth == 0 {
                    if !current.is_empty() {
                        items.push(current);
                    }
                    current = Vec::new();
                    in_item = false;
                } else {
                    current.push(ev.clone());
                }
            }
            _ => {
                if in_item {
                    current.push(ev.clone());
                }
            }
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
}

fn collect_table_cells<'a>(events: &'a [Event<'a>]) -> (Vec<&'a [Event<'a>]>, Vec<Vec<&'a [Event<'a>]>>) {
    let mut headers: Vec<&[Event]> = Vec::new();
    let mut rows: Vec<Vec<&[Event]>> = Vec::new();
    let mut current_row: Vec<&[Event]> = Vec::new();
    let mut in_header = false;
    let mut i = 0;

    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::TableHead) => {
                in_header = true;
                i += 1;
            }
            Event::End(TagEnd::TableHead) => {
                in_header = false;
                i += 1;
            }
            Event::Start(Tag::TableRow) => {
                current_row = Vec::new();
                i += 1;
            }
            Event::End(TagEnd::TableRow) => {
                rows.push(std::mem::take(&mut current_row));
                i += 1;
            }
            Event::Start(Tag::TableCell) => {
                let (end, inner) = find_inner(events, i + 1);
                if in_header {
                    headers.push(inner);
                } else {
                    current_row.push(inner);
                }
                i = end;
            }
            _ => {
                i += 1;
            }
        }
    }

    (headers, rows)
}

fn longest_word_width(text: &str, max_width: usize) -> usize {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut longest = 0;
    for word in words {
        let w = visible_width(word);
        longest = longest.max(w.min(max_width));
    }
    longest
}

fn distribute_table_widths(
    natural_widths: &[usize],
    min_word_widths: &[usize],
    available: usize,
) -> Vec<usize> {
    let num_cols = natural_widths.len();
    if num_cols == 0 {
        return vec![];
    }

    let total_natural: usize = natural_widths.iter().sum();
    let total_min: usize = min_word_widths.iter().sum();
    let min_spread = min_word_widths.to_vec();

    if total_natural <= available {
        return natural_widths
            .iter()
            .enumerate()
            .map(|(i, &nw)| nw.max(min_spread[i]))
            .collect();
    }

    let mut result = if total_min <= available {
        let extra = available - total_min;
        let total_grow: usize = natural_widths
            .iter()
            .enumerate()
            .map(|(i, &nw)| nw.saturating_sub(min_spread[i]))
            .sum();
        let mut widths: Vec<usize> = min_spread
            .iter()
            .enumerate()
            .map(|(i, &mw)| {
                if total_grow > 0 {
                    let delta = natural_widths[i].saturating_sub(mw);
                    mw + (delta * extra / total_grow)
                } else {
                    mw
                }
            })
            .collect();
        let allocated: usize = widths.iter().sum();
        let mut remaining = available.saturating_sub(allocated);
        while remaining > 0 {
            let mut grew = false;
            for i in 0..num_cols {
                if remaining == 0 {
                    break;
                }
                if widths[i] < natural_widths[i] {
                    widths[i] += 1;
                    remaining -= 1;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        widths
    } else {
        min_spread
            .iter()
            .enumerate()
            .map(|(i, &mw)| {
                if total_min > 0 {
                    mw * available / total_min
                } else {
                    mw
                }
            })
            .collect()
    };

    let sum: usize = result.iter().sum();
    if sum < available {
        let mut remaining = available - sum;
        for w in result.iter_mut() {
            if remaining == 0 {
                break;
            }
            *w += 1;
            remaining -= 1;
        }
    }

    result
}

impl Component for Markdown {
    fn render(&self, width: u16) -> Vec<String> {
        let width = width as usize;

        if let (Some(ref ct), Some(ref cw), Some(ref cl)) =
            (&self.cached_text, &self.cached_width, &self.cached_lines)
        {
            if ct == &self.text && *cw as usize == width {
                return cl.clone();
            }
        }

        let cw = self.content_width(width);
        if self.text.trim().is_empty() {
            return vec![];
        }

        let normalized = self.text.replace('\t', "   ");
        let mut opts = Options::empty();
        opts.insert(Options::ENABLE_TABLES);
        opts.insert(Options::ENABLE_STRIKETHROUGH);
        let parser = Parser::new_ext(&normalized, opts);
        let events: Vec<Event> = parser.collect();
        let rendered: Vec<String> = self.render_events(&events, cw);

        let mut wrapped: Vec<String> = Vec::new();
        for line in rendered {
            if is_image_line(&line) {
                wrapped.push(line);
            } else {
                for wl in wrap_text_with_ansi(&line, cw) {
                    wrapped.push(wl);
                }
            }
        }

        let left_pad = " ".repeat(self.padding_x);
        let right_pad = " ".repeat(self.padding_x);
        let bg_fn = self
            .default_text_style
            .as_ref()
            .and_then(|dts| dts.bg_color.as_ref());

        let mut content_lines: Vec<String> = Vec::new();
        for line in wrapped {
            if is_image_line(&line) {
                content_lines.push(line);
                continue;
            }
            let padded = format!("{}{}{}", left_pad, line, right_pad);
            if let Some(bg) = bg_fn {
                content_lines.push(apply_background_to_line(&padded, &|s| bg(s)));
            } else {
                let extra = width.saturating_sub(visible_width(&padded));
                content_lines.push(format!("{}{}", padded, " ".repeat(extra)));
            }
        }

        let empty = " ".repeat(width);
        let empty_lines: Vec<String> = (0..self.padding_y)
            .map(|_| {
                if let Some(ref bg) = bg_fn {
                    bg(&empty)
                } else {
                    empty.clone()
                }
            })
            .collect();

        let result: Vec<String> = empty_lines
            .iter()
            .chain(content_lines.iter())
            .chain(empty_lines.iter())
            .cloned()
            .collect();

        if result.is_empty() {
            vec![String::new()]
        } else {
            result
        }
    }

    fn handle_input(&mut self, _data: &str) {}

    fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_theme() -> MarkdownTheme {
        MarkdownTheme::default_theme()
    }

    #[test]
    fn test_simple_paragraph() {
        let md = Markdown::new(
            "Hello, world!".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("Hello, world!"));
    }

    #[test]
    fn test_heading_no_marker_h1() {
        let md = Markdown::new(
            "# Heading 1".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("Heading 1"));
    }

    #[test]
    fn test_heading_no_marker_h2() {
        let md = Markdown::new(
            "## Heading 2".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("Heading 2"));
    }

    #[test]
    fn test_heading_with_marker_h3() {
        let md = Markdown::new(
            "### Heading 3".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("# "));
        assert!(lines[0].contains("Heading 3"));
    }

    #[test]
    fn test_spacing_between_blocks() {
        let md = Markdown::new(
            "# Title\n\nParagraph text".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(lines.len() >= 2, "Expected at least 2 lines, got {}", lines.len());
        assert!(lines[0].contains("Title"));
        assert!(
            lines[1].trim().is_empty() || lines[1].contains("Paragraph"),
            "Expected empty line or Paragraph, got: '{:?}'",
            lines[1]
        );
    }

    #[test]
    fn test_inline_bold() {
        let md = Markdown::new(
            "Hello **world**!".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("Hello"));
        assert!(lines[0].contains("world"));
        assert!(lines[0].contains("\x1b[1m"));
    }

    #[test]
    fn test_inline_code() {
        let md = Markdown::new(
            "Use `let x = 1` here".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("let x = 1"));
    }

    #[test]
    fn test_code_block() {
        let md = Markdown::new(
            "```rust\nfn main() {}\n```".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(lines.iter().any(|l| l.contains("```")));
        assert!(lines.iter().any(|l| l.contains("fn main()")));
    }

    #[test]
    fn test_blockquote() {
        let md = Markdown::new(
            "> quoted text".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("│"));
        assert!(lines[0].contains("quoted text"));
    }

    #[test]
    fn test_list_unordered() {
        let md = Markdown::new(
            "- item 1\n- item 2".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(lines.iter().any(|l| l.contains("item 1")));
        assert!(lines.iter().any(|l| l.contains("item 2")));
    }

    #[test]
    fn test_list_ordered() {
        let md = Markdown::new(
            "1. first\n2. second".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(lines.iter().any(|l| l.contains("first")));
        assert!(lines.iter().any(|l| l.contains("second")));
    }

    #[test]
    fn test_horizontal_rule() {
        let md = Markdown::new("---".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("─"));
    }

    #[test]
    fn test_empty_text_returns_empty() {
        let md = Markdown::new(String::new(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_padding() {
        let md = Markdown::new(
            "Hello".to_string(),
            2,
            1,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(20);
        assert_eq!(lines.len(), 3); // top pad + content + bottom pad
        assert!(lines[0].len() == 20 || lines[0].is_empty());
        assert!(lines[1].contains("Hello"));
        assert!(lines[2].len() == 20 || lines[2].is_empty());
    }

    #[test]
    fn test_default_text_style() {
        let dts = Box::new(DefaultTextStyle {
            color: Some(Box::new(|t| format!("\x1b[31m{}\x1b[0m", t))),
            bg_color: None,
            bold: true,
            italic: false,
            strikethrough: false,
            underline: false,
        });
        let md = Markdown::new(
            "colored text".to_string(),
            0,
            0,
            default_theme(),
            Some(dts),
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("\x1b[31m"));
        assert!(lines[0].contains("\x1b[1m"));
    }

    #[test]
    fn test_table_simple() {
        let md = Markdown::new(
            "| H1 | H2 |\n|----|----|\n| A  | B  |\n| C  | D  |".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        let output = lines.join("\n");
        assert!(output.contains("┌─"), "Table should contain top border: {:?}", output);
        assert!(output.contains("└─"), "Table should contain bottom border: {:?}", output);
        assert!(output.contains("H1"), "Table should contain H1: {:?}", output);
        assert!(output.contains("H2"), "Table should contain H2: {:?}", output);
        assert!(output.contains("A"), "Table should contain cell A: {:?}", output);
        assert!(output.contains("B"), "Table should contain cell B: {:?}", output);
    }

    #[test]
    fn test_link_rendering() {
        let md = Markdown::new(
            "[text](https://example.com)".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("text"));
        assert!(lines[0].contains("https://example.com"));
    }

    #[test]
    fn test_strikethrough() {
        let md = Markdown::new(
            "~~struck~~".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("struck"));
    }

    #[test]
    fn test_spacing_no_extra_at_end() {
        let md = Markdown::new(
            "# Title".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert_eq!(lines.len(), 1, "Single heading should produce 1 line, got {}: {:?}", lines.len(), lines);
    }

    #[test]
    fn test_bold_inside_heading() {
        let md = Markdown::new(
            "## H2 with **bold**".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("H2 with"));
        assert!(lines[0].contains("bold"));
        assert!(lines[0].contains("\x1b[1m"));
    }

    #[test]
    fn test_link_inside_heading() {
        let md = Markdown::new(
            "# [Title Link](https://example.com)".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("Title Link"));
    }

    #[test]
    fn test_multiple_paragraphs() {
        let md = Markdown::new(
            "First paragraph.\n\nSecond paragraph.".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(
            lines.len() >= 3,
            "Expected at least 3 lines for two paragraphs with spacing, got {}",
            lines.len()
        );
        assert!(lines[0].contains("First paragraph"));
        let has_gap = lines
            .windows(2)
            .any(|w| w[0].contains("First") && w[1].trim().is_empty());
        assert!(has_gap, "Should have empty line between paragraphs");
    }

    #[test]
    fn test_code_inline_with_style_reset() {
        let dts = Box::new(DefaultTextStyle {
            color: Some(Box::new(|t| format!("\x1b[34m{}\x1b[0m", t))),
            bg_color: None,
            bold: false,
            italic: false,
            strikethrough: false,
            underline: false,
        });
        let md = Markdown::new(
            "text `code` text".to_string(),
            0,
            0,
            default_theme(),
            Some(dts),
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        let line = &lines[0];
        assert!(line.contains("text"), "First text should be present");
        assert!(line.contains("code"), "Code should be present");
        let trimmed = line.trim_end();
        assert!(trimmed.ends_with("\x1b[0m") || trimmed.ends_with("\x1b[34m"), "Line should end with proper reset");
    }

    #[test]
    fn test_nested_bold_italic() {
        let md = Markdown::new(
            "***bold italic***".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("bold italic"));
    }

    #[test]
    fn test_empty_list_items() {
        let md = Markdown::new("- A\n\n- B".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.iter().any(|l| l.contains("A")));
        assert!(lines.iter().any(|l| l.contains("B")));
    }

    #[test]
    fn test_cache_invalidation() {
        let mut md = Markdown::new(
            "Hello".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines1 = md.render(80);
        let lines2 = md.render(80);
        assert_eq!(lines1, lines2, "Cached result should match");
        md.set_text("World".to_string());
        let lines3 = md.render(80);
        assert!(lines3[0].contains("World"), "Updated text should render");
    }

    #[test]
    fn test_table_with_alignment() {
        let md = Markdown::new(
            "| Left | Center | Right |\n|:-----|:------:|------:|\n| A    | B      | C     |".to_string(),
            0,
            0,
            default_theme(),
            None,
            None,
        );
        let lines = md.render(100);
        assert!(!lines.is_empty());
        let output = lines.join("\n");
        assert!(output.contains("┌─"), "Table should have top border");
        assert!(output.contains("└─"), "Table should have bottom border");
        assert!(output.contains("Left"), "Header Left should be present");
        assert!(output.contains("Center"), "Header Center should be present");
        assert!(output.contains("Right"), "Header Right should be present");
    }
}
