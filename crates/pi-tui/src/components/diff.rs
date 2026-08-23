//! Diff view — color-coded text diff using `similar`.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use similar::{ChangeTag, TextDiff};

/// A diff viewer widget.
pub struct DiffView {
    lines: Vec<Line<'static>>,
}

impl DiffView {
    pub fn new(old: &str, new: &str) -> Self {
        let mut dv = Self { lines: Vec::new() };
        dv.compute_diff(old, new);
        dv
    }

    fn compute_diff(&mut self, old: &str, new: &str) {
        let style_added = Style::new().fg(Color::Green).bg(Color::Rgb(0x00, 0x2A, 0x00));
        let style_removed = Style::new().fg(Color::Red).bg(Color::Rgb(0x2A, 0x00, 0x00));
        let style_context = Style::default();

        let diff = TextDiff::from_lines(old, new);
        self.lines.clear();

        for change in diff.iter_all_changes() {
            let (style, prefix) = match change.tag() {
                ChangeTag::Delete => (style_removed, "- "),
                ChangeTag::Insert => (style_added, "+ "),
                ChangeTag::Equal => (style_context, "  "),
            };
            let text = change.value().trim_end_matches('\n');
            self.lines
                .push(Line::from(Span::styled(format!("{prefix}{text}"), style)));
        }

        if self.lines.is_empty() {
            self.lines.push(Line::from(Span::raw("(no changes)")));
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Diff ");

        let inner = block.inner(area);
        frame.render_widget(block, area);

        for (i, line) in self.lines.iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }
            frame.render_widget(Paragraph::new(line.clone()), Rect::new(inner.x, y, inner.width, 1));
        }
    }

    pub fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }
}
