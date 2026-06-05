use crossterm::event::KeyEvent;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::Component;

pub struct DefaultTextStyle {
    pub style: Style,
}

pub struct MarkdownTheme {
    pub heading: Box<dyn Fn(u8) -> Style + Send + Sync>,
    pub heading_prefix: Box<dyn Fn(u8) -> Style + Send + Sync>,
    pub link: Box<dyn Fn() -> Style + Send + Sync>,
    pub link_url: Box<dyn Fn() -> Style + Send + Sync>,
    pub code: Box<dyn Fn() -> Style + Send + Sync>,
    pub code_block: Box<dyn Fn() -> Style + Send + Sync>,
    pub code_block_border: Box<dyn Fn() -> Style + Send + Sync>,
    pub quote: Box<dyn Fn() -> Style + Send + Sync>,
    pub quote_border: Box<dyn Fn() -> Style + Send + Sync>,
    pub hr: Box<dyn Fn() -> Style + Send + Sync>,
    pub list_bullet: Box<dyn Fn() -> Style + Send + Sync>,
    pub bold: Box<dyn Fn() -> Style + Send + Sync>,
    pub italic: Box<dyn Fn() -> Style + Send + Sync>,
    pub strikethrough: Box<dyn Fn() -> Style + Send + Sync>,
    pub underline: Box<dyn Fn() -> Style + Send + Sync>,
    pub table_border: Box<dyn Fn() -> Style + Send + Sync>,
    pub table_header: Box<dyn Fn() -> Style + Send + Sync>,
    pub highlight_code: Option<Box<dyn Fn(&str, Option<&str>) -> Vec<Line<'static>> + Send + Sync>>,
    pub code_block_indent: String,
}

