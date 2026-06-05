use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::tui::Component;

pub use ratkit::widgets::code_diff::{CodeDiff, DiffConfig, DiffHunk, DiffLine, DiffLineKind, DiffStyle};

pub struct CodeDiffComponent {
    inner: CodeDiff,
}

impl CodeDiffComponent {
    pub fn new() -> Self {
        Self {
            inner: CodeDiff::new(),
        }
    }

    /// Parse a unified diff string and create a component.
    pub fn from_unified_diff(diff: &str) -> Self {
        Self {
            inner: CodeDiff::from_unified_diff(diff),
        }
    }

    /// Parse a unified diff with a custom config.
    pub fn from_unified_diff_with_config(diff: &str, config: DiffConfig) -> Self {
        let mut inner = CodeDiff::from_unified_diff(diff);
        inner.config = config;
        Self { inner }
    }

    pub fn set_file_path(&mut self, path: &str) {
        self.inner.file_path = Some(path.to_string());
    }

    pub fn set_diff(&mut self, diff: &str) {
        self.inner = CodeDiff::from_unified_diff(diff);
    }

    pub fn inner(&self) -> &CodeDiff {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut CodeDiff {
        &mut self.inner
    }
}

impl Default for CodeDiffComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for CodeDiffComponent {
    fn render(&self, width: u16) -> Vec<Line<'static>> {
        if width == 0 {
            return vec![Line::from(vec![])];
        }

        let area = Rect::new(0, 0, width, 10000);
        let mut buf = Buffer::empty(area);
        self.inner.clone().render(area, &mut buf);
        buffer_to_lines(&buf, width)
    }
}

fn buffer_to_lines(buf: &Buffer, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut lines = Vec::new();

    for row in buf.content.chunks(buf.area.width as usize) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut i = 0;

        while i < w {
            let cell = &row[i];

            if cell.symbol().is_empty() {
                i += 1;
                continue;
            }

            let style = cell.style();
            let mut text = String::new();
            text.push_str(cell.symbol());
            i += 1;

            while i < w {
                let next = &row[i];
                if next.symbol().is_empty() {
                    i += 1;
                    continue;
                }
                if next.style() != style {
                    break;
                }
                text.push_str(next.symbol());
                i += 1;
            }

            spans.push(Span::styled(text, style));
        }

        while let Some(last) = spans.last() {
            if last.content.trim().is_empty() {
                spans.pop();
            } else {
                break;
            }
        }

        lines.push(Line::from(spans));
    }

    while let Some(last) = lines.last() {
        if last.spans.is_empty() {
            lines.pop();
        } else {
            break;
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(vec![]));
    }

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_renders_something() {
        let diff = CodeDiffComponent::new();
        let lines = diff.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_zero_width_returns_empty_line() {
        let diff = CodeDiffComponent::new();
        let lines = diff.render(0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.is_empty());
    }

    #[test]
    fn test_from_unified_diff_parses_hunks() {
        let diff_text = "\
@@ -1,3 +1,4 @@
 fn foo() {
-    println!(\"old\");
+    println!(\"new\");
+    println!(\"extra\");
 }
";
        let comp = CodeDiffComponent::from_unified_diff(diff_text);
        // The ratkit CodeDiff Widget render is a stub (shows "Diff: (no file)"),
        // so we verify the parsing layer instead
        assert!(!comp.inner.hunks().is_empty(), "should have parsed at least one hunk");
        let all_lines: Vec<String> = comp
            .inner
            .hunks()
            .iter()
            .flat_map(|h| h.lines().iter())
            .map(|l| l.content.clone())
            .collect();
        assert!(all_lines.iter().any(|l| l.contains("foo")), "hunks should contain 'foo'");
    }

    #[test]
    fn test_set_file_path() {
        let mut comp = CodeDiffComponent::new();
        comp.set_file_path("src/main.rs");
        assert_eq!(comp.inner().file_path.as_deref(), Some("src/main.rs"));
    }

    #[test]
    fn test_set_diff_replaces_content() {
        let mut comp = CodeDiffComponent::new();
        comp.set_diff("@@ -0,0 +1,1 @@\n+new\n");
        // Should not panic
        let _lines = comp.render(80);
    }
}
