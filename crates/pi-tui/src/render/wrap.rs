//! Word wrapping with style preservation and soft-wrap joiners.
//!
//! Vendored from grok-build `xai-grok-pager-render` (`render/wrapping.rs` +
//! `render/line_utils.rs` + `util.rs`), Apache-2.0. Kept as a standalone
//! module for pi-tui's message rendering: continuation rows carry the exact
//! skipped substring (joiner) so copy/selection can rejoin soft-wrapped
//! lines, table lines are never wrapped, and blockquote prefixes repeat on
//! continuation rows. See THIRD-PARTY-NOTICES.md.

use ratatui::text::{Line, Span};
use std::borrow::Cow;
use std::ops::Range;
use textwrap::Options;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// util (grok-build pager-render util.rs)

/// Byte offset at which display width would exceed `max_width`, or `s.len()`.
pub fn byte_offset_at_width(s: &str, max_width: usize) -> usize {
    let mut width = 0;
    for (i, ch) in s.char_indices() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > max_width {
            return i;
        }
        width += cw;
    }
    s.len()
}

/// Truncate to at most `max_width` display columns (CJK counts 2), ending
/// with the ellipsis char when cut; a zero budget yields an empty string.
pub fn truncate_to_width(s: &str, max_width: usize) -> Cow<'_, str> {
    if byte_offset_at_width(s, max_width) == s.len() {
        return Cow::Borrowed(s);
    }
    if max_width == 0 {
        return Cow::Borrowed("");
    }
    let end = byte_offset_at_width(s, max_width - 1);
    Cow::Owned(format!("{}...", &s[..end]))
}

// line_utils (grok-build pager-render render/line_utils.rs)

/// Clone a borrowed ratatui `Line` into an owned `'static` line.
pub fn line_to_static(line: &Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .iter()
            .map(|s| Span {
                style: s.style,
                content: std::borrow::Cow::Owned(s.content.to_string()),
            })
            .collect(),
    }
}

/// Append owned copies of borrowed lines to `out`.
pub fn push_owned_lines(src: &[Line<'_>], out: &mut Vec<Line<'static>>) {
    for l in src {
        out.push(line_to_static(l));
    }
}

/// True for a character unsafe to render from untrusted/server text:
/// C0/C1 controls plus Unicode bidi/zero-width format controls.
pub fn is_unsafe_display_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
        )
}

/// Snaps a byte index down to the nearest char boundary.
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    let index = index.min(s.len());
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `String`-owning delegate of `truncate_to_width`.
pub fn truncate_str(s: &str, max_width: usize) -> String {
    truncate_to_width(s, max_width).into_owned()
}

/// Truncate a styled `Line` (multiple spans) to fit within `max_width`.
pub fn truncate_line(line: Line<'static>, max_width: usize) -> Line<'static> {
    if max_width == 0 {
        return Line::from(vec![]);
    }

    let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if total <= max_width {
        return line;
    }

    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut out: Vec<Span<'static>> = Vec::new();

    for span in line.spans {
        let sw = span.content.width();
        if used + sw <= budget {
            used += sw;
            out.push(span);
        } else {
            let remaining = budget - used;
            if remaining > 0 {
                let truncated = take_width(&span.content, remaining);
                out.push(Span::styled(truncated, span.style));
            }
            let ellipsis_style = out.last().map(|s| s.style).unwrap_or_default();
            out.push(Span::styled("…", ellipsis_style));
            return Line::from(out);
        }
    }

    Line::from(out)
}

/// Take the first `n` display columns from a string.
fn take_width(s: &str, n: usize) -> String {
    s[..byte_offset_at_width(s, n)].to_string()
}

/// Clip or pad a styled `Line` to exactly `width` display columns.
pub fn fit_line_to_width<'a>(line: Line<'a>, width: usize) -> Line<'a> {
    let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
    if total == width {
        return line;
    }

    let Line {
        style,
        alignment,
        mut spans,
    } = line;

    if total < width {
        spans.push(Span::raw(" ".repeat(width - total)));
        return Line {
            style,
            alignment,
            spans,
        };
    }

    // Wider than width: clip on grapheme boundaries, no ellipsis.
    let mut out: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let sw = span.content.width();
        if used + sw <= width {
            used += sw;
            out.push(span);
            if used == width {
                break;
            }
            continue;
        }
        let remaining = width - used;
        let mut taken = String::new();
        let mut taken_width = 0usize;
        for g in span.content.graphemes(true) {
            let gw = g.width();
            if taken_width + gw > remaining {
                break;
            }
            taken_width += gw;
            taken.push_str(g);
        }
        if !taken.is_empty() {
            out.push(Span::styled(taken, span.style));
            used += taken_width;
        }
        if used < width {
            out.push(Span::raw(" ".repeat(width - used)));
        }
        break;
    }

    Line {
        style,
        alignment,
        spans: out,
    }
}

