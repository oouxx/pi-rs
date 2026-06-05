use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};

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
    for ev in events {
        match ev {
            Event::Text(t) => result.push_str(&format!("{}{}\x1b[0m", default_prefix, t)),
            Event::Code(t) => result.push_str(&(theme.code)(t)),
            Event::SoftBreak | Event::HardBreak => result.push(' '),
            Event::Start(Tag::Strong) => {}
            Event::End(TagEnd::Strong) => result.push_str(default_prefix),
            Event::Start(Tag::Emphasis) => {}
            Event::End(TagEnd::Emphasis) => result.push_str(default_prefix),
            Event::Start(Tag::Strikethrough) => {}
            Event::End(TagEnd::Strikethrough) => result.push_str(default_prefix),
            Event::Start(Tag::Link { .. }) => {}
            Event::End(TagEnd::Link) => result.push_str(default_prefix),
            _ => {}
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
    let mut in_bold = false;
    let mut in_italic = false;
    let mut in_strike = false;

    for ev in events {
        match ev {
            Event::Text(t) => {
                let mut styled = t.to_string();
                if in_bold {
                    styled = (theme.bold)(&styled);
                }
                if in_italic {
                    styled = (theme.italic)(&styled);
                }
                if in_strike {
                    styled = (theme.strikethrough)(&styled);
                }
                result.push_str(&style_fn(&styled));
                result.push_str(prefix);
            }
            Event::Code(t) => {
                result.push_str(&(theme.code)(t));
                result.push_str(prefix);
            }
            Event::SoftBreak | Event::HardBreak => result.push(' '),
            Event::Start(Tag::Strong) => in_bold = true,
            Event::End(TagEnd::Strong) => {
                in_bold = false;
                result.push_str(prefix);
            }
            Event::Start(Tag::Emphasis) => in_italic = true,
            Event::End(TagEnd::Emphasis) => {
                in_italic = false;
                result.push_str(prefix);
            }
            Event::Start(Tag::Strikethrough) => in_strike = true,
            Event::End(TagEnd::Strikethrough) => {
                in_strike = false;
                result.push_str(prefix);
            }
            _ => {}
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

    fn render_events(&self, events: &[Event], content_width: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut i = 0;
        let n = events.len();
        let prefix = self.default_prefix();

        while i < n {
            match &events[i] {
                Event::Start(tag) => {
                    let (end, inner) = find_inner(events, i + 1);
                    let tag = tag.clone();
                    let mut block_lines = self.render_block(&tag, inner, content_width, &prefix);
                    lines.append(&mut block_lines);
                    i = end;
                }
                Event::Text(t) => {
                    if !t.is_empty() {
                        lines.push(format!("{}{}\x1b[0m", prefix, t));
                    }
                    i += 1;
                }
                Event::Code(t) => {
                    lines.push((self.theme.code)(t));
                    i += 1;
                }
                Event::Rule => {
                    lines.push((self.theme.hr)(&"─".repeat(content_width.min(80))));
                    i += 1;
                }
                Event::SoftBreak | Event::HardBreak => {
                    i += 1;
                }
                Event::Html(t) => {
                    lines.push(format!("{}{}\x1b[0m", prefix, t));
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

    #[allow(clippy::only_used_in_recursion)]
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
                let mut result: Vec<String> = Vec::new();
                for line in inner_lines {
                    for wl in wrap_text_with_ansi(&line, content_width.saturating_sub(2)) {
                        result.push(format!(
                            "{}{}",
                            (self.theme.quote_border)("│ "),
                            (self.theme.quote)(&wl)
                        ));
                    }
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
        let parser = Parser::new(&normalized);
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
