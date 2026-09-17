//! Application-owned text selection — Rust port of the fullscreen selection
//! subsystem in the TS original (`packages/tui/src/tui-alt-screen.ts`:
//! `SelectionPoint` / `getWordSelection` / `getSelectionColumns` /
//! `getActiveSelectionText` / `handleSelectionMouseEvent`).
//!
//! The original owns mouse selection in fullscreen mode (regular mode
//! delegates to the terminal emulator): drag / double-click word / triple-click
//! line selection, copy-on-select, and auto-scroll while dragging past the
//! viewport edge. pi-rs renders fullscreen, so this module holds the pure
//! selection logic; the ratatui wiring (mouse routing, highlight, clipboard
//! command) lives in [`crate::app`].
//!
//! Coordinate spaces (original `SelectionPoint.scrollView`):
//! - [`SelSpace::Content`] — the transcript's **content rows** (0 = first
//!   block row, independent of the viewport scroll offset).
//! - [`SelSpace::Screen`] — docked regions (status / editor / footer), where
//!   the row is the physical screen row.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Word-selection joiners (TS `TERMINAL_WORD_SELECTION_JOINERS`): a terminal
/// double-click keeps paths (`a/b`) and kebab-case (`a-b`) tokens whole.
pub const JOINERS: [&str; 2] = ["/", "-"];

/// Which coordinate space a selection point lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelSpace {
    /// Transcript content row (see module docs).
    Content,
    /// Physical screen row (docked regions).
    Screen,
}

/// A selection endpoint. `col` is a 0-based terminal cell column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelPoint {
    pub space: SelSpace,
    pub row: usize,
    pub col: u16,
    /// Whether this point lies *between* cells (word/line selections end on a
    /// cell boundary rather than on a cell) — TS `SelectionPoint.boundary`.
    pub boundary: bool,
}

impl SelPoint {
    pub fn character(space: SelSpace, row: usize, col: u16) -> Self {
        Self { space, row, col, boundary: false }
    }
}

/// Selection granularity (TS `SelectionGranularity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Granularity {
    #[default]
    Character,
    Word,
    Line,
}

/// Click tracking for double/triple-click detection (TS `ClickTarget`).
#[derive(Debug, Clone)]
pub struct ClickTarget {
    pub timestamp: std::time::Instant,
    pub count: u8,
    pub space: SelSpace,
    pub row: usize,
    pub word_start: u16,
    pub word_end: u16,
}

/// Application-owned selection state (TS `selectionAnchor/Focus/Granularity/
/// InitialRange/PressActive/Dragged/LastClick/AutoScrollDirection`).
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub anchor: Option<SelPoint>,
    pub focus: Option<SelPoint>,
    pub granularity: Granularity,
    /// The word/line range captured on double/triple click; drag extension
    /// snaps to this instead of the raw anchor (TS `selectionInitialRange`).
    pub initial_range: Option<(SelPoint, SelPoint)>,
    /// A primary-button press is in progress (TS `selectionPressActive`).
    pub press_active: bool,
    /// The pointer moved while pressed (TS `selectionDragged`).
    pub dragged: bool,
    pub last_click: Option<ClickTarget>,
    /// Auto-scroll direction while dragging past the viewport edge.
    pub auto_scroll_dir: i8,
    /// Pointer position of the held drag (TS `selectionDragPointer`), used to
    /// re-derive the focus after an autoscroll step.
    pub drag_pointer: Option<(u16, u16)>,
    /// Source text snapshot taken on press: content lines for
    /// [`SelSpace::Content`], screen lines for [`SelSpace::Screen`]. The TS
    /// original reads the previous frame's lines on every event; snapshotting
    /// on press is the equivalent for a single drag.
    pub source_lines: Vec<String>,
    /// Total cell width of the source lines' space (clamp for extraction).
    pub source_max_col: u16,
}

