//! Theme — the TS original interactive palette, shared by every widget.
//!
//! Source of truth: the TS monorepo's built-in dark theme
//! (`packages/coding-agent/src/modes/interactive/theme/dark.json`), with
//! `vars` resolved to their hex values. Every pi-tui surface (scrollback
//! boxes, editor borders, dialogs, completers, diff views) uses these
//! tokens so the whole UI agrees on one palette — the same way the TS
//! original drives all components from one `Theme` instance.
//!
//! Reference values (dark.json):
//!
//! | token | hex |
//! | --- | --- |
//! | accent | `#8abeb7` |
//! | border / borderAccent / borderMuted | `#5f87ff` / `#00d7ff` / `#505050` |
//! | success / error / warning | `#b5bd68` / `#cc6666` / `#ffff00` |
//! | muted / dim / text / thinkingText | `#808080` / `#666666` / `#d4d4d4` / `#808080` |
//! | selectedBg | `#3a3a4a` |
//! | userMessageBg / userMessageText | `#343541` / `#d4d4d4` |
//! | toolPendingBg / toolSuccessBg / toolErrorBg | `#282832` / `#283228` / `#3c2828` |
//! | toolTitle / toolOutput | `#d4d4d4` / `#808080` |
//! | mdHeading / mdLink / mdLinkUrl | `#f0c674` / `#81a2be` / `#666666` |
//! | mdCode / mdCodeBlock / mdCodeBlockBorder | `#8abeb7` / `#b5bd68` / `#808080` |
//! | mdQuote / mdQuoteBorder / mdHr / mdListBullet | `#808080` / `#808080` / `#808080` / `#8abeb7` |
//! | toolDiffAdded / toolDiffRemoved / toolDiffContext | `#b5bd68` / `#cc6666` / `#808080` |

use ratatui::style::Color;

/// Build a `Color::Rgb` from a 0xRRGGBB value (const-friendly).
pub const fn rgb(hex: u32) -> Color {
    Color::Rgb(((hex >> 16) & 0xff) as u8, ((hex >> 8) & 0xff) as u8, (hex & 0xff) as u8)
}

// ── Core UI ────────────────────────────────────────────────────────────────
pub const ACCENT: Color = rgb(0x8abeb7);
pub const BORDER: Color = rgb(0x5f87ff);
pub const BORDER_ACCENT: Color = rgb(0x00d7ff);
pub const BORDER_MUTED: Color = rgb(0x505050);
pub const SUCCESS: Color = rgb(0xb5bd68);
pub const ERROR: Color = rgb(0xcc6666);
pub const WARNING: Color = rgb(0xffff00);
pub const MUTED: Color = rgb(0x808080);
pub const DIM: Color = rgb(0x666666);
pub const TEXT: Color = rgb(0xd4d4d4);
pub const THINKING_TEXT: Color = rgb(0x808080);

// ── Backgrounds & content ──────────────────────────────────────────────────
pub const SELECTED_BG: Color = rgb(0x3a3a4a);
pub const USER_MESSAGE_BG: Color = rgb(0x343541);
pub const USER_MESSAGE_TEXT: Color = rgb(0xd4d4d4);
pub const TOOL_PENDING_BG: Color = rgb(0x282832);
pub const TOOL_SUCCESS_BG: Color = rgb(0x283228);
pub const TOOL_ERROR_BG: Color = rgb(0x3c2828);
pub const TOOL_TITLE: Color = rgb(0xd4d4d4);
pub const TOOL_OUTPUT: Color = rgb(0x808080);

// ── Markdown ───────────────────────────────────────────────────────────────
pub const MD_HEADING: Color = rgb(0xf0c674);
pub const MD_LINK: Color = rgb(0x81a2be);
pub const MD_LINK_URL: Color = rgb(0x666666);
pub const MD_CODE: Color = rgb(0x8abeb7);
pub const MD_CODE_BLOCK: Color = rgb(0xb5bd68);
pub const MD_CODE_BLOCK_BORDER: Color = rgb(0x808080);
pub const MD_QUOTE: Color = rgb(0x808080);
pub const MD_QUOTE_BORDER: Color = rgb(0x808080);
pub const MD_HR: Color = rgb(0x808080);
pub const MD_LIST_BULLET: Color = rgb(0x8abeb7);

// ── Tool diffs ─────────────────────────────────────────────────────────────
pub const DIFF_ADDED: Color = rgb(0xb5bd68);
pub const DIFF_REMOVED: Color = rgb(0xcc6666);
pub const DIFF_CONTEXT: Color = rgb(0x808080);

/// The full theme surface the scrollback and dialogs read. Mirrors the TS
/// original `Theme` (dark.json): semantic names, resolved hex values.
#[derive(Clone)]
pub struct Theme {
    pub accent: Color,
    pub border: Color,
    pub border_accent: Color,
    pub border_muted: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub muted: Color,
    pub dim: Color,
    pub text: Color,
    pub thinking_text: Color,
    pub selected_bg: Color,
    pub user_message_bg: Color,
    pub user_message_text: Color,
    pub tool_pending_bg: Color,
    pub tool_success_bg: Color,
    pub tool_error_bg: Color,
    pub tool_title: Color,
    pub tool_output: Color,
    pub md_heading: Color,
    pub md_link: Color,
    pub md_link_url: Color,
    pub md_code: Color,
    pub md_code_block: Color,
    pub md_code_block_border: Color,
    pub md_quote: Color,
    pub md_quote_border: Color,
    pub md_hr: Color,
    pub md_list_bullet: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_context: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: ACCENT,
            border: BORDER,
            border_accent: BORDER_ACCENT,
            border_muted: BORDER_MUTED,
            success: SUCCESS,
            error: ERROR,
            warning: WARNING,
            muted: MUTED,
            dim: DIM,
            text: TEXT,
            thinking_text: THINKING_TEXT,
            selected_bg: SELECTED_BG,
            user_message_bg: USER_MESSAGE_BG,
            user_message_text: USER_MESSAGE_TEXT,
            tool_pending_bg: TOOL_PENDING_BG,
            tool_success_bg: TOOL_SUCCESS_BG,
            tool_error_bg: TOOL_ERROR_BG,
            tool_title: TOOL_TITLE,
            tool_output: TOOL_OUTPUT,
            md_heading: MD_HEADING,
            md_link: MD_LINK,
            md_link_url: MD_LINK_URL,
            md_code: MD_CODE,
            md_code_block: MD_CODE_BLOCK,
            md_code_block_border: MD_CODE_BLOCK_BORDER,
            md_quote: MD_QUOTE,
            md_quote_border: MD_QUOTE_BORDER,
            md_hr: MD_HR,
            md_list_bullet: MD_LIST_BULLET,
            diff_added: DIFF_ADDED,
            diff_removed: DIFF_REMOVED,
            diff_context: DIFF_CONTEXT,
        }
    }
}
