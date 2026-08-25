//! Line-level differential renderer for the alternate screen — the same
//! strategy as the TS original (`TuiAltScreen.doRender` / grok-build):
//!
//! - The frame is rendered into a ratatui buffer, then serialized into one
//!   ANSI-styled string per row.
//! - Rows are compared **as whole strings** against the previous frame;
//!   changed rows are rewritten in full (`ESC[{row};1H` + `ESC[2K` + the
//!   whole row). There are no cell-level partial updates, so a terminal can
//!   never be left with a half-overwritten wide character — the stale-cell
//!   residue class that cell-level diffs (ratatui `Buffer::diff`) are
//!   vulnerable to on terminals with imperfect wide-char handling.
//! - A full clear-and-repaint (`ESC[2J` + every row) happens only on the
//!   first frame and on width/height changes (TS `fullRedraw`).
//! - Every update batch is wrapped in synchronized output
//!   (`ESC[?2026h/l`) so the terminal presents it as one frame (no
//!   flicker), exactly like the TS original.

use std::io::{self, Write};

/// Begin / end synchronized output (flicker-free batches).
const SYNC_START: &str = "\x1b[?2026h";
const SYNC_END: &str = "\x1b[?2026l";

/// Convert a ratatui buffer row into one ANSI-styled text line.
///
/// Trailing *unstyled* spaces are trimmed (the TS components trim their
/// lines, and trimming keeps the row comparison stable); trailing spaces
/// with a background color are kept — the TS `Box` component paints its
/// background across the full row (`applyBg` pads to `width`), so the
/// user-message boxes must keep their background to the row end.
/// Wide characters occupy two cells; the trailing continuation cell is
/// skipped (the terminal advances past the glyph automatically).
fn buffer_row_to_ansi(row: &[ratatui::buffer::Cell]) -> String {
    // Find the last cell that must be emitted: a non-space glyph, or a
    // space with a background color (full-row background boxes).
    let mut end = row.len();
    while end > 0 {
        let c = &row[end - 1];
        if c.symbol() != " " && !c.symbol().is_empty() {
            break;
        }
        if c.bg != ratatui::style::Color::Reset {
            break;
        }
        end -= 1;
    }
    let mut out = String::new();
    let mut fg: Option<ratatui::style::Color> = None;
    let mut bg: Option<ratatui::style::Color> = None;
    let mut mods = ratatui::style::Modifier::empty();
    let mut prev_wide = false;
    for cell in row.iter().take(end) {
        let sym = cell.symbol();
        if sym.is_empty() {
            // Continuation cell of a wide character: consume the wide
            // marker so the *next* glyph is not skipped.
            prev_wide = false;
            continue;
        }
        let width = unicode_width::UnicodeWidthStr::width(sym);
        // Wide characters (CJK, emoji) occupy two cells: the first holds
        // the glyph, the second is the continuation. The terminal advances
        // two columns past the glyph itself, so writing the continuation
        // cell would insert a visible gap between every CJK character.
        if prev_wide {
            prev_wide = false;
            continue;
        }
        prev_wide = width == 2;
        if cell.fg != fg.unwrap_or(ratatui::style::Color::Reset)
            || (fg.is_none() && cell.fg != ratatui::style::Color::Reset)
        {
            out.push_str(&sgr_fg(cell.fg));
            fg = Some(cell.fg);
        }
        if cell.bg != bg.unwrap_or(ratatui::style::Color::Reset)
            || (bg.is_none() && cell.bg != ratatui::style::Color::Reset)
        {
            out.push_str(&sgr_bg(cell.bg));
            bg = Some(cell.bg);
        }
        if cell.modifier != mods {
            out.push_str(&sgr_modifier(cell.modifier));
            mods = cell.modifier;
        }
        out.push_str(sym);
    }
    if end > 0 {
        out.push_str("\x1b[0m");
    }
    out
}

fn sgr_fg(c: ratatui::style::Color) -> String {
    match c {
        ratatui::style::Color::Reset => "\x1b[39m".to_string(),
        ratatui::style::Color::Black => "\x1b[30m".to_string(),
        ratatui::style::Color::Red => "\x1b[31m".to_string(),
        ratatui::style::Color::Green => "\x1b[32m".to_string(),
        ratatui::style::Color::Yellow => "\x1b[33m".to_string(),
        ratatui::style::Color::Blue => "\x1b[34m".to_string(),
        ratatui::style::Color::Magenta => "\x1b[35m".to_string(),
        ratatui::style::Color::Cyan => "\x1b[36m".to_string(),
        ratatui::style::Color::Gray => "\x1b[37m".to_string(),
        ratatui::style::Color::DarkGray => "\x1b[90m".to_string(),
        ratatui::style::Color::LightRed => "\x1b[91m".to_string(),
        ratatui::style::Color::LightGreen => "\x1b[92m".to_string(),
        ratatui::style::Color::LightYellow => "\x1b[93m".to_string(),
        ratatui::style::Color::LightBlue => "\x1b[94m".to_string(),
        ratatui::style::Color::LightMagenta => "\x1b[95m".to_string(),
        ratatui::style::Color::LightCyan => "\x1b[96m".to_string(),
        ratatui::style::Color::White => "\x1b[97m".to_string(),
        ratatui::style::Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        ratatui::style::Color::Indexed(n) => format!("\x1b[38;5;{n}m"),
    }
}