// wrapping.rs (grok-build pager-render render/wrapping.rs)
pub fn wrap_ranges_trim<'a, O>(text: &str, width_or_options: O) -> Vec<Range<usize>>
where
    O: Into<Options<'a>>,
{
    let opts = width_or_options.into();
    let mut lines: Vec<Range<usize>> = Vec::new();
    for line in textwrap::wrap(text, opts).iter() {
        match line {
            Cow::Borrowed(slice) => {
                let start = unsafe { slice.as_ptr().offset_from(text.as_ptr()) as usize };
                let end = start + slice.len();
                lines.push(start..end);
            }
            Cow::Owned(_) => panic!("wrap_ranges_trim: unexpected owned string"),
        }
    }
    lines
}

/// A segment of a match highlight on a single wrapped row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSegment {
    /// Wrapped row index (0-based).
    pub row: usize,
    /// Start display column within that row (0-based, unicode-width aware).
    pub col_start: usize,
    /// End display column (exclusive).
    pub col_end: usize,
}

/// Map a byte range in `text` to one or more `(row, col_start, col_end)`
/// segments, given the byte ranges per wrapped row.
///
/// `text`        — the original flat text (needed for display-width conversion).
/// `wrap_ranges` — byte ranges per wrapped row (from `wrap_ranges_trim`).
/// `match_range` — the byte range of the match in the flat text.
///
/// `col_start` and `col_end` in the returned segments are **display columns**,
/// not byte offsets.  This correctly handles multi-byte characters (e.g. em dash
/// `—` is 3 bytes but 1 display column).
///
/// Wrapping options with initial/subsequent indent support.
#[derive(Debug, Clone)]
pub struct RtOptions<'a> {
    /// The width in columns at which the text will be wrapped.
    pub width: usize,
    /// Line ending used for breaking lines.
    pub line_ending: textwrap::LineEnding,
    /// Indentation used for the first line of output.
    pub initial_indent: Line<'a>,
    /// Indentation used for subsequent lines of output.
    pub subsequent_indent: Line<'a>,
    /// Allow long words to be broken if they cannot fit on a line.
    pub break_words: bool,
    /// Wrapping algorithm to use.
    pub wrap_algorithm: textwrap::WrapAlgorithm,
    /// The line breaking algorithm to use.
    pub word_separator: textwrap::WordSeparator,
    /// The method for splitting words.
    pub word_splitter: textwrap::WordSplitter,
}

impl From<usize> for RtOptions<'_> {
    fn from(width: usize) -> Self {
        RtOptions::new(width)
    }
}

#[allow(dead_code)]
impl<'a> RtOptions<'a> {
    pub fn new(width: usize) -> Self {
        RtOptions {
            width,
            line_ending: textwrap::LineEnding::LF,
            initial_indent: Line::default(),
            subsequent_indent: Line::default(),
            break_words: true,
            word_separator: textwrap::WordSeparator::new(),
            wrap_algorithm: textwrap::WrapAlgorithm::FirstFit,
            word_splitter: textwrap::WordSplitter::HyphenSplitter,
        }
    }