impl Selection {
    /// Reset everything (TS `clearTextSelection`).
    pub fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.granularity = Granularity::Character;
        self.initial_range = None;
        self.press_active = false;
        self.dragged = false;
        self.auto_scroll_dir = 0;
        self.drag_pointer = None;
        self.source_lines.clear();
        self.source_max_col = 0;
    }

    /// Stop the drag without touching a completed selection (TS focus-out
    /// handler: only an *active* press is cancelled).
    pub fn cancel_press(&mut self) {
        self.press_active = false;
        self.auto_scroll_dir = 0;
        self.drag_pointer = None;
        self.dragged = false;
    }

    /// Ordered selection bounds, or `None` when empty / cross-space
    /// (TS `getSelectionBounds`).
    pub fn bounds(&self) -> Option<(SelPoint, SelPoint)> {
        let anchor = self.anchor?;
        let focus = self.focus?;
        if anchor.space != focus.space {
            return None;
        }
        if anchor.row == focus.row && anchor.col == focus.col {
            return None;
        }
        let anchor_before = anchor.row < focus.row
            || (anchor.row == focus.row && anchor.col < focus.col);
        Some(if anchor_before { (anchor, focus) } else { (focus, anchor) })
    }

    /// The selected text, or `None` when empty (TS `getActiveSelectionText`).
    /// Lines are trimmed at the end; rows join with `\n`.
    pub fn text(&self) -> Option<String> {
        let (start, end) = self.bounds()?;
        extract_text(&self.source_lines, (start, end), 0, self.source_max_col)
    }

    /// Whether a non-empty selection is visible (TS `hasActiveSelection`).
    pub fn has_active(&self) -> bool {
        self.text().is_some()
    }

    /// Extend the focus during a drag (TS `updateSelectionFocus`): character
    /// drags follow the pointer directly; word/line drags snap to whole
    /// words/lines and flip the anchor when the target precedes the initial
    /// range.
    pub fn update_focus(&mut self, point: SelPoint) {
        if self.granularity == Granularity::Character || self.initial_range.is_none() {
            self.focus = Some(point);
            return;
        }
        let line = self.source_lines.get(point.row).map(String::as_str).unwrap_or("");
        let range = match self.granularity {
            Granularity::Word => word_range(line, point.col),
            Granularity::Line => Some(line_range(line)),
            Granularity::Character => None,
        };
        let Some((start_col, end_col)) = range else { return };
        let initial = self.initial_range.unwrap_or((point, point));
        let target_before = point.row < initial.0.row
            || (point.row == initial.0.row && start_col < initial.0.col);
        if target_before {
            self.anchor = Some(initial.1);
            self.focus = Some(SelPoint { col: start_col, ..point });
        } else {
            self.anchor = Some(initial.0);
            self.focus = Some(SelPoint { col: end_col, boundary: true, ..point });
        }
    }
}

/// Visible terminal-cell width of a string (TS `visibleWidth`; the content is
/// already ANSI-free here).
pub fn visible_width(text: &str) -> u16 {
    UnicodeWidthStr::width(text) as u16
}

/// Cell range of the grapheme at `col` (TS `getGraphemeCellRange`); `None`
/// when `col` is past the end of the line (or the line is empty).
pub fn grapheme_cell_range(line: &str, col: u16) -> Option<(u16, u16)> {
    let mut cell = 0u16;
    for grapheme in line.graphemes(true) {
        let width = visible_width(grapheme);
        if width > 0 && col >= cell && col < cell + width {
            return Some((cell, cell + width));
        }
        cell += width;
    }
    None
}

/// Whole-word range around `col` (TS `getWordSelection`). Word boundaries come
/// from UAX #29 (`unicode_segmentation`, the same algorithm as `Intl.Segmenter`
/// word granularity); `/` and `-` join adjacent word-like segments.
pub fn word_range(line: &str, col: u16) -> Option<(u16, u16)> {
    // (start, end, selectable, joiner)
    let mut segments: Vec<(u16, u16, bool, bool)> = Vec::new();
    let mut start = 0u16;
    for segment in line.split_word_bounds() {
        let end = start + visible_width(segment);
        let joiner = JOINERS.contains(&segment);
        let selectable = segment.chars().any(|c| c.is_alphanumeric()) || joiner;
        segments.push((start, end, selectable, joiner));
        start = end;
    }
    let clicked = segments
        .iter()
        .position(|&(s, e, _, _)| col >= s && col < e)?;
    let can_join = |left: &(u16, u16, bool, bool), right: &(u16, u16, bool, bool)| {
        left.2 && right.2 && (left.3 || right.3)
    };
    let mut selection_start = segments[clicked].0;
    let mut index = clicked;
    while index > 0 && can_join(&segments[index - 1], &segments[index]) {
        selection_start = segments[index - 1].0;
        index -= 1;
    }
    let mut selection_end = segments[clicked].1;
    let mut index = clicked;
    while index + 1 < segments.len() && can_join(&segments[index], &segments[index + 1]) {
        selection_end = segments[index + 1].1;
        index += 1;
    }
    Some((selection_start, selection_end))
}

/// Whole-line range (TS `getLineSelection`).
pub fn line_range(line: &str) -> (u16, u16) {
    (0, visible_width(line))
}