fn sgr_bg(c: ratatui::style::Color) -> String {
    match c {
        ratatui::style::Color::Reset => "\x1b[49m".to_string(),
        ratatui::style::Color::Black => "\x1b[40m".to_string(),
        ratatui::style::Color::Red => "\x1b[41m".to_string(),
        ratatui::style::Color::Green => "\x1b[42m".to_string(),
        ratatui::style::Color::Yellow => "\x1b[43m".to_string(),
        ratatui::style::Color::Blue => "\x1b[44m".to_string(),
        ratatui::style::Color::Magenta => "\x1b[45m".to_string(),
        ratatui::style::Color::Cyan => "\x1b[46m".to_string(),
        ratatui::style::Color::Gray => "\x1b[47m".to_string(),
        ratatui::style::Color::DarkGray => "\x1b[100m".to_string(),
        ratatui::style::Color::LightRed => "\x1b[101m".to_string(),
        ratatui::style::Color::LightGreen => "\x1b[102m".to_string(),
        ratatui::style::Color::LightYellow => "\x1b[103m".to_string(),
        ratatui::style::Color::LightBlue => "\x1b[104m".to_string(),
        ratatui::style::Color::LightMagenta => "\x1b[105m".to_string(),
        ratatui::style::Color::LightCyan => "\x1b[106m".to_string(),
        ratatui::style::Color::White => "\x1b[107m".to_string(),
        ratatui::style::Color::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        ratatui::style::Color::Indexed(n) => format!("\x1b[48;5;{n}m"),
    }
}