    pub fn line_ending(self, line_ending: textwrap::LineEnding) -> Self {
        RtOptions {
            line_ending,
            ..self
        }
    }

    pub fn width(self, width: usize) -> Self {
        RtOptions { width, ..self }
    }

    pub fn initial_indent(self, initial_indent: Line<'a>) -> Self {
        RtOptions {
            initial_indent,
            ..self
        }
    }

    pub fn subsequent_indent(self, subsequent_indent: Line<'a>) -> Self {
        RtOptions {
            subsequent_indent,
            ..self
        }
    }

    pub fn break_words(self, break_words: bool) -> Self {
        RtOptions {
            break_words,
            ..self
        }
    }

    pub fn word_separator(self, word_separator: textwrap::WordSeparator) -> RtOptions<'a> {
        RtOptions {
            word_separator,
            ..self
        }
    }

    pub fn wrap_algorithm(self, wrap_algorithm: textwrap::WrapAlgorithm) -> RtOptions<'a> {
        RtOptions {
            wrap_algorithm,
            ..self
        }
    }

    pub fn word_splitter(self, word_splitter: textwrap::WordSplitter) -> RtOptions<'a> {
        RtOptions {
            word_splitter,
            ..self
        }
    }
}

/// Wrap a single line, preserving styles.
#[must_use]
pub fn word_wrap_line<'a, O>(line: &'a Line<'a>, width_or_options: O) -> Vec<Line<'a>>
where
    O: Into<RtOptions<'a>>,
{
    let (lines, _joiners) = word_wrap_line_with_joiners(line, width_or_options);
    lines
}

fn flatten_line_and_bounds<'a>(
    line: &'a Line<'a>,
) -> (String, Vec<(Range<usize>, ratatui::style::Style)>) {
    let mut flat = String::new();
    let mut span_bounds = Vec::new();
    let mut acc = 0usize;
    for s in &line.spans {
        let text = s.content.as_ref();
        let start = acc;
        flat.push_str(text);
        acc += text.len();
        span_bounds.push((start..acc, s.style));
    }
    (flat, span_bounds)
}

fn build_wrapped_line_from_range<'a>(
    indent: Line<'a>,
    original: &'a Line<'a>,
    span_bounds: &[(Range<usize>, ratatui::style::Style)],
    range: &Range<usize>,
    cursor: &mut usize,
) -> Line<'a> {
    let mut out = indent.style(original.style);
    let sliced = slice_line_spans(original, span_bounds, range, cursor);
    let mut spans = out.spans;
    spans.append(
        &mut sliced
            .spans
            .into_iter()
            .map(|s| s.patch_style(original.style))
            .collect(),
    );
    out.spans = spans;
    out
}

/// Check if a line is a table line (box-drawing border or content row).
///
/// Table lines start with box-drawing characters and should never be word-wrapped,
/// as wrapping destroys column alignment. Instead, they are passed through as-is
/// and clipped by the terminal at the edge.
///
/// Blockquote lines also start with `│` (U+2502) but should NOT be treated as
/// table lines — they need word wrapping with the prefix repeated on continuation
/// lines.  The distinction: table content rows have interior `│` cell separators
/// (e.g. `│ cell1 │ cell2 │`), while blockquote lines have `│` only in the
/// leading prefix (e.g. `│ text` or `│ │ nested text`).
fn is_table_line(line: &Line<'_>) -> bool {
    let mut chars = line.spans.iter().flat_map(|s| s.content.chars());
    match chars.next() {
        // Box-drawing border characters (horizontal lines, corners, junctions)
        // — these are unambiguously table borders, never blockquote prefixes.
        Some(ch)
            if ('\u{2500}'..='\u{257F}').contains(&ch) && ch != '\u{2502}' && ch != '\u{2503}' =>
        {
            true
        }
        // │ (U+2502) at line start: table content row only if │ appears after
        // the leading prefix region (blockquote prefixes are only │ and spaces).
        Some('\u{2502}') => {
            let mut in_prefix = true;
            for ch in chars {
                if in_prefix && (ch == '\u{2502}' || ch == ' ') {
                    continue;
                }
                in_prefix = false;
                if ch == '\u{2502}' {
                    return true;
                }
            }
            false
        }
        // ASCII table borders (TableBorders::ASCII uses '+' and '|')
        Some('|') => true,
        _ => false,
    }
}

