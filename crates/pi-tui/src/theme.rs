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
/// original `Theme` (dark.json / light.json): semantic names, resolved hex
/// values. `name` is the theme's display name (`dark` / `light`) so the
/// `/theme` command can report which palette is active.
#[derive(Clone)]
pub struct Theme {
    pub name: &'static str,
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

impl Theme {
    /// The TS original light theme (`light.json`), with `vars` resolved to
    /// their hex values. Reference values:
    ///
    /// | token | hex |
    /// | --- | --- |
    /// | accent / borderAccent | `#5a8080` (teal) |
    /// | border | `#547da7` (blue) |
    /// | borderMuted | `#b0b0b0` (lightGray) |
    /// | success / error / warning | `#588458` / `#aa5555` / `#9a7326` |
    /// | muted / dim / text / thinkingText | `#6c6c6c` / `#767676` / `#1f2328` / `#6c6c6c` |
    /// | selectedBg | `#d0d0e0` |
    /// | userMessageBg / userMessageText | `#e8e8e8` / `#1f2328` |
    /// | toolPendingBg / toolSuccessBg / toolErrorBg | `#e8e8f0` / `#e8f0e8` / `#f0e8e8` |
    /// | toolTitle / toolOutput | `#1f2328` / `#6c6c6c` |
    /// | mdHeading / mdLink / mdLinkUrl | `#9a7326` / `#547da7` / `#767676` |
    /// | mdCode / mdCodeBlock / mdCodeBlockBorder | `#5a8080` / `#588458` / `#6c6c6c` |
    /// | mdQuote / mdQuoteBorder / mdHr / mdListBullet | `#6c6c6c` / `#6c6c6c` / `#6c6c6c` / `#588458` |
    /// | toolDiffAdded / toolDiffRemoved / toolDiffContext | `#588458` / `#aa5555` / `#6c6c6c` |
    #[must_use]
    pub fn light() -> Self {
        Self {
            name: "light",
            accent: rgb(0x5a8080),
            border: rgb(0x547da7),
            border_accent: rgb(0x5a8080),
            border_muted: rgb(0xb0b0b0),
            success: rgb(0x588458),
            error: rgb(0xaa5555),
            warning: rgb(0x9a7326),
            muted: rgb(0x6c6c6c),
            dim: rgb(0x767676),
            text: rgb(0x1f2328),
            thinking_text: rgb(0x6c6c6c),
            selected_bg: rgb(0xd0d0e0),
            user_message_bg: rgb(0xe8e8e8),
            user_message_text: rgb(0x1f2328),
            tool_pending_bg: rgb(0xe8e8f0),
            tool_success_bg: rgb(0xe8f0e8),
            tool_error_bg: rgb(0xf0e8e8),
            tool_title: rgb(0x1f2328),
            tool_output: rgb(0x6c6c6c),
            md_heading: rgb(0x9a7326),
            md_link: rgb(0x547da7),
            md_link_url: rgb(0x767676),
            md_code: rgb(0x5a8080),
            md_code_block: rgb(0x588458),
            md_code_block_border: rgb(0x6c6c6c),
            md_quote: rgb(0x6c6c6c),
            md_quote_border: rgb(0x6c6c6c),
            md_hr: rgb(0x6c6c6c),
            md_list_bullet: rgb(0x588458),
            diff_added: rgb(0x588458),
            diff_removed: rgb(0xaa5555),
            diff_context: rgb(0x6c6c6c),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "dark",
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