/// Column range selected on `row` (TS `getSelectionColumns`): the first and
/// last rows snap to grapheme boundaries, and the result is clamped to
/// `[min_col, max_col]`.
pub fn columns_for_row(
    line: &str,
    row: usize,
    selection: (SelPoint, SelPoint),
    min_col: u16,
    max_col: u16,
) -> (u16, u16) {
    let line_width = visible_width(line);
    let mut start = min_col;
    let mut end = line_width.min(max_col);
    if row == selection.0.row {
        start = grapheme_cell_range(line, selection.0.col)
            .map(|range| range.0)
            .unwrap_or_else(|| selection.0.col.min(line_width));
    }
    if row == selection.1.row {
        end = if selection.1.boundary {
            selection.1.col.min(line_width)
        } else {
            grapheme_cell_range(line, selection.1.col)
                .map(|range| range.1)
                .unwrap_or_else(|| selection.1.col.saturating_add(1).min(line_width))
        };
    }
    (start.max(min_col), end.min(max_col))
}

/// Slice `[start, end)` cells out of a line, including only whole graphemes that
/// fit inside the range (`strict` mode of TS `sliceByColumn`).
pub fn slice_columns(line: &str, start: u16, end: u16) -> String {
    if end <= start {
        return String::new();
    }
    let mut out = String::new();
    let mut cell = 0u16;
    for grapheme in line.graphemes(true) {
        let width = visible_width(grapheme);
        let g_start = cell;
        let g_end = cell + width;
        cell = g_end;
        if g_start >= start && g_end <= end {
            out.push_str(grapheme);
        }
        if cell >= end {
            break;
        }
    }
    out
}