/// Byte length of the blockquote prefix at the start of `flat`.
///
/// A blockquote prefix is one or more `│ ` (U+2502 + space) sequences,
/// e.g. `│ ` for a single-level quote or `│ │ ` for a nested quote.
/// Returns 0 if the text does not start with a blockquote prefix.
///
/// The selection layer's stricter, style-aware twin lives in xai-grok-pager
/// scrollback/blocks/quote_bar.rs (`rendered_quote_prefix_len`); it relies on
/// this wrap layer re-injecting the prefix spans (with their styles) on
/// continuation rows, so keep the two shapes in agreement.
fn blockquote_prefix_len(flat: &str) -> usize {
    const BAR_BYTES: usize = '\u{2502}'.len_utf8(); // 3
    let mut len = 0;
    let mut chars = flat.chars();
    while let Some('\u{2502}') = chars.next() {
        if chars.next() == Some(' ') {
            len += BAR_BYTES + 1;
        } else {
            break;
        }
    }
    len
}

/// Wrap a single line and also return, for each output line, the string that should be inserted
/// when joining it to the previous output line as a *soft wrap*.
///
/// - The first output line always has `None`.
/// - Continuation lines have `Some(joiner)` where `joiner` is the exact substring (often spaces,
///   possibly empty) that was skipped at the wrap boundary.
pub fn word_wrap_line_with_joiners<'a, O>(
    line: &'a Line<'a>,
    width_or_options: O,
) -> (Vec<Line<'a>>, Vec<Option<String>>)
where
    O: Into<RtOptions<'a>>,
{
    let mut rt_opts: RtOptions<'a> = width_or_options.into();

    // Table lines must never be word-wrapped (it destroys column alignment).
    // Clip+pad each row to the content width so it owns every column; trusting
    // the terminal to clip at the edge desyncs by a column on glyphs it renders
    // wider than measured, stranding a "ghost" cell past the trailing border.
    if is_table_line(line) {
        let fitted = fit_line_to_width(line.clone(), rt_opts.width);
        return (vec![fitted], vec![None]);
    }

    let (flat, span_bounds) = flatten_line_and_bounds(line);

    // Blockquote lines start with │ (U+2502) prefix(es).  Set subsequent_indent
    // to the prefix so continuation lines repeat the quote marker.
    // Skip if the caller already set a subsequent_indent (caller intent wins).
    let bq_len = blockquote_prefix_len(&flat);
    if bq_len > 0 && bq_len < flat.len() && rt_opts.subsequent_indent.width() == 0 {
        let prefix = slice_line_spans(line, &span_bounds, &(0..bq_len), &mut 0);
        rt_opts = rt_opts.subsequent_indent(prefix);
    }

    let opts = Options::new(rt_opts.width)
        .line_ending(rt_opts.line_ending)
        .break_words(rt_opts.break_words)
        .wrap_algorithm(rt_opts.wrap_algorithm)
        .word_separator(rt_opts.word_separator)
        .word_splitter(rt_opts.word_splitter);

    let mut out: Vec<Line<'a>> = Vec::new();
    let mut joiners: Vec<Option<String>> = Vec::new();

    // The first output line uses the initial indent and a reduced available width.
    let initial_width_available = opts
        .width
        .saturating_sub(rt_opts.initial_indent.width())
        .max(1);
    let initial_wrapped = wrap_ranges_trim(&flat, opts.clone().width(initial_width_available));
    let Some(first_line_range) = initial_wrapped.first() else {
        out.push(rt_opts.initial_indent.clone());
        joiners.push(None);
        return (out, joiners);
    };

    // Shared monotonic cursor into `span_bounds`: rows are emitted in
    // increasing byte order, so each row resumes the span scan where the
    // previous one stopped instead of rescanning from 0 (see
    // `slice_line_spans`). This is what keeps wrapping one huge line linear.
    let mut span_cursor = 0usize;

    let first_line = build_wrapped_line_from_range(
        rt_opts.initial_indent.clone(),
        line,
        &span_bounds,
        first_line_range,
        &mut span_cursor,
    );
    out.push(first_line);
    joiners.push(None);

    // Wrap the remainder using subsequent indent width.
    let mut base = first_line_range.end;
    let skip_leading_spaces = flat[base..].chars().take_while(|c| *c == ' ').count();
    let joiner_first = flat[base..base.saturating_add(skip_leading_spaces)].to_string();
    base = base.saturating_add(skip_leading_spaces);

    let subsequent_width_available = opts
        .width
        .saturating_sub(rt_opts.subsequent_indent.width())
        .max(1);
    let remaining = &flat[base..];
    let remaining_wrapped = wrap_ranges_trim(remaining, opts.width(subsequent_width_available));

    let mut prev_end = 0usize;
    for (i, r) in remaining_wrapped.iter().enumerate() {
        if r.is_empty() {
            continue;
        }

        let joiner = if i == 0 {
            joiner_first.clone()
        } else {
            remaining[prev_end..r.start].to_string()
        };
        prev_end = r.end;

        let offset_range = (r.start + base)..(r.end + base);
        let subsequent_line = build_wrapped_line_from_range(
            rt_opts.subsequent_indent.clone(),
            line,
            &span_bounds,
            &offset_range,
            &mut span_cursor,
        );
        out.push(subsequent_line);
        joiners.push(Some(joiner));
    }

    (out, joiners)
}

/// Wrap a sequence of lines, applying the initial indent only to the very first
/// output line, and using the subsequent indent for all later wrapped pieces.
#[allow(private_bounds)]
pub fn word_wrap_lines<'a, I, O, L>(lines: I, width_or_options: O) -> Vec<Line<'static>>
where
    I: IntoIterator<Item = L>,
    L: IntoLineInput<'a>,
    O: Into<RtOptions<'a>>,
{
    let base_opts: RtOptions<'a> = width_or_options.into();
    let mut out: Vec<Line<'static>> = Vec::new();

    for (idx, line) in lines.into_iter().enumerate() {
        let line_input = line.into_line_input();
        let opts = if idx == 0 {
            base_opts.clone()
        } else {
            let mut o = base_opts.clone();
            let sub = o.subsequent_indent.clone();
            o = o.initial_indent(sub);
            o
        };
        let wrapped = word_wrap_line(line_input.as_ref(), opts);
        push_owned_lines(&wrapped, &mut out);
    }

    out
}

/// Like `word_wrap_lines`, but also returns a parallel vector of soft-wrap joiners.
#[allow(private_bounds)]
pub fn word_wrap_lines_with_joiners<'a, I, O, L>(
    lines: I,
    width_or_options: O,
) -> (Vec<Line<'static>>, Vec<Option<String>>)
where
    I: IntoIterator<Item = L>,
    L: IntoLineInput<'a>,
    O: Into<RtOptions<'a>>,
{
    let base_opts: RtOptions<'a> = width_or_options.into();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut joiners: Vec<Option<String>> = Vec::new();

    for (idx, line) in lines.into_iter().enumerate() {
        let line_input = line.into_line_input();
        let opts = if idx == 0 {
            base_opts.clone()
        } else {
            let mut o = base_opts.clone();
            let sub = o.subsequent_indent.clone();
            o = o.initial_indent(sub);
            o
        };

        let (wrapped, wrapped_joiners) = word_wrap_line_with_joiners(line_input.as_ref(), opts);
        for (l, j) in wrapped.into_iter().zip(wrapped_joiners) {
            out.push(line_to_static(&l));
            joiners.push(j);
        }
    }

    (out, joiners)
}

/// Utilities to allow wrapping either borrowed or owned lines.
#[derive(Debug)]
enum LineInput<'a> {
    Borrowed(&'a Line<'a>),
    Owned(Line<'a>),
}

impl<'a> LineInput<'a> {
    fn as_ref(&self) -> &Line<'a> {
        match self {
            LineInput::Borrowed(line) => line,
            LineInput::Owned(line) => line,
        }
    }
}