impl MarkdownTheme {
    pub fn default_theme() -> Self {
        let heading_style = Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD);
        Self {
            heading: Box::new(move |level| {
                if level == 1 {
                    heading_style.add_modifier(Modifier::UNDERLINED)
                } else {
                    heading_style
                }
            }),
            heading_prefix: Box::new(move |_level| {
                Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD)
            }),
            link: Box::new(|| Style::new().fg(Color::Blue).add_modifier(Modifier::UNDERLINED)),
            link_url: Box::new(|| Style::new().fg(Color::DarkGray)),
            code: Box::new(|| Style::new().fg(Color::Yellow)),
            code_block: Box::new(|| Style::new().fg(Color::Yellow)),
            code_block_border: Box::new(|| {
                Style::new()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC)
            }),
            quote: Box::new(|| Style::new().add_modifier(Modifier::ITALIC)),
            quote_border: Box::new(|| Style::new().fg(Color::DarkGray)),
            hr: Box::new(|| Style::new().fg(Color::DarkGray)),
            list_bullet: Box::new(|| Style::new().fg(Color::Cyan)),
            bold: Box::new(|| Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            italic: Box::new(|| Style::new().fg(Color::Green).add_modifier(Modifier::ITALIC)),
            strikethrough: Box::new(|| Style::new().add_modifier(Modifier::CROSSED_OUT)),
            underline: Box::new(|| Style::new().add_modifier(Modifier::UNDERLINED)),
            table_header: Box::new(|| {
                Style::new()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            }),
            table_border: Box::new(|| Style::new().fg(Color::DarkGray)),
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

/// The visible width of a string (excluding ANSI — plain Unicode width).
fn visible_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
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

fn render_inline(events: &[Event], theme: &MarkdownTheme, style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let ev = &events[i];
        match ev {
            Event::Text(t) => {
                spans.push(Span::styled(t.to_string(), style));
                i += 1;
            }
            Event::Code(t) => {
                let code_style = style.patch((theme.code)());
                spans.push(Span::styled(t.to_string(), code_style));
                i += 1;
            }
            Event::SoftBreak | Event::HardBreak => {
                spans.push(Span::raw(" "));
                i += 1;
            }
            Event::Start(Tag::Strong) => {
                let (end, inner) = find_inner(events, i + 1);
                let bold_style = style.patch((theme.bold)());
                spans.extend(render_inline(inner, theme, bold_style));
                i = end;
            }
            Event::Start(Tag::Emphasis) => {
                let (end, inner) = find_inner(events, i + 1);
                let italic_style = style.patch((theme.italic)());
                spans.extend(render_inline(inner, theme, italic_style));
                i = end;
            }
            Event::Start(Tag::Strikethrough) => {
                let (end, inner) = find_inner(events, i + 1);
                let strike_style = style.patch((theme.strikethrough)());
                spans.extend(render_inline(inner, theme, strike_style));
                i = end;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let (end, inner) = find_inner(events, i + 1);
                let link_style = style.patch((theme.link)());
                let link_text = collect_text(inner).trim().to_string();
                let href = dest_url.to_string();
                let href_for_comparison = if href.starts_with("mailto:") {
                    href[7..].to_string()
                } else {
                    href.clone()
                };
                spans.extend(render_inline(inner, theme, link_style));
                if link_text != href && link_text != href_for_comparison {
                    let url_style = style.patch((theme.link_url)());
                    spans.push(Span::styled(format!(" ({})", href), url_style));
                }
                i = end;
            }
            Event::End(TagEnd::Link)
            | Event::End(TagEnd::Strong)
            | Event::End(TagEnd::Emphasis)
            | Event::End(TagEnd::Strikethrough) => {
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    spans
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

fn collect_table_cells<'a>(
    events: &'a [Event<'a>],
) -> (Vec<&'a [Event<'a>]>, Vec<Vec<&'a [Event<'a>]>>) {
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
    cached_lines: Option<Vec<Line<'static>>>,
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

    fn default_style(&self) -> Style {
        match self.default_text_style {
            Some(ref dts) => dts.style,
            None => Style::reset(),
        }
    }

    /// Check if the event sequence contains only inline content (no block-level tags).
    /// When true, the events should be rendered as a single paragraph via `render_inline`.
    fn has_only_inline_content(events: &[Event]) -> bool {
        !events.iter().any(|ev| matches!(ev,
            Event::Start(Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::BlockQuote(_)
                | Tag::List(_)
                | Tag::Table(_)
                | Tag::HtmlBlock
                | Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition)
        ))
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

    fn render_events(&self, events: &[Event], content_width: usize) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut i = 0;
        let n = events.len();
        let base_style = self.default_style();

        while i < n {
            match &events[i] {
                Event::Start(tag) => {
                    let (end, inner) = find_inner(events, i + 1);
                    let tag_clone = tag.clone();
                    let mut block_lines =
                        self.render_block(&tag_clone, inner, content_width, base_style);
                    lines.append(&mut block_lines);
                    i = end;
                    if Self::is_next_block(events, i) {
                        lines.push(Line::from(vec![]));
                    }
                }
                Event::Text(t) => {
                    if !t.is_empty() {
                        lines.push(Line::from(Span::styled(t.to_string(), base_style)));
                        let next = i + 1;
                        if next < n && Self::is_next_block(events, next) {
                            lines.push(Line::from(vec![]));
                        }
                    }
                    i += 1;
                }
                Event::Code(t) => {
                    let code_style = base_style.patch((self.theme.code)());
                    lines.push(Line::from(Span::styled(t.to_string(), code_style)));
                    let next = i + 1;
                    if next < n && Self::is_next_block(events, next) {
                        lines.push(Line::from(vec![]));
                    }
                    i += 1;
                }
                Event::Rule => {
                    let hr_style = base_style.patch((self.theme.hr)());
                    let hr_text = "─".repeat(content_width.min(80));
                    lines.push(Line::from(Span::styled(hr_text, hr_style)));
                    let next = i + 1;
                    if next < n && Self::is_next_block(events, next) {
                        lines.push(Line::from(vec![]));
                    }
                    i += 1;
                }
                Event::SoftBreak | Event::HardBreak => {
                    i += 1;
                }
                Event::Html(t) => {
                    if base_style == Style::reset() {
                        lines.push(Line::from(Span::raw(t.to_string())));
                    } else {
                        lines.push(Line::from(Span::styled(t.to_string(), base_style)));
                    }
                    let next = i + 1;
                    if next < n && Self::is_next_block(events, next) {
                        lines.push(Line::from(vec![]));
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
        base_style: Style,
    ) -> Vec<Line<'static>> {
        match tag {
            Tag::Paragraph => {
                let spans = render_inline(inner, &self.theme, base_style);
                if spans.is_empty() {
                    vec![]
                } else {
                    vec![Line::from(spans)]
                }
            }
            Tag::Heading { level, .. } => {
                let h_level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                let heading_style = base_style.patch((self.theme.heading)(h_level));
                let spans = render_inline(inner, &self.theme, heading_style);
                let prefix = format!("{} ", "#".repeat(h_level as usize));
                if h_level >= 3 {
                    let prefix_style = base_style.patch((self.theme.heading_prefix)(h_level));
                    let mut result = Line::from(Span::styled(prefix, prefix_style));
                    result.spans.extend(spans);
                    vec![result]
                } else {
                    vec![Line::from(spans)]
                }
            }
            Tag::BlockQuote(_) => {
                let inner_lines =
                    self.render_events(inner, content_width.saturating_sub(2));
                let quote_style = base_style.patch((self.theme.quote)());
                let border_style = base_style.patch((self.theme.quote_border)());
                let mut result: Vec<Line<'static>> = Vec::new();
                let mut non_empty_found = false;
                for line in inner_lines {
                    let text = line_content(&line);
                    if non_empty_found || !text.trim().is_empty() {
                        non_empty_found = true;
                        let inner_style = if line.spans.is_empty() || line.spans[0].style == Style::reset() {
                            quote_style
                        } else {
                            line.spans[0].style.patch(quote_style)
                        };
                        let mut new_line = Line::from(Span::styled("│ ", border_style));
                        for span in line.spans {
                            new_line.spans.push(Span::styled(
                                span.content.to_string(),
                                inner_style,
                            ));
                        }
                        result.push(new_line);
                    }
                }
                while result.last().map_or(false, |l| line_content(l).trim().is_empty()) {
                    result.pop();
                }
                result
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        let s = l.to_string();
                        if s.is_empty() { None } else { Some(s) }
                    }
                    CodeBlockKind::Indented => None,
                };
                let code_text = collect_text(inner);
                let indent = &self.theme.code_block_indent;
                let border_style = base_style.patch((self.theme.code_block_border)());
                let code_style = base_style.patch((self.theme.code_block)());

                let header = Line::from(Span::styled(
                    format!("```{}", lang.as_deref().unwrap_or("")),
                    border_style,
                ));
                let footer = Line::from(Span::styled("```".to_string(), border_style));
                let mut lines = vec![header];

                if let Some(ref highlight) = self.theme.highlight_code {
                    for hl in highlight(&code_text, lang.as_deref()) {
                        let mut styled_line = Line::from(Span::raw(indent.clone()));
                        styled_line.spans.extend(hl.spans);
                        lines.push(styled_line);
                    }
                } else {
                    for cl in code_text.split('\n') {
                        let styled_line =
                            Line::from(Span::styled(format!("{}{}", indent, cl), code_style));
                        lines.push(styled_line);
                    }
                }
                lines.push(footer);
                lines
            }
            Tag::List(start_number) => {
                let items = collect_list_items(inner);
                let mut result: Vec<Line<'static>> = Vec::new();
                let bullet_style = base_style.patch((self.theme.list_bullet)());
                for (idx, item_events) in items.iter().enumerate() {
                    let marker = if start_number.is_some() {
                        let num = start_number.unwrap_or(1) + idx as u64;
                        format!("{}. ", num)
                    } else {
                        "- ".to_string()
                    };
                    let bw = visible_width(&marker);
                    let item_width = content_width.saturating_sub(bw);
                    let inner_lines = if Self::has_only_inline_content(item_events) {
                        let spans = render_inline(item_events, &self.theme, base_style);
                        if spans.is_empty() {
                            vec![]
                        } else {
                            vec![Line::from(spans)]
                        }
                    } else {
                        self.render_events(item_events, item_width)
                    };
                    for (j, content_line) in inner_lines.iter().enumerate() {
                        if j == 0 {
                            let mut new_line = vec![Span::styled(marker.clone(), bullet_style)];
                            new_line.extend(content_line.spans.iter().cloned());
                            result.push(Line::from(new_line));
                        } else {
                            let mut new_line = vec![Span::raw(" ".repeat(bw))];
                            new_line.extend(content_line.spans.iter().cloned());
                            result.push(Line::from(new_line));
                        }
                    }
                }
                result
            }
            Tag::Item => vec![],
            Tag::Table(alignments) => {
                self.render_table(inner, content_width, alignments, base_style)
            }
            _ => {
                let text = collect_text(inner);
                if text.is_empty() {
                    vec![]
                } else {
                    vec![Line::from(Span::styled(text, base_style))]
                }
            }
        }
    }

    fn render_table(
        &self,
        events: &[Event],
        available_width: usize,
        alignments: &[Alignment],
        base_style: Style,
    ) -> Vec<Line<'static>> {
        let (headers, rows) = collect_table_cells(events);
        let num_cols = headers.len();
        if num_cols == 0 {
            return vec![];
        }

        let border_style = base_style.patch((self.theme.table_border)());
        let header_style = base_style.patch((self.theme.table_header)());

        let border_overhead = 3 * num_cols + 1;
        if available_width <= border_overhead {
            return vec![];
        }
        let available_for_cells = available_width - border_overhead;
        let max_unbroken_word_width = 30;

        let mut natural_widths: Vec<usize> = Vec::with_capacity(num_cols);
        let mut min_word_widths: Vec<usize> = Vec::with_capacity(num_cols);

        for cell_events in &headers {
            let text = span_text(&render_inline(cell_events, &self.theme, base_style));
            natural_widths.push(visible_width(&text));
            min_word_widths.push(longest_word_width(&text, max_unbroken_word_width).max(1));
        }
        for row in &rows {
            for (i, cell_events) in row.iter().enumerate() {
                if i >= num_cols {
                    break;
                }
                let text = span_text(&render_inline(cell_events, &self.theme, base_style));
                natural_widths[i] = natural_widths[i].max(visible_width(&text));
                min_word_widths[i] = min_word_widths[i]
                    .max(longest_word_width(&text, max_unbroken_word_width).max(1));
            }
        }

        let mut column_widths =
            distribute_table_widths(&natural_widths, &min_word_widths, available_for_cells);

        for w in &mut column_widths {
            *w = (*w).max(1);
        }

        let mut lines: Vec<Line<'static>> = Vec::new();

        let apply_alignment = |mut spans: Vec<Span<'static>>, align: Alignment, width: usize| -> Vec<Span<'static>> {
            let cw: usize = spans.iter().map(|s| visible_width(&s.content)).sum();
            let pad = width.saturating_sub(cw);
            if pad == 0 {
                return spans;
            }
            match align {
                Alignment::Right => {
                    let mut result = vec![Span::raw(" ".repeat(pad))];
                    result.extend(spans);
                    result
                }
                Alignment::Center => {
                    let left = pad / 2;
                    let right = pad - left;
                    let mut result = vec![Span::raw(" ".repeat(left))];
                    result.extend(spans);
                    result.push(Span::raw(" ".repeat(right)));
                    result
                }
                _ => {
                    spans.push(Span::raw(" ".repeat(pad)));
                    spans
                }
            }
        };

        let build_frame_line = |cell_wrapped: &[Vec<Vec<Span<'static>>>], line_idx: usize, header: bool| -> Vec<Span<'static>> {
            let mut spans = vec![Span::styled("│ ", border_style)];
            for ci in 0..num_cols {
                if ci > 0 {
                    spans.push(Span::styled(" │ ", border_style));
                }
                if let Some(spans_line) = cell_wrapped.get(ci).and_then(|w| w.get(line_idx)) {
                    let align = alignments.get(ci).copied().unwrap_or(Alignment::None);
                    let aligned = apply_alignment(spans_line.clone(), align, column_widths[ci]);
                    if header {
                        for mut s in aligned {
                            // Add bold modifier to all header cells
                            s.style.add_modifier = s.style.add_modifier | header_style.add_modifier;
                            // Set header fg color only if cell has no explicit fg
                            // (Style::reset() uses Some(Color::Reset), treat that same as None)
                            let needs_header_fg = s.style.fg.is_none()
                                || s.style.fg == Some(Color::Reset);
                            if needs_header_fg {
                                s.style.fg = header_style.fg;
                            }
                            spans.push(s);
                        }
                    } else {
                        spans.extend(aligned);
                    }
                } else {
                    spans.push(Span::raw(" ".repeat(column_widths[ci])));
                }
            }
            spans.push(Span::styled(" │", border_style));
            spans
        };

        let top_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        lines.push(Line::from(Span::styled(
            format!("┌─{}─┐", top_cells.join("─┬─")),
            border_style,
        )));

        let header_wrapped: Vec<Vec<Vec<Span<'static>>>> = headers
            .iter()
            .enumerate()
            .map(|(ci, cell_events)| {
                let spans = render_inline(cell_events, &self.theme, base_style);
                wrap_spans_to_lines(&spans, column_widths[ci])
            })
            .collect();

        let header_line_count = header_wrapped.iter().map(|c| c.len()).max().unwrap_or(1);
        for li in 0..header_line_count {
            lines.push(Line::from(build_frame_line(&header_wrapped, li, true)));
        }

        let sep_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        lines.push(Line::from(Span::styled(
            format!("├─{}─┤", sep_cells.join("─┼─")),
            border_style,
        )));

        for row in &rows {
            let row_wrapped: Vec<Vec<Vec<Span<'static>>>> = row
                .iter()
                .enumerate()
                .map(|(ci, cell_events)| {
                    let spans = render_inline(cell_events, &self.theme, base_style);
                    wrap_spans_to_lines(&spans, column_widths[ci])
                })
                .collect();

            let row_line_count = row_wrapped.iter().map(|c| c.len()).max().unwrap_or(1);
            for li in 0..row_line_count {
                lines.push(Line::from(build_frame_line(&row_wrapped, li, false)));
            }
        }

        let bottom_cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
        lines.push(Line::from(Span::styled(
            format!("└─{}─┘", bottom_cells.join("─┴─")),
            border_style,
        )));

        lines
    }

    fn content_width(&self, total_width: usize) -> usize {
        total_width.saturating_sub(self.padding_x * 2).max(1)
    }
}

/// Helper: get the visible text content of a Line as a String
fn line_content(line: &Line<'static>) -> String {
    let mut s = String::new();
    for span in &line.spans {
        s.push_str(&span.content);
    }
    s
}

/// Helper: get the plain text of a Vec<Span>
fn span_text(spans: &[Span<'static>]) -> String {
    let mut s = String::new();
    for span in spans {
        s.push_str(&span.content);
    }
    s
}

fn wrap_spans_to_lines(spans: &[Span<'static>], max_width: usize) -> Vec<Vec<Span<'static>>> {
    if max_width == 0 || spans.is_empty() {
        return vec![vec![]];
    }

    let mut lines: Vec<Vec<Span<'static>>> = vec![vec![]];
    let line_width = |l: &[Span<'static>]| -> usize { l.iter().map(|s| visible_width(&s.content)).sum() };

    for span in spans {
        let text = &span.content;
        let style = span.style;
        let sw = visible_width(text);

        if sw == 0 {
            continue;
        }

        let cur = line_width(lines.last().unwrap());

        if cur + sw <= max_width {
            lines.last_mut().unwrap().push(span.clone());
        } else if sw <= max_width {
            lines.push(vec![span.clone()]);
        } else {
            let mut remaining: &str = text;
            let mut first = true;
            while !remaining.is_empty() {
                let avail = if first {
                    max_width.saturating_sub(line_width(lines.last().unwrap()))
                } else {
                    max_width
                };
                first = false;

                if avail == 0 {
                    lines.push(vec![]);
                    continue;
                }

                let mut taken = 0;
                let mut tw = 0;
                for ch in remaining.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if tw + cw > avail {
                        break;
                    }
                    tw += cw;
                    taken += 1;
                }

                if taken == 0 {
                    taken = 1;
                }

                let (part, rest) = remaining.split_at(taken);
                lines.last_mut().unwrap().push(Span::styled(part.to_string(), style));
                remaining = rest;

                if !remaining.is_empty() {
                    lines.push(vec![]);
                }
            }
        }
    }

    while lines.len() > 1 && lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }

    lines
}

/// Simple text wrapping by character count (not ANSI-aware, used for tables)
fn wrap_text_plain(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if visible_width(&current) + cw > width {
            lines.push(current);
            current = String::new();
        }
        current.push(ch);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

impl Component for Markdown {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
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
        let mut rendered: Vec<Line<'static>> = self.render_events(&events, cw);

        let left_pad = self.padding_x;
        let right_pad = self.padding_x;

        for line in &mut rendered {
            let mut new_spans = Vec::new();
            if left_pad > 0 {
                new_spans.push(Span::raw(" ".repeat(left_pad)));
            }
            new_spans.extend(line.spans.drain(..));
            if right_pad > 0 {
                new_spans.push(Span::raw(" ".repeat(right_pad)));
            }
            line.spans = new_spans;
        }

        let empty_line = Line::from(vec![]);
        let mut result: Vec<Line<'static>> = Vec::new();
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }
        result.extend(rendered);
        for _ in 0..self.padding_y {
            result.push(empty_line.clone());
        }

        if result.is_empty() {
            result.push(Line::from(vec![]));
        }

        result
    }

    fn handle_input(&mut self, _event: &KeyEvent) {}

    fn invalidate(&mut self) {
        self.cached_text = None;
        self.cached_width = None;
        self.cached_lines = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SyntaxHighlighter;
    use ratatui::backend::TestBackend;

    fn default_theme() -> MarkdownTheme {
        MarkdownTheme::default_theme()
    }

    #[test]
    fn test_simple_paragraph() {
        let md = Markdown::new("Hello, world!".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Hello, world!"));
    }

    #[test]
    fn test_heading_no_marker_h1() {
        let md = Markdown::new("# Heading 1".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Heading 1"));
    }

    #[test]
    fn test_heading_no_marker_h2() {
        let md = Markdown::new("## Heading 2".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Heading 2"));
    }

    #[test]
    fn test_heading_with_marker_h3() {
        let md = Markdown::new("### Heading 3".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("# "));
        assert!(line_content(&lines[0]).contains("Heading 3"));
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
        assert!(line_content(&lines[0]).contains("Title"));
        assert!(
            lines[1].spans.is_empty() || line_content(&lines[1]).contains("Paragraph"),
            "Expected empty line or Paragraph, got: '{:?}'",
            line_content(&lines[1])
        );
    }

    #[test]
    fn test_inline_bold() {
        let md = Markdown::new("Hello **world**!".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("Hello"));
        assert!(line_content(&lines[0]).contains("world"));
        // Check that 'world' has bold style
        let has_bold = lines[0].spans.iter().any(|s| {
            s.content == "world" && s.style.add_modifier.contains(Modifier::BOLD)
        });
        assert!(has_bold, "'world' should have bold style");
    }

    #[test]
    fn test_inline_code() {
        let md = Markdown::new("Use `let x = 1` here".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("let x = 1"));
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
        assert!(lines.iter().any(|l| line_content(l).contains("```")));
        assert!(lines.iter().any(|l| line_content(l).contains("fn main()")));
    }

    #[test]
    fn test_blockquote() {
        let md = Markdown::new("> quoted text".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("│"));
        assert!(line_content(&lines[0]).contains("quoted text"));
    }

    #[test]
    fn test_list_unordered() {
        let md = Markdown::new("- item 1\n- item 2".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.iter().any(|l| line_content(l).contains("item 1")));
        assert!(lines.iter().any(|l| line_content(l).contains("item 2")));
    }

    #[test]
    fn test_list_ordered() {
        let md = Markdown::new("1. first\n2. second".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.iter().any(|l| line_content(l).contains("first")));
        assert!(lines.iter().any(|l| line_content(l).contains("second")));
    }

    #[test]
    fn test_horizontal_rule() {
        let md = Markdown::new("---".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("─"));
    }

    #[test]
    fn test_empty_text_returns_empty() {
        let md = Markdown::new(String::new(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_padding() {
        let md = Markdown::new("Hello".to_string(), 2, 1, default_theme(), None, None);
        let lines = md.render(20);
        assert_eq!(lines.len(), 3);
        assert!(line_content(&lines[1]).contains("Hello"));
    }

    #[test]
    fn test_default_text_style() {
        let dts = Box::new(DefaultTextStyle {
            style: Style::new()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        });
        let md = Markdown::new("colored text".to_string(), 0, 0, default_theme(), Some(dts), None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(lines[0].spans.iter().any(|s| s.style.fg == Some(Color::Red)));
        assert!(lines[0].spans.iter().any(|s| {
            s.style.add_modifier.contains(Modifier::BOLD)
        }));
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
        let output: String = lines.iter().map(|l| line_content(l) + "\n").collect();
        assert!(output.contains("┌─"), "Table should contain top border: {:?}", output);
        assert!(output.contains("└─"), "Table should contain bottom border: {:?}", output);
        assert!(output.contains("H1"), "Table should contain H1");
        assert!(output.contains("H2"), "Table should contain H2");
        assert!(output.contains("A"), "Table should contain cell A");
        assert!(output.contains("B"), "Table should contain cell B");
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
        assert!(line_content(&lines[0]).contains("text"));
        assert!(line_content(&lines[0]).contains("https://example.com"));
    }

    #[test]
    fn test_strikethrough() {
        let md = Markdown::new("~~struck~~".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("struck"));
    }

    #[test]
    fn test_spacing_no_extra_at_end() {
        let md = Markdown::new("# Title".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert_eq!(lines.len(), 1);
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
        assert!(line_content(&lines[0]).contains("H2 with"));
        assert!(line_content(&lines[0]).contains("bold"));
        let has_bold = lines[0].spans.iter().any(|s| {
            s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD)
        });
        assert!(has_bold, "'bold' should have bold style");
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
        assert!(line_content(&lines[0]).contains("Title Link"));
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
        assert!(line_content(&lines[0]).contains("First paragraph"));
        let has_gap = lines
            .windows(2)
            .any(|w| line_content(&w[0]).contains("First") && w[1].spans.is_empty());
        assert!(has_gap, "Should have empty line between paragraphs");
    }

    #[test]
    fn test_code_inline_with_style_reset() {
        let dts = Box::new(DefaultTextStyle {
            style: Style::new().fg(Color::Blue),
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
        assert!(line_content(line).contains("text"), "First text should be present");
        assert!(line_content(line).contains("code"), "Code should be present");
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
        assert!(line_content(&lines[0]).contains("bold italic"));
    }

    #[test]
    fn test_empty_list_items() {
        let md = Markdown::new("- A\n\n- B".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(lines.iter().any(|l| line_content(l).contains("A")));
        assert!(lines.iter().any(|l| line_content(l).contains("B")));
    }

    #[test]
    fn test_cache_invalidation() {
        let mut md = Markdown::new("Hello".to_string(), 0, 0, default_theme(), None, None);
        let lines1 = md.render(80);
        let lines2 = md.render(80);
        assert_eq!(
            lines1.iter().map(|l| line_content(l)).collect::<Vec<_>>(),
            lines2.iter().map(|l| line_content(l)).collect::<Vec<_>>()
        );
        md.set_text("World".to_string());
        let lines3 = md.render(80);
        assert!(
            line_content(&lines3[0]).contains("World"),
            "Updated text should render"
        );
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
        let output: String = lines.iter().map(|l| line_content(l) + "\n").collect();
        assert!(output.contains("┌─"), "Table should have top border");
        assert!(output.contains("└─"), "Table should have bottom border");
        assert!(output.contains("Left"), "Header Left should be present");
        assert!(output.contains("Center"), "Header Center should be present");
        assert!(output.contains("Right"), "Header Right should be present");
    }

    #[test]
    fn test_list_bold_item() {
        let md = Markdown::new("- **bold item**".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("bold item"));
        let has_bold = lines[0].spans.iter().any(|s| {
            s.content == "bold item" && s.style.add_modifier.contains(Modifier::BOLD)
        });
        assert!(has_bold, "'bold item' should have bold style in list");
    }

    #[test]
    fn test_list_inline_code_item() {
        let md = Markdown::new("- use `let x = 1` here".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        assert!(line_content(&lines[0]).contains("let x = 1"));
        let has_code_style = lines[0].spans.iter().any(|s| {
            s.content == "let x = 1" && s.style.fg == Some(Color::Yellow)
        });
        assert!(has_code_style, "inline code should have yellow style in list");
    }

    #[test]
    fn test_list_mixed_inline_styles() {
        let md = Markdown::new(
            "- normal **bold** and `code`".to_string(),
            0, 0, default_theme(), None, None,
        );
        let lines = md.render(80);
        assert!(!lines.is_empty());
        let has_bold = lines[0].spans.iter().any(|s| {
            s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD)
        });
        assert!(has_bold, "'bold' in list should have bold style");
        let has_code = lines[0].spans.iter().any(|s| {
            s.content == "code" && s.style.fg == Some(Color::Yellow)
        });
        assert!(has_code, "'code' in list should have code style");
    }

    #[test]
    fn test_table_inline_styles() {
        let md = Markdown::new(
            "| Normal | Colored |\n|--------|---------|\n| **bold** | `code` |".to_string(),
            0, 0, default_theme(), None, None,
        );
        let lines = md.render(80);
        let output: String = lines.iter().map(|l| line_content(l) + "\n").collect();
        assert!(output.contains("bold"), "Table should contain bold text");
        assert!(output.contains("code"), "Table should contain code text");

        let has_bold = lines.iter().any(|l| {
            l.spans.iter().any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD))
        });
        assert!(has_bold, "'bold' in table cell should have bold style");
        let has_code = lines.iter().any(|l| {
            l.spans.iter().any(|s| s.content == "code" && s.style.fg == Some(Color::Yellow))
        });
        assert!(has_code, "'code' in table cell should have code style");
    }

    #[test]
    fn test_table_inline_styles_header() {
        let md = Markdown::new(
            "| **H1** | `H2` |\n|------|------|\n| A    | B    |".to_string(),
            0, 0, default_theme(), None, None,
        );
        let lines = md.render(80);
        let has_bold = lines.iter().any(|l| {
            l.spans.iter().any(|s| s.content == "H1" && s.style.add_modifier.contains(Modifier::BOLD))
        });
        assert!(has_bold, "'H1' in table header should have bold style due to markdown + header bold");
        let has_code = lines.iter().any(|l| {
            l.spans.iter().any(|s| s.content == "H2" && s.style.fg == Some(Color::Yellow))
        });
        assert!(has_code, "'H2' in table header should have code style");
    }

    #[test]
    fn test_list_italic_item() {
        let md = Markdown::new("- *italic text*".to_string(), 0, 0, default_theme(), None, None);
        let lines = md.render(80);
        assert!(!lines.is_empty());
        let has_italic = lines[0].spans.iter().any(|s| {
            s.content == "italic text" && s.style.add_modifier.contains(Modifier::ITALIC)
        });
        assert!(has_italic, "'italic text' in list should have italic style");
    }

    /// Visual integration test showing rich markdown with syntax highlighting.
    /// Run with: `cargo test -p pi-tui visual_markdown -- --nocapture`
    #[test]
    fn visual_markdown_full() {
        let markdown = r#"# Heading 1

## Heading 2

### Heading 3: with **bold** and `inline code`

- Unordered item one
- Unordered **bold** item
- `code` and *italic* in a list

1. Ordered item alpha
2. Ordered item beta
3. Ordered **bold italic** item

| Left      | Center        | Right |
|:----------|:-------------:|------:|
| lorem ipsum | **bold**     | `code`|
| *italic*    | center text  | 42    |

```rust
fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

fn main() {
    let msg = greet("World");
    println!("{msg}");
}
```

```python
def fibonacci(n: int) -> int:
    if n <= 1:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)

print(fibonacci(10))
```
"#;

        let highlighter = SyntaxHighlighter::new();
        let mut theme = default_theme();
        theme.highlight_code = Some(highlighter.into_highlight_fn());

        let md = Markdown::new(markdown.to_string(), 1, 0, theme, None, None);
        let lines = md.render(100);

        // === Section 1: Plain text overview ===
        println!("\n═══ Markdown Output (plain text) ═══\n");
        let mut code_block_start = vec![];
        for (i, line) in lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            if text.contains("```rust") || text.contains("```python") {
                code_block_start.push(i);
            }
            println!("{:3}: {}", i, text);
        }

        // === Section 2: Style details for code blocks ===
        println!("\n═══ Syntax Highlighting: Span Styles ═══\n");
        use std::fmt::Write;
        for &start in &code_block_start {
            // Find the closing ``` marker
            let end = lines[start + 1..]
                .iter()
                .position(|l| {
                    let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                    t.trim() == "```"
                })
                .map(|pos| start + 1 + pos)
                .unwrap_or(lines.len());
            let lang: String = lines[start].spans.iter().map(|s| s.content.as_ref()).collect();
            println!("  {} — styled spans per token:", lang.trim());
            for line_idx in start + 1..end {
                let line = &lines[line_idx];
                let mut detail = String::new();
                for s in &line.spans {
                    let fg = s.style.fg.map(|c| format!("{:?}", c)).unwrap_or_else(|| "default".into());
                    let mut mods = Vec::new();
                    if s.style.add_modifier.contains(Modifier::BOLD) { mods.push("bold"); }
                    if s.style.add_modifier.contains(Modifier::ITALIC) { mods.push("italic"); }
                    if s.style.add_modifier.contains(Modifier::UNDERLINED) { mods.push("underline"); }
                    let mod_str = if mods.is_empty() { "".into() } else { format!("+{}", mods.join("+")) };
                    write!(detail, "[[{}]{} ", fg, mod_str).unwrap();
                    detail.push_str(s.content.as_ref());
                    detail.push_str("] ");
                }
                // Indent and print with original text as annotation
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                println!("  {:20}  ← {}", detail.trim(), text.trim());
            }
        }

        // === Section 3: ANSI rendering via syntect (24-bit true color) ===
        println!("\n═══ ANSI 24-bit Color Output (true terminal colors) ═══\n");
        {
            use syntect::highlighting::ThemeSet;
            use syntect::util::as_24_bit_terminal_escaped;
            let ps = syntect::parsing::SyntaxSet::load_defaults_newlines();
            let ts = ThemeSet::load_defaults();
            let rust_syntax = ps.find_syntax_by_extension("rs").unwrap();
            let py_syntax = ps.find_syntax_by_extension("py").unwrap();

            let code_rust = r#"fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

fn main() {
    let msg = greet("World");
    println!("{msg}");
}
"#;
            let code_py = r#"def fibonacci(n: int) -> int:
    if n <= 1:
        return n
    return fibonacci(n - 1) + fibonacci(n - 2)

print(fibonacci(10))
"#;

            println!("  --- Rust ---");
            let mut h = syntect::easy::HighlightLines::new(rust_syntax, &ts.themes["base16-ocean.dark"]);
            for line in syntect::util::LinesWithEndings::from(code_rust) {
                let ranges = h.highlight_line(line, &ps).unwrap();
                let escaped = as_24_bit_terminal_escaped(&ranges[..], true);
                print!("  {}", escaped);
            }
            print!("\x1b[0m");

            println!("  --- Python ---");
            let mut h = syntect::easy::HighlightLines::new(py_syntax, &ts.themes["base16-ocean.dark"]);
            for line in syntect::util::LinesWithEndings::from(code_py) {
                let ranges = h.highlight_line(line, &ps).unwrap();
                let escaped = as_24_bit_terminal_escaped(&ranges[..], true);
                print!("  {}", escaped);
            }
            print!("\x1b[0m");
        }

        // === Section 3.5: Full ANSI Rendered Markdown ===
        println!("\n═══ Full ANSI Rendered Markdown (all elements with colors) ═══\n");
        {
            // Helper: convert ratatui Color to ANSI foreground code
            let fg_code = |c: Color| -> Option<String> {
                match c {
                    Color::Black => Some("30".into()),
                    Color::Red => Some("31".into()),
                    Color::Green => Some("32".into()),
                    Color::Yellow => Some("33".into()),
                    Color::Blue => Some("34".into()),
                    Color::Magenta => Some("35".into()),
                    Color::Cyan => Some("36".into()),
                    Color::White => Some("37".into()),
                    Color::DarkGray => Some("90".into()),
                    Color::LightRed => Some("91".into()),
                    Color::LightGreen => Some("92".into()),
                    Color::LightYellow => Some("93".into()),
                    Color::LightBlue => Some("94".into()),
                    Color::LightMagenta => Some("95".into()),
                    Color::LightCyan => Some("96".into()),
                    Color::Rgb(r, g, b) => Some(format!("38;2;{};{};{}", r, g, b)),
                    _ => None,
                }
            };
            for line in &lines {
                for s in &line.spans {
                    let mut codes: Vec<String> = Vec::new();
                    if let Some(code) = s.style.fg.and_then(&fg_code) {
                        codes.push(code);
                    }
                    let m = s.style.add_modifier;
                    if m.contains(Modifier::BOLD) { codes.push("1".into()); }
                    if m.contains(Modifier::ITALIC) { codes.push("3".into()); }
                    if m.contains(Modifier::UNDERLINED) { codes.push("4".into()); }
                    if m.contains(Modifier::CROSSED_OUT) { codes.push("9".into()); }
                    if codes.is_empty() {
                        print!("{}", s.content);
                    } else {
                        print!("\x1b[{}m{}\x1b[0m", codes.join(";"), s.content);
                    }
                }
                println!();
            }
            print!("\x1b[0m");
        }

        // === Section 4: Element Theme Styles (table, list, headings) ===
        println!("\n═══ Element Theme Styles ═══\n");
        {
            use std::fmt::Write;
            let table_top = lines.iter().position(|l| {
                let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                t.starts_with("┌─")
            });
            let table_bottom = lines.iter().position(|l| {
                let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                t.starts_with("└─")
            });
            let header_line = lines.iter().position(|l| {
                let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                t.contains("Left") && t.contains("Center") && t.contains("Right")
            });

            if let Some(h) = header_line {
                println!("  --- Table Header Row (line {}) ---", h);
                for s in &lines[h].spans {
                    let fg = s.style.fg.map(|c| format!("{:?}", c)).unwrap_or_else(|| "default".into());
                    let mods = if s.style.add_modifier.contains(Modifier::BOLD) { " +bold" } else { "" };
                    println!("    [{}{}] {:?}", fg, mods, s.content);
                }
            }
            if let Some(t) = table_top {
                println!("  --- Table Border (line {}) ---", t);
                for s in &lines[t].spans {
                    let fg = s.style.fg.map(|c| format!("{:?}", c)).unwrap_or_else(|| "default".into());
                    println!("    [fg={}] {:?}", fg, s.content);
                }
            }
            if let Some(h1) = lines.iter().position(|l| {
                let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                t.trim() == "# Heading 1"
            }) {
                println!("  --- Heading 1 (line {}) ---", h1);
                for s in &lines[h1].spans {
                    let fg = s.style.fg.map(|c| format!("{:?}", c)).unwrap_or_else(|| "default".into());
                    let mods = if s.style.add_modifier.contains(Modifier::BOLD) { " +bold" } else { "" };
                    if !s.content.trim().is_empty() || fg != "default" {
                        println!("    [{}{}] {:?}", fg, mods, s.content);
                    }
                }
            }
            for (i, line) in lines.iter().enumerate() {
                let t: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                if t.trim_start().starts_with("- ") || t.trim_start().starts_with(|c: char| c.is_ascii_digit() && t.trim_start().contains(". ")) {
                    let mut detail = String::new();
                    for s in &line.spans {
                        let fg = s.style.fg.map(|c| format!("{:?}", c)).unwrap_or_else(|| "default".into());
                        let mods = if s.style.add_modifier.contains(Modifier::BOLD) { " +bold" } else { "" };
                        write!(detail, "[{}{}]{} ", fg, mods, s.content).unwrap();
                    }
                    println!("  --- List Line {}: {}", i, detail.trim());
                    break;
                }
            }
        }

        // === Section 5: Verification ===
        let full_text: String = lines.iter().flat_map(|l| {
            l.spans.iter().map(|s| s.content.as_ref())
        }).collect();

        assert!(full_text.contains("Heading 1"), "Missing H1");
        assert!(full_text.contains("Heading 2"), "Missing H2");
        assert!(full_text.contains("Heading 3"), "Missing H3");
        assert!(full_text.contains("Unordered item one"), "Missing list item");
        assert!(full_text.contains("Ordered item alpha"), "Missing ordered item");
        assert!(full_text.contains("lorem ipsum"), "Missing table cell");
        assert!(full_text.contains("fn greet"), "Missing rust code block");
        assert!(full_text.contains("def fibonacci"), "Missing python code block");

        // Verify that syntax highlighting produces different colors for different tokens
        let has_multi_color = lines.iter().skip(code_block_start[0] + 1).take(9).any(|l| {
            let fgs: Vec<_> = l.spans.iter().filter_map(|s| s.style.fg).collect();
            if fgs.len() < 2 { return false; }
            fgs.iter().any(|c1| fgs.iter().any(|c2| c1 != c2))
        });
        assert!(has_multi_color, "Syntax highlighting should produce different fg colors for different token types");

        // Verify table border has a styled foreground color (not default)
        let border_has_color = lines.iter().any(|l| {
            let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            let trimmed = t.trim();
            (trimmed.starts_with("┌─") || trimmed.starts_with("├─") || trimmed.starts_with("└─"))
                && l.spans.iter().any(|s| s.style.fg.is_some())
        });
        assert!(border_has_color, "Table border should have a foreground color");

        // Verify table header has a distinct foreground color (Cyan)
        let header_has_cyan = lines.iter().any(|l| {
            let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            t.contains("Left") && t.contains("Center") && t.contains("Right")
                && l.spans.iter().any(|s| s.style.fg == Some(Color::Cyan))
        });
        assert!(header_has_cyan, "Table header should have Cyan foreground color");

        // Verify heading has blue foreground color
        let heading_has_blue = lines.iter().any(|l| {
            let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            t.trim() == "Heading 1"
                && l.spans.iter().any(|s| s.style.fg == Some(Color::Blue))
                && l.spans.iter().any(|s| s.style.add_modifier.contains(Modifier::BOLD))
        });
        assert!(heading_has_blue, "Heading 1 should have Blue foreground color with Bold modifier");

        // Verify list bullet has cyan foreground color
        let bullet_has_color = lines.iter().any(|l| {
            let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            (t.trim_start().starts_with("- ") || t.trim_start().starts_with("1. "))
                && l.spans.iter().any(|s| s.style.fg == Some(Color::Cyan))
        });
        assert!(bullet_has_color, "List bullet should have Cyan foreground color");

        println!("\n═══ Test complete — all elements verified with theme coloring ═══");
    }
}