/// Extract the selected text from `lines` (TS `getActiveSelectionText`): each
/// row is sliced, trimmed at the end, and rows join with `\n`. Returns `None`
/// for an empty result.
pub fn extract_text(
    lines: &[String],
    selection: (SelPoint, SelPoint),
    min_col: u16,
    max_col: u16,
) -> Option<String> {
    let (first, second) = selection;
    if first.row == second.row && first.col == second.col {
        return None;
    }
    // TS `getActiveSelectionText` runs after `getSelectionBounds`, which always
    // orders anchor/focus from top-left to bottom-right.
    let selection = if first.row < second.row
        || (first.row == second.row && first.col < second.col)
    {
        (first, second)
    } else {
        (second, first)
    };
    let mut rows: Vec<String> = Vec::new();
    for row in selection.0.row..=selection.1.row {
        let line = lines.get(row).map(String::as_str).unwrap_or("");
        let (start, end) = columns_for_row(line, row, selection, min_col, max_col);
        rows.push(slice_columns(line, start, end).trim_end().to_string());
    }
    let text = rows.join("\n");
    if text.is_empty() { None } else { Some(text) }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::field_reassign_with_default)]
    use super::*;

    fn content(row: usize, col: u16) -> SelPoint {
        SelPoint::character(SelSpace::Content, row, col)
    }

    #[test]
    fn grapheme_range_snaps_wide_and_combining_glyphs() {
        // "A界🙂e\u{301}Z": A[0,1) 界[1,3) 🙂[3,5) é[5,6) Z[6,7)
        let line = "A界🙂e\u{301}Z";
        assert_eq!(grapheme_cell_range(line, 0), Some((0, 1)));
        assert_eq!(grapheme_cell_range(line, 1), Some((1, 3)));
        assert_eq!(grapheme_cell_range(line, 2), Some((1, 3)));
        assert_eq!(grapheme_cell_range(line, 3), Some((3, 5)));
        assert_eq!(grapheme_cell_range(line, 5), Some((5, 6)));
        assert_eq!(grapheme_cell_range(line, 7), None);
    }

    #[test]
    fn slice_columns_keeps_whole_wide_glyphs() {
        let line = "A界🙂e\u{301}Z";
        assert_eq!(slice_columns(line, 1, 5), "界🙂");
        assert_eq!(slice_columns(line, 5, 7), "e\u{301}Z");
        // End cutting through a wide glyph drops it (strict slicing).
        assert_eq!(slice_columns(line, 0, 2), "A");
    }

    #[test]
    fn word_range_coalesces_slash_and_hyphen_tokens() {
        // TS test: double-click inside `starline` selects the whole path.
        let line = "extensions/starline/fixed-editor/compositor.ts";
        let (start, end) = word_range(line, line.find("starline").unwrap() as u16).unwrap();
        assert_eq!(slice_columns(line, start, end), line);

        let line = "earendil-works/pi-tui";
        let (start, end) = word_range(line, line.find("works").unwrap() as u16).unwrap();
        assert_eq!(slice_columns(line, start, end), line);
    }

    #[test]
    fn word_range_does_not_swallow_whitespace() {
        // TS "does not append whitespace to double-click word highlighting".
        let (start, end) = word_range("foo  bar", 0).unwrap();
        assert_eq!((start, end), (0, 3));
        // Whitespace columns select the whitespace segment itself.
        let (start, end) = word_range("foo  bar", 4).unwrap();
        assert_eq!((start, end), (3, 5));
    }

    #[test]
    fn word_range_splits_cjk_per_ideograph() {
        // Deviation from `Intl.Segmenter`: Rust's UAX #29 word bounds have no
        // CJK dictionary, so each ideograph is its own word-like segment
        // (see DEVIATIONS.md). Double-clicking a CJK char selects that char.
        let line = "中文 abc";
        let (start, end) = word_range(line, 1).unwrap();
        assert_eq!(slice_columns(line, start, end), "中");
        let (start, end) = word_range(line, 3).unwrap();
        assert_eq!(slice_columns(line, start, end), "文");
    }

    #[test]
    fn extract_text_trims_trailing_padding_and_joins_rows() {
        let lines = vec!["alpha   ".to_string(), "beta ".to_string(), "gamma".to_string()];
        let sel = (content(0, 0), content(1, 4));
        assert_eq!(extract_text(&lines, sel, 0, 20).as_deref(), Some("alpha\nbeta"));
        // Zero-width selection yields nothing.
        let sel = (content(0, 1), content(0, 1));
        assert_eq!(extract_text(&lines, sel, 0, 20), None);
    }

    #[test]
    fn extract_text_snaps_endpoint_columns_to_graphemes() {
        let lines = vec!["A界🙂e\u{301}Z".to_string()];
        // Press on the second cell of 界, release on the first cell of 🙂.
        let sel = (content(0, 2), content(0, 3));
        assert_eq!(extract_text(&lines, sel, 0, 20).as_deref(), Some("界🙂"));
        // Reverse drag (release before anchor) extracts the same range.
        let sel = (content(0, 4), content(0, 1));
        assert_eq!(extract_text(&lines, sel, 0, 20).as_deref(), Some("界🙂"));
    }

    #[test]
    fn bounds_rejects_cross_space_and_zero_width_selections() {
        let mut sel = Selection::default();
        sel.anchor = Some(content(0, 0));
        sel.focus = Some(SelPoint::character(SelSpace::Screen, 2, 3));
        assert_eq!(sel.bounds(), None);
        sel.focus = Some(content(0, 0));
        assert_eq!(sel.bounds(), None);
    }

    #[test]
    fn update_focus_snaps_word_drags_to_whole_words() {
        let mut sel = Selection::default();
        sel.source_lines = vec!["zero alpha beta".to_string(), "gamma delta".to_string()];
        sel.source_max_col = 20;
        // Double-click on `beta` (columns 11..15).
        let rect = word_range(&sel.source_lines[0], 12).unwrap();
        sel.granularity = Granularity::Word;
        sel.initial_range = Some((content(0, rect.0), content(0, rect.1)));
        sel.anchor = Some(content(0, rect.0));
        sel.focus = Some(content(0, rect.1));
        // Drag down to `gamma`: the selection becomes `beta\ngamma` (whole words).
        sel.update_focus(content(1, 2));
        let (start, end) = sel.bounds().unwrap();
        assert_eq!(
            extract_text(&sel.source_lines, (start, end), 0, 20).as_deref(),
            Some("beta\ngamma")
        );
        // Dragging back above the initial range flips the anchor: the whole
        // first line is selected (from the initial range's end to `zero`).
        sel.update_focus(content(0, 2));
        let (start, end) = sel.bounds().unwrap();
        assert_eq!(
            extract_text(&sel.source_lines, (start, end), 0, 20).as_deref(),
            Some("zero alpha beta")
        );
    }

    #[test]
    fn update_focus_line_drag_selects_whole_lines() {
        let mut sel = Selection::default();
        sel.source_lines = vec!["zero".to_string(), "gamma delta".to_string()];
        sel.source_max_col = 20;
        sel.granularity = Granularity::Line;
        sel.initial_range = Some((content(0, 0), content(0, 4)));
        sel.anchor = Some(content(0, 0));
        sel.focus = Some(content(0, 4));
        sel.update_focus(content(1, 3));
        let (start, end) = sel.bounds().unwrap();
        assert_eq!(
            extract_text(&sel.source_lines, (start, end), 0, 20).as_deref(),
            Some("zero\ngamma delta")
        );
    }
}
