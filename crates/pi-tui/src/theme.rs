//! Theme — the TS original interactive palette, shared by every widget.
//!
//! Source of truth: the TS monorepo's built-in dark theme
//! (`packages/coding-agent/src/modes/interactive/theme/dark.json` @ `v0.82.1`),
//! with `vars` resolved to their hex values. Every pi-tui surface (scrollback
//! boxes, editor borders, dialogs, completers, diff views) uses these tokens
//! so the whole UI agrees on one palette — the same way the TS original drives
//! all components from one `Theme` instance.
//!
//! Reference values (v0.82.1 `dark.json`):
//!
//! | token | hex |
//! | --- | --- |
//! | accent | `#8abeb7` |
//! | border / borderAccent / borderMuted | `#5f87ff` / `#00d7ff` / `#505050` |
//! | success / error / warning | `#b5bd68` / `#cc6666` / `#ffff00` |
//! | muted / dim / text / thinkingText | `#808080` / `#666666` / `#d4d4d4` / `#808080` |
//! | selectedBg | `#3a3a4a` |
//! | userMessageBg / userMessageText | `#343541` / `#d4d4d4` |
//! | customMessageBg / customMessageText / customMessageLabel | `#2d2838` / `#d4d4d4` / `#9575cd` |
//! | toolPendingBg / toolSuccessBg / toolErrorBg | `#282832` / `#283228` / `#3c2828` |
//! | toolTitle / toolOutput | `#d4d4d4` / `#808080` |
//! | mdHeading / mdLink / mdLinkUrl | `#f0c674` / `#81a2be` / `#666666` |
//! | mdCode / mdCodeBlock / mdCodeBlockBorder | `#8abeb7` / `#b5bd68` / `#808080` |
//! | mdQuote / mdQuoteBorder / mdHr / mdListBullet | `#808080` / `#808080` / `#808080` / `#8abeb7` |
//! | toolDiffAdded / toolDiffRemoved / toolDiffContext | `#b5bd68` / `#cc6666` / `#808080` |
//! | syntaxComment … syntaxPunctuation | `#6A9955` … `#D4D4D4` (VS Code Dark+) |
//! | thinkingOff … thinkingMax | `#505050` … `#ff5fff` |
//! | bashMode | `#b5bd68` |

use ratatui::style::Color;

/// Build a `Color::Rgb` from a 0xRRGGBB value (const-friendly).
pub const fn rgb(hex: u32) -> Color {
    Color::Rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
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
pub const CUSTOM_MESSAGE_BG: Color = rgb(0x2d2838);
pub const CUSTOM_MESSAGE_TEXT: Color = rgb(0xd4d4d4);
pub const CUSTOM_MESSAGE_LABEL: Color = rgb(0x9575cd);
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

// ── Syntax highlighting (v0.82.1 syntax* tokens) ───────────────────────────
pub const SYNTAX_COMMENT: Color = rgb(0x6a9955);
pub const SYNTAX_KEYWORD: Color = rgb(0x569cd6);
pub const SYNTAX_FUNCTION: Color = rgb(0xdcdcaa);
pub const SYNTAX_VARIABLE: Color = rgb(0x9cdcfe);
pub const SYNTAX_STRING: Color = rgb(0xce9178);
pub const SYNTAX_NUMBER: Color = rgb(0xb5cea8);
pub const SYNTAX_TYPE: Color = rgb(0x4ec9b0);
pub const SYNTAX_OPERATOR: Color = rgb(0xd4d4d4);
pub const SYNTAX_PUNCTUATION: Color = rgb(0xd4d4d4);

// ── Thinking levels (v0.82.1 thinking* tokens) ─────────────────────────────
pub const THINKING_OFF: Color = rgb(0x505050);
pub const THINKING_MINIMAL: Color = rgb(0x6e6e6e);
pub const THINKING_LOW: Color = rgb(0x5f87af);
pub const THINKING_MEDIUM: Color = rgb(0x81a2be);
pub const THINKING_HIGH: Color = rgb(0xb294bb);
pub const THINKING_XHIGH: Color = rgb(0xd183e8);
pub const THINKING_MAX: Color = rgb(0xff5fff);

/// Bash mode prompt border (v0.82.1 `bashMode` token).
pub const BASH_MODE: Color = rgb(0xb5bd68);

/// The full theme surface the scrollback and dialogs read. Mirrors the TS
/// original `Theme` (dark.json / light.json @ v0.82.1): semantic names,
/// resolved hex values. `name` is the theme's display name (`dark` / `light`)
/// so the `/theme` command can report which palette is active.
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
    pub custom_message_bg: Color,
    pub custom_message_text: Color,
    pub custom_message_label: Color,
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
    pub syntax_comment: Color,
    pub syntax_keyword: Color,
    pub syntax_function: Color,
    pub syntax_variable: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_type: Color,
    pub syntax_operator: Color,
    pub syntax_punctuation: Color,
    pub thinking_off: Color,
    pub thinking_minimal: Color,
    pub thinking_low: Color,
    pub thinking_medium: Color,
    pub thinking_high: Color,
    pub thinking_xhigh: Color,
    pub thinking_max: Color,
    pub bash_mode: Color,
}