trait IntoLineInput<'a> {
    fn into_line_input(self) -> LineInput<'a>;
}

impl<'a> IntoLineInput<'a> for &'a Line<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Borrowed(self)
    }
}

impl<'a> IntoLineInput<'a> for &'a mut Line<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Borrowed(self)
    }
}

impl<'a> IntoLineInput<'a> for Line<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(self)
    }
}

impl<'a> IntoLineInput<'a> for String {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for &'a str {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for Cow<'a, str> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for Span<'a> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

impl<'a> IntoLineInput<'a> for Vec<Span<'a>> {
    fn into_line_input(self) -> LineInput<'a> {
        LineInput::Owned(Line::from(self))
    }
}

/// Slice the spans of `original` down to byte `range`, using `cursor` as a
/// monotonic hint of the first span that may still be relevant.
///
/// `span_bounds` are contiguous, sorted, non-overlapping byte ranges (one per
/// span in `original`, from [`flatten_line_and_bounds`]). A line is wrapped
/// top-to-bottom, so successive `range`s have non-decreasing `start`; that lets
/// `cursor` advance permanently past spans that end before the current row
/// instead of rescanning from index 0 on every row.
///
/// Without the cursor this is O(rows × spans): each of the R wrapped rows
/// rescans all S spans — quadratic on one huge line (e.g. a long streamed
/// reasoning paragraph flattened to thousands of styled spans, the pathology
/// behind the 100%-CPU render-thread spin). With it, wrapping a line is
/// O(rows + spans).
///
/// `cursor` must start at `0` (or any index ≤ the first relevant span) and be
/// reused across the row sequence for one line. Issuing a `range` that starts
/// before a previous one is unsupported — the cursor does not rewind.
fn slice_line_spans<'a>(
    original: &'a Line<'a>,
    span_bounds: &[(Range<usize>, ratatui::style::Style)],
    range: &Range<usize>,
    cursor: &mut usize,
) -> Line<'a> {
    let start_byte = range.start;
    let end_byte = range.end;

    // Spans ending at/before this row's start are done for every later row too
    // (ranges are contiguous and queries monotonic), so advance the shared
    // cursor past them once and never revisit them.
    while span_bounds
        .get(*cursor)
        .is_some_and(|(r, _)| r.end <= start_byte)
    {
        *cursor += 1;
    }

    let mut acc: Vec<Span<'a>> = Vec::new();
    for (i, (r, style)) in span_bounds.iter().enumerate().skip(*cursor) {
        let s = r.start;
        let e = r.end;
        if s >= end_byte {
            break;
        }
        let seg_start = start_byte.max(s);
        let seg_end = end_byte.min(e);
        if seg_end > seg_start {
            let local_start = seg_start - s;
            let local_end = seg_end - s;
            let content = original.spans[i].content.as_ref();
            let slice = &content[local_start..local_end];
            acc.push(Span {
                style: *style,
                content: Cow::Borrowed(slice),
            });
        }
        if e >= end_byte {
            break;
        }
    }
    Line {
        style: original.style,
        alignment: original.alignment,
        spans: acc,
    }
}