fn sgr_modifier(m: ratatui::style::Modifier) -> String {
    if m.is_empty() {
        return String::new();
    }
    use ratatui::style::Modifier;
    let mut codes: Vec<&str> = Vec::new();
    if m.contains(Modifier::BOLD) {
        codes.push("1");
    }
    if m.contains(Modifier::DIM) {
        codes.push("2");
    }
    if m.contains(Modifier::ITALIC) {
        codes.push("3");
    }
    if m.contains(Modifier::UNDERLINED) {
        codes.push("4");
    }
    if m.contains(Modifier::SLOW_BLINK) {
        codes.push("5");
    }
    if m.contains(Modifier::RAPID_BLINK) {
        codes.push("6");
    }
    if m.contains(Modifier::REVERSED) {
        codes.push("7");
    }
    if m.contains(Modifier::HIDDEN) {
        codes.push("8");
    }
    if m.contains(Modifier::CROSSED_OUT) {
        codes.push("9");
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// Convert a ratatui buffer to ANSI text lines (one per buffer row).
pub fn buffer_to_lines(buf: &ratatui::buffer::Buffer) -> Vec<String> {
    let mut lines = Vec::with_capacity(buf.area.height as usize);
    for y in 0..buf.area.height {
        let row: Vec<ratatui::buffer::Cell> = (0..buf.area.width)
            .map(|x| buf[(x, y)].clone())
            .collect();
        lines.push(buffer_row_to_ansi(&row));
    }
    lines
}

/// Line-level differential renderer for the alternate screen (TS
/// `TuiAltScreen.doRender` semantics).
pub struct LineScreen {
    previous_lines: Vec<String>,
    previous_width: u16,
    previous_height: u16,
}

impl Default for LineScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl LineScreen {
    pub fn new() -> Self {
        Self {
            previous_lines: Vec::new(),
            previous_width: 0,
            previous_height: 0,
        }
    }

    /// Render the frame rows to the terminal.
    ///
    /// `lines` must contain exactly `height` rows (the ratatui buffer is
    /// always the full terminal area). `cursor` is the hardware cursor
    /// position (the input caret), `None` hides it.
    pub fn render(
        &mut self,
        lines: &[String],
        cursor: Option<(u16, u16)>,
        width: u16,
        height: u16,
    ) -> io::Result<()> {
        let full_redraw = self.previous_lines.is_empty()
            || self.previous_width != width
            || self.previous_height != height;

        let mut buffer = String::new();
        buffer.push_str(SYNC_START);
        if full_redraw {
            // TS `fullRedraw`: clear the screen, then rewrite every row.
            buffer.push_str("\x1b[2J");
            for (row, line) in lines.iter().enumerate() {
                buffer.push_str(&format!("\x1b[{};1H\x1b[2K{}", row + 1, line));
            }
        } else {
            // Differential pass: rewrite only the changed rows in full
            // (TS: `screen[row] === previousScreen[row]` skip).
            for (row, line) in lines.iter().enumerate() {
                if self.previous_lines.get(row) != Some(line) {
                    buffer.push_str(&format!("\x1b[{};1H\x1b[2K{}", row + 1, line));
                }
            }
        }
        match cursor {
            Some((x, y)) => {
                buffer.push_str(&format!(
                    "\x1b[{};{}H\x1b[?25h",
                    (y as usize).min(height as usize - 1) + 1,
                    (x as usize).min(width as usize - 1) + 1
                ));
            }
            None => buffer.push_str("\x1b[?25l"),
        }
        buffer.push_str(SYNC_END);

        io::stdout().write_all(buffer.as_bytes())?;
        io::stdout().flush()?;

        self.previous_lines = lines.to_vec();
        self.previous_width = width;
        self.previous_height = height;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};

    fn cell(sym: &str, fg: Color, bg: Color, mods: Modifier) -> ratatui::buffer::Cell {
        let mut c = ratatui::buffer::Cell::default();
        c.set_symbol(sym);
        c.set_fg(fg);
        c.set_bg(bg);
        c.modifier = mods;
        c
    }

    fn row(cells: &[ratatui::buffer::Cell]) -> Vec<ratatui::buffer::Cell> {
        cells.to_vec()
    }

    /// Plain ASCII row: trailing unstyled spaces are trimmed.
    #[test]
    fn trims_unstyled_trailing_spaces() {
        let r = row(&[
            cell("a", Color::Reset, Color::Reset, Modifier::empty()),
            cell("b", Color::Reset, Color::Reset, Modifier::empty()),
            cell(" ", Color::Reset, Color::Reset, Modifier::empty()),
            cell(" ", Color::Reset, Color::Reset, Modifier::empty()),
        ]);
        assert_eq!(buffer_row_to_ansi(&r), "ab\x1b[0m");
    }

    /// Full-row background (TS Box `applyBg`): trailing spaces with a
    /// background color must survive so the box paints to the row end.
    #[test]
    fn keeps_background_trailing_spaces() {
        let r = row(&[
            cell("a", Color::Reset, Color::Rgb(52, 53, 65), Modifier::empty()),
            cell(" ", Color::Reset, Color::Rgb(52, 53, 65), Modifier::empty()),
            cell(" ", Color::Reset, Color::Rgb(52, 53, 65), Modifier::empty()),
        ]);
        let out = buffer_row_to_ansi(&r);
        assert!(out.contains("\x1b[48;2;52;53;65m"), "bg emitted: {out:?}");
        assert!(out.ends_with("  \x1b[0m"), "trailing bg spaces kept: {out:?}");
    }

    /// Wide characters: the continuation cell must not be emitted (no gap
    /// between CJK chars), and the row width stays correct.
    #[test]
    fn wide_chars_skip_continuation_cell() {
        let r = row(&[
            cell("你", Color::Reset, Color::Reset, Modifier::empty()),
            cell("", Color::Reset, Color::Reset, Modifier::empty()),
            cell("好", Color::Reset, Color::Reset, Modifier::empty()),
            cell("", Color::Reset, Color::Reset, Modifier::empty()),
        ]);
        assert_eq!(buffer_row_to_ansi(&r), "你好\x1b[0m");
    }

    /// Style changes emit SGR only at change points.
    #[test]
    fn emits_sgr_at_change_points() {
        let r = row(&[
            cell("a", Color::Red, Color::Reset, Modifier::empty()),
            cell("b", Color::Red, Color::Reset, Modifier::empty()),
            cell("c", Color::Green, Color::Reset, Modifier::empty()),
        ]);
        let out = buffer_row_to_ansi(&r);
        assert_eq!(out, "\x1b[31mab\x1b[32mc\x1b[0m");
    }

    /// Differential pass: only changed rows are rewritten; the first frame
    /// and size changes do a full clear + repaint. Capture stdout via a
    /// pipe is awkward, so assert on the state transitions instead: after
    /// render, `previous_lines` mirrors the input.
    #[test]
    fn render_tracks_previous_lines() {
        let mut s = LineScreen::new();
        let lines = vec!["a".to_string(), "b".to_string()];
        // First render: full redraw path (no stdout assertions here — the
        // pty e2e tests cover the emitted bytes).
        let _ = s.render(&lines, None, 2, 2);
        assert_eq!(s.previous_lines, lines);
        assert_eq!(s.previous_width, 2);
        assert_eq!(s.previous_height, 2);
    }

    /// `buffer_to_lines` produces exactly one line per buffer row.
    #[test]
    fn buffer_to_lines_matches_area() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 3));
        buf[(0, 0)].set_symbol("x");
        let lines = buffer_to_lines(&buf);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("x"));
    }
}