impl Theme {
    /// The TS original light theme (`light.json` @ v0.82.1), with `vars`
    /// resolved to their hex values. Reference values:
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
    /// | customMessageBg / customMessageText / customMessageLabel | `#ede7f6` / `#1f2328` / `#7e57c2` |
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
            custom_message_bg: rgb(0xede7f6),
            custom_message_text: rgb(0x1f2328),
            custom_message_label: rgb(0x7e57c2),
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
            syntax_comment: rgb(0x008000),
            syntax_keyword: rgb(0x0000ff),
            syntax_function: rgb(0x795e26),
            syntax_variable: rgb(0x001080),
            syntax_string: rgb(0xa31515),
            syntax_number: rgb(0x098658),
            syntax_type: rgb(0x267f99),
            syntax_operator: rgb(0x000000),
            syntax_punctuation: rgb(0x000000),
            thinking_off: rgb(0xb0b0b0),
            thinking_minimal: rgb(0x767676),
            thinking_low: rgb(0x547da7),
            thinking_medium: rgb(0x5a8080),
            thinking_high: rgb(0x875f87),
            thinking_xhigh: rgb(0x8b008b),
            thinking_max: rgb(0xaf005f),
            bash_mode: rgb(0x588458),
        }
    }

    /// Map a thinking level to its border color (TS
    /// `getThinkingBorderColor`: off/minimal/low/medium/high/xhigh/max).
    /// Unknown levels fall back to `thinking_off`, like the TS `default` arm.
    #[must_use]
    pub fn thinking_level_color(&self, level: &str) -> Color {
        match level {
            "minimal" => self.thinking_minimal,
            "low" => self.thinking_low,
            "medium" => self.thinking_medium,
            "high" => self.thinking_high,
            "xhigh" => self.thinking_xhigh,
            "max" => self.thinking_max,
            _ => self.thinking_off,
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
            custom_message_bg: CUSTOM_MESSAGE_BG,
            custom_message_text: CUSTOM_MESSAGE_TEXT,
            custom_message_label: CUSTOM_MESSAGE_LABEL,
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
            syntax_comment: SYNTAX_COMMENT,
            syntax_keyword: SYNTAX_KEYWORD,
            syntax_function: SYNTAX_FUNCTION,
            syntax_variable: SYNTAX_VARIABLE,
            syntax_string: SYNTAX_STRING,
            syntax_number: SYNTAX_NUMBER,
            syntax_type: SYNTAX_TYPE,
            syntax_operator: SYNTAX_OPERATOR,
            syntax_punctuation: SYNTAX_PUNCTUATION,
            thinking_off: THINKING_OFF,
            thinking_minimal: THINKING_MINIMAL,
            thinking_low: THINKING_LOW,
            thinking_medium: THINKING_MEDIUM,
            thinking_high: THINKING_HIGH,
            thinking_xhigh: THINKING_XHIGH,
            thinking_max: THINKING_MAX,
            bash_mode: BASH_MODE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every TS v0.82.1 `ThemeColor`/`ThemeBg` token must be represented.
    /// These are the values the TS `dark.json` resolves to (`vars` applied).
    #[test]
    fn dark_palette_matches_v0_82_1_dark_json() {
        let t = Theme::default();
        let expected: &[(&str, Color)] = &[
            ("accent", rgb(0x8abeb7)),
            ("border", rgb(0x5f87ff)),
            ("borderAccent", rgb(0x00d7ff)),
            ("borderMuted", rgb(0x505050)),
            ("success", rgb(0xb5bd68)),
            ("error", rgb(0xcc6666)),
            ("warning", rgb(0xffff00)),
            ("muted", rgb(0x808080)),
            ("dim", rgb(0x666666)),
            ("text", rgb(0xd4d4d4)),
            ("thinkingText", rgb(0x808080)),
            ("selectedBg", rgb(0x3a3a4a)),
            ("userMessageBg", rgb(0x343541)),
            ("userMessageText", rgb(0xd4d4d4)),
            ("customMessageBg", rgb(0x2d2838)),
            ("customMessageText", rgb(0xd4d4d4)),
            ("customMessageLabel", rgb(0x9575cd)),
            ("toolPendingBg", rgb(0x282832)),
            ("toolSuccessBg", rgb(0x283228)),
            ("toolErrorBg", rgb(0x3c2828)),
            ("toolTitle", rgb(0xd4d4d4)),
            ("toolOutput", rgb(0x808080)),
            ("mdHeading", rgb(0xf0c674)),
            ("mdLink", rgb(0x81a2be)),
            ("mdLinkUrl", rgb(0x666666)),
            ("mdCode", rgb(0x8abeb7)),
            ("mdCodeBlock", rgb(0xb5bd68)),
            ("mdCodeBlockBorder", rgb(0x808080)),
            ("mdQuote", rgb(0x808080)),
            ("mdQuoteBorder", rgb(0x808080)),
            ("mdHr", rgb(0x808080)),
            ("mdListBullet", rgb(0x8abeb7)),
            ("toolDiffAdded", rgb(0xb5bd68)),
            ("toolDiffRemoved", rgb(0xcc6666)),
            ("toolDiffContext", rgb(0x808080)),
            ("syntaxComment", rgb(0x6a9955)),
            ("syntaxKeyword", rgb(0x569cd6)),
            ("syntaxFunction", rgb(0xdcdcaa)),
            ("syntaxVariable", rgb(0x9cdcfe)),
            ("syntaxString", rgb(0xce9178)),
            ("syntaxNumber", rgb(0xb5cea8)),
            ("syntaxType", rgb(0x4ec9b0)),
            ("syntaxOperator", rgb(0xd4d4d4)),
            ("syntaxPunctuation", rgb(0xd4d4d4)),
            ("thinkingOff", rgb(0x505050)),
            ("thinkingMinimal", rgb(0x6e6e6e)),
            ("thinkingLow", rgb(0x5f87af)),
            ("thinkingMedium", rgb(0x81a2be)),
            ("thinkingHigh", rgb(0xb294bb)),
            ("thinkingXhigh", rgb(0xd183e8)),
            ("thinkingMax", rgb(0xff5fff)),
            ("bashMode", rgb(0xb5bd68)),
        ];
        for (name, color) in expected {
            let actual = dark_token(&t, name);
            assert_eq!(actual, *color, "dark token {name} mismatch");
        }
    }

    /// Light theme tokens — the values the TS `light.json` resolves to.
    #[test]
    fn light_palette_matches_v0_82_1_light_json() {
        let t = Theme::light();
        let expected: &[(&str, Color)] = &[
            ("accent", rgb(0x5a8080)),
            ("border", rgb(0x547da7)),
            ("borderAccent", rgb(0x5a8080)),
            ("borderMuted", rgb(0xb0b0b0)),
            ("success", rgb(0x588458)),
            ("error", rgb(0xaa5555)),
            ("warning", rgb(0x9a7326)),
            ("muted", rgb(0x6c6c6c)),
            ("dim", rgb(0x767676)),
            ("text", rgb(0x1f2328)),
            ("thinkingText", rgb(0x6c6c6c)),
            ("selectedBg", rgb(0xd0d0e0)),
            ("userMessageBg", rgb(0xe8e8e8)),
            ("userMessageText", rgb(0x1f2328)),
            ("customMessageBg", rgb(0xede7f6)),
            ("customMessageText", rgb(0x1f2328)),
            ("customMessageLabel", rgb(0x7e57c2)),
            ("toolPendingBg", rgb(0xe8e8f0)),
            ("toolSuccessBg", rgb(0xe8f0e8)),
            ("toolErrorBg", rgb(0xf0e8e8)),
            ("toolTitle", rgb(0x1f2328)),
            ("toolOutput", rgb(0x6c6c6c)),
            ("mdHeading", rgb(0x9a7326)),
            ("mdLink", rgb(0x547da7)),
            ("mdLinkUrl", rgb(0x767676)),
            ("mdCode", rgb(0x5a8080)),
            ("mdCodeBlock", rgb(0x588458)),
            ("mdCodeBlockBorder", rgb(0x6c6c6c)),
            ("mdQuote", rgb(0x6c6c6c)),
            ("mdQuoteBorder", rgb(0x6c6c6c)),
            ("mdHr", rgb(0x6c6c6c)),
            ("mdListBullet", rgb(0x588458)),
            ("toolDiffAdded", rgb(0x588458)),
            ("toolDiffRemoved", rgb(0xaa5555)),
            ("toolDiffContext", rgb(0x6c6c6c)),
            ("syntaxComment", rgb(0x008000)),
            ("syntaxKeyword", rgb(0x0000ff)),
            ("syntaxFunction", rgb(0x795e26)),
            ("syntaxVariable", rgb(0x001080)),
            ("syntaxString", rgb(0xa31515)),
            ("syntaxNumber", rgb(0x098658)),
            ("syntaxType", rgb(0x267f99)),
            ("syntaxOperator", rgb(0x000000)),
            ("syntaxPunctuation", rgb(0x000000)),
            ("thinkingOff", rgb(0xb0b0b0)),
            ("thinkingMinimal", rgb(0x767676)),
            ("thinkingLow", rgb(0x547da7)),
            ("thinkingMedium", rgb(0x5a8080)),
            ("thinkingHigh", rgb(0x875f87)),
            ("thinkingXhigh", rgb(0x8b008b)),
            ("thinkingMax", rgb(0xaf005f)),
            ("bashMode", rgb(0x588458)),
        ];
        for (name, color) in expected {
            let actual = dark_token(&t, name);
            assert_eq!(actual, *color, "light token {name} mismatch");
        }
    }

    #[test]
    fn thinking_level_color_maps_each_level() {
        let t = Theme::default();
        assert_eq!(t.thinking_level_color("off"), THINKING_OFF);
        assert_eq!(t.thinking_level_color("minimal"), THINKING_MINIMAL);
        assert_eq!(t.thinking_level_color("low"), THINKING_LOW);
        assert_eq!(t.thinking_level_color("medium"), THINKING_MEDIUM);
        assert_eq!(t.thinking_level_color("high"), THINKING_HIGH);
        assert_eq!(t.thinking_level_color("xhigh"), THINKING_XHIGH);
        assert_eq!(t.thinking_level_color("max"), THINKING_MAX);
        // Unknown level falls back to off, matching the TS `default` arm.
        assert_eq!(t.thinking_level_color("bogus"), THINKING_OFF);
    }

    /// Resolve a token name the same way the TS `Theme` does, so the test
    /// above enumerates the exact TS token set rather than Rust field names.
    fn dark_token(t: &Theme, name: &str) -> Color {
        match name {
            "accent" => t.accent,
            "border" => t.border,
            "borderAccent" => t.border_accent,
            "borderMuted" => t.border_muted,
            "success" => t.success,
            "error" => t.error,
            "warning" => t.warning,
            "muted" => t.muted,
            "dim" => t.dim,
            "text" => t.text,
            "thinkingText" => t.thinking_text,
            "selectedBg" => t.selected_bg,
            "userMessageBg" => t.user_message_bg,
            "userMessageText" => t.user_message_text,
            "customMessageBg" => t.custom_message_bg,
            "customMessageText" => t.custom_message_text,
            "customMessageLabel" => t.custom_message_label,
            "toolPendingBg" => t.tool_pending_bg,
            "toolSuccessBg" => t.tool_success_bg,
            "toolErrorBg" => t.tool_error_bg,
            "toolTitle" => t.tool_title,
            "toolOutput" => t.tool_output,
            "mdHeading" => t.md_heading,
            "mdLink" => t.md_link,
            "mdLinkUrl" => t.md_link_url,
            "mdCode" => t.md_code,
            "mdCodeBlock" => t.md_code_block,
            "mdCodeBlockBorder" => t.md_code_block_border,
            "mdQuote" => t.md_quote,
            "mdQuoteBorder" => t.md_quote_border,
            "mdHr" => t.md_hr,
            "mdListBullet" => t.md_list_bullet,
            "toolDiffAdded" => t.diff_added,
            "toolDiffRemoved" => t.diff_removed,
            "toolDiffContext" => t.diff_context,
            "syntaxComment" => t.syntax_comment,
            "syntaxKeyword" => t.syntax_keyword,
            "syntaxFunction" => t.syntax_function,
            "syntaxVariable" => t.syntax_variable,
            "syntaxString" => t.syntax_string,
            "syntaxNumber" => t.syntax_number,
            "syntaxType" => t.syntax_type,
            "syntaxOperator" => t.syntax_operator,
            "syntaxPunctuation" => t.syntax_punctuation,
            "thinkingOff" => t.thinking_off,
            "thinkingMinimal" => t.thinking_minimal,
            "thinkingLow" => t.thinking_low,
            "thinkingMedium" => t.thinking_medium,
            "thinkingHigh" => t.thinking_high,
            "thinkingXhigh" => t.thinking_xhigh,
            "thinkingMax" => t.thinking_max,
            "bashMode" => t.bash_mode,
            other => panic!("unknown theme token in test: {other}"),
        }
    }
}
