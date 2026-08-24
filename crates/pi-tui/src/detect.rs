//! Terminal theme auto-detection — Rust port of TS `theme.ts`
//! (`detectTerminalBackgroundFromEnv` / `detectTerminalBackgroundTheme`) and
//! the OSC 11 query in `tui.ts` (`queryTerminalBackgroundColor`).
//!
//! Detection order (matches TS): OSC 11 terminal background query → `COLORFGBG`
//! env var → dark fallback. Luminance threshold 0.5 (TS `getThemeForRgbColor`).

use std::time::{Duration, Instant};

/// Terminal color scheme (TS `TerminalTheme`: "dark" | "light").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTheme {
    Dark,
    Light,
}

impl TerminalTheme {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            TerminalTheme::Dark => "dark",
            TerminalTheme::Light => "light",
        }
    }
}

/// Detection source (TS `TerminalThemeDetection.source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeDetectionSource {
    /// OSC 11 query replied with an RGB background color.
    TerminalBackground,
    /// `COLORFGBG` env var carried a background color index.
    ColorFgBg,
    /// No hint found — fell back to dark.
    Fallback,
}

/// Result of one detection pass (TS `TerminalThemeDetection`).
#[derive(Debug, Clone)]
pub struct TerminalThemeDetection {
    pub theme: TerminalTheme,
    pub source: ThemeDetectionSource,
    /// Human-readable detail (TS `detail`, e.g. "OSC 11 background rgb(...)").
    pub detail: String,
}

impl TerminalThemeDetection {
    /// TS `confidence`: high for terminal background / COLORFGBG, low for
    /// the fallback. Only high-confidence detections are persisted to
    /// settings (TS `applyFromSettings`).
    #[must_use]
    pub fn confidence_high(&self) -> bool {
        !matches!(self.source, ThemeDetectionSource::Fallback)
    }
}

fn fallback_detection() -> TerminalThemeDetection {
    TerminalThemeDetection {
        theme: TerminalTheme::Dark,
        source: ThemeDetectionSource::Fallback,
        detail: "no terminal background hint found".to_string(),
    }
}

/// sRGB relative luminance (TS `getRgbColorLuminance`): linearized channels
/// weighted 0.2126/0.7152/0.0722.
#[must_use]
pub fn rgb_luminance(r: u8, g: u8, b: u8) -> f64 {
    let to_linear = |channel: u8| {
        let value = channel as f64 / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * to_linear(r) + 0.7152 * to_linear(g) + 0.0722 * to_linear(b)
}

/// TS `getThemeForRgbColor`: luminance >= 0.5 → light.
#[must_use]
pub fn theme_for_rgb(r: u8, g: u8, b: u8) -> TerminalTheme {
    if rgb_luminance(r, g, b) >= 0.5 {
        TerminalTheme::Light
    } else {
        TerminalTheme::Dark
    }
}

/// TS `ansi256ToHex` — xterm 256-color palette → RGB.
#[must_use]
pub fn ansi256_to_rgb(index: u8) -> (u8, u8, u8) {
    // Basic colors (0-15): approximate common terminal values.
    const BASIC: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0x80, 0x00, 0x00),
        (0x00, 0x80, 0x00),
        (0x80, 0x80, 0x00),
        (0x00, 0x00, 0x80),
        (0x80, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0xc0, 0xc0, 0xc0),
        (0x80, 0x80, 0x80),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x00, 0x00, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    if index < 16 {
        return BASIC[index as usize];
    }
    // Color cube (16-231): 6x6x6, channels ∈ {0, 55 + 40n}.
    if index < 232 {
        let cube = index - 16;
        let to_channel = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
        let r = to_channel(cube / 36);
        let g = to_channel((cube % 36) / 6);
        let b = to_channel(cube % 6);
        return (r, g, b);
    }
    // Grayscale (232-255): 24 shades.
    let gray = 8 + (index - 232) * 10;
    (gray, gray, gray)
}

/// TS `getColorFgBgBackgroundIndex`: the last 0..=255 integer in the
/// semicolon-separated `COLORFGBG` value is the background color index.
fn color_fg_bg_background_index(colorfgbg: &str) -> Option<u8> {
    let mut found = None;
    for part in colorfgbg.split(';') {
        if let Ok(n) = part.trim().parse::<u16>() {
            if n <= 255 {
                found = Some(n as u8);
            }
        }
    }
    found
}

/// TS `detectTerminalBackgroundFromEnv` (sync): `COLORFGBG` → luminance;
/// fallback dark when the env var is absent/unparseable.
#[must_use]
pub fn detect_theme_from_env_value(colorfgbg: Option<&str>) -> TerminalThemeDetection {
    if let Some(value) = colorfgbg {
        if let Some(bg) = color_fg_bg_background_index(value) {
            let (r, g, b) = ansi256_to_rgb(bg);
            return TerminalThemeDetection {
                theme: theme_for_rgb(r, g, b),
                source: ThemeDetectionSource::ColorFgBg,
                detail: format!("background color index {bg}"),
            };
        }
    }
    fallback_detection()
}

/// `detectTerminalBackgroundFromEnv()` reading the process environment.
#[must_use]
pub fn detect_theme_from_env() -> TerminalThemeDetection {
    detect_theme_from_env_value(std::env::var("COLORFGBG").ok().as_deref())
}

/// TS `parseOscHexChannel`: hex channel (2 or 4 digits) scaled to 0..=255.
fn parse_osc_hex_channel(channel: &str) -> Option<u8> {
    if channel.is_empty() || !channel.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let len = channel.len() as u32;
    let max = 16u64.pow(len).saturating_sub(1);
    if max == 0 {
        return None;
    }
    let value = u64::from_str_radix(channel, 16).ok()?;
    Some((value as f64 / max as f64 * 255.0).round() as u8)
}

/// TS `parseOsc11BackgroundColor` (relaxed to scan a byte buffer): finds the
/// last `ESC ] 11 ; <color> BEL|ESC\` sequence and parses `<color>` as
/// `#rrggbb`, `#rrrrggggbbbb` or `rgb:r/g/b` / `rgba:r/g/b/a`.
#[must_use]
pub fn parse_osc11_background_color(data: &[u8]) -> Option<(u8, u8, u8)> {
    let text = String::from_utf8_lossy(data);
    let start = text.rfind("\x1b]11;")?;
    let body = &text[start + "\x1b]11;".len()..];
    let end = body.find('\x07').or_else(|| body.find("\x1b\\"))?;
    let value = body[..end].trim();

    if let Some(hex) = value.strip_prefix('#') {
        // #rrggbb (2-digit channels) or #rrrrggggbbbb (4-digit channels).
        if hex.len() == 6 {
            return Some((
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            ));
        }
        if hex.len() == 12 {
            return Some((
                parse_osc_hex_channel(&hex[0..4])?,
                parse_osc_hex_channel(&hex[4..8])?,
                parse_osc_hex_channel(&hex[8..12])?,
            ));
        }
        return None;
    }

    let rgb = value
        .strip_prefix("rgb:")
        .or_else(|| value.strip_prefix("rgba:"))?;
    let mut channels = rgb.split('/');
    let r = parse_osc_hex_channel(channels.next()?)?;
    let g = parse_osc_hex_channel(channels.next()?)?;
    let b = parse_osc_hex_channel(channels.next()?)?;
    Some((r, g, b))
}

/// Send `ESC ] 11 ; ? BEL` and read the terminal's reply.
///
/// Must run with stdin in raw mode (a canonical-mode tty only delivers input
/// on newline; the OSC reply ends with BEL) and before any crossterm
/// `EventStream` starts consuming stdin (unparseable OSC replies are dropped
/// there). The fd is flipped to non-blocking for the duration of the query
/// and restored afterwards.
#[cfg(unix)]
pub fn query_terminal_background_color(timeout: Duration) -> Option<(u8, u8, u8)> {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x1b]11;?\x07");
    let _ = stdout.flush();

    let fd = std::io::stdin().as_raw_fd();
    // SAFETY: fcntl on fd 0 is always valid; flags are restored on exit.
    let old_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if old_flags == -1 {
        return None;
    }
    unsafe {
        libc::fcntl(fd, libc::F_SETFL, old_flags | libc::O_NONBLOCK);
    }

    let deadline = Instant::now() + timeout;
    let mut data: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 256];
    loop {
        // SAFETY: tmp is a valid writable buffer of tmp.len() bytes.
        let n = unsafe { libc::read(fd, tmp.as_mut_ptr().cast(), tmp.len()) };
        if n > 0 {
            data.extend_from_slice(&tmp[..n as usize]);
            if data.contains(&0x07) {
                break;
            }
        } else if n == 0 {
            break;
        } else {
            let err = std::io::Error::last_os_error();
            if err.kind() != std::io::ErrorKind::WouldBlock {
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // Restore the original flags so crossterm's raw-mode handling is unaffected.
    unsafe {
        libc::fcntl(fd, libc::F_SETFL, old_flags);
    }

    parse_osc11_background_color(&data)
}

#[cfg(not(unix))]
pub fn query_terminal_background_color(_timeout: Duration) -> Option<(u8, u8, u8)> {
    None
}

/// TS `detectTerminalBackgroundTheme`: OSC 11 query first, then
/// `COLORFGBG`, then the dark fallback.
#[must_use]
pub fn detect_terminal_background_theme(timeout: Duration) -> TerminalThemeDetection {
    if let Some((r, g, b)) = query_terminal_background_color(timeout) {
        return TerminalThemeDetection {
            theme: theme_for_rgb(r, g, b),
            source: ThemeDetectionSource::TerminalBackground,
            detail: format!("OSC 11 background rgb({r}, {g}, {b})"),
        };
    }
    detect_theme_from_env()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn colorfgbg_background_index_takes_last_valid() {
        // Standard COLORFGBG: "fg;bg" — 15 is a light gray, 0 black.
        assert_eq!(color_fg_bg_background_index("15;0"), Some(0));
        assert_eq!(color_fg_bg_background_index("0;15"), Some(15));
        assert_eq!(color_fg_bg_background_index("10;0;231"), Some(231));
        // Invalid values are skipped; nothing valid → None.
        assert_eq!(color_fg_bg_background_index("x;y"), None);
        assert_eq!(color_fg_bg_background_index("300;400"), None);
        assert_eq!(color_fg_bg_background_index(""), None);
    }

    #[test]
    fn env_detection_dark_and_light() {
        // bg index 0 (black) → dark.
        let d = detect_theme_from_env_value(Some("15;0"));
        assert_eq!(d.theme, TerminalTheme::Dark);
        assert!(d.confidence_high());
        assert_eq!(d.source, ThemeDetectionSource::ColorFgBg);
        // bg index 15 (white #ffffff) → light.
        let l = detect_theme_from_env_value(Some("0;15"));
        assert_eq!(l.theme, TerminalTheme::Light);
        // Missing env → dark fallback, low confidence.
        let f = detect_theme_from_env_value(None);
        assert_eq!(f.theme, TerminalTheme::Dark);
        assert!(!f.confidence_high());
        assert_eq!(f.source, ThemeDetectionSource::Fallback);
    }

    #[test]
    fn ansi256_cube_channels() {
        // Index 16: black cube corner.
        assert_eq!(ansi256_to_rgb(16), (0, 0, 0));
        // Index 231: white cube corner (55 + 5*40 = 255).
        assert_eq!(ansi256_to_rgb(231), (255, 255, 255));
        // Index 196: pure red.
        assert_eq!(ansi256_to_rgb(196), (255, 0, 0));
        // Grayscale: 232 → 8, 255 → 8 + 23*10 = 238.
        assert_eq!(ansi256_to_rgb(232), (8, 8, 8));
        assert_eq!(ansi256_to_rgb(255), (238, 238, 238));
        // Basic 15 → white.
        assert_eq!(ansi256_to_rgb(15), (255, 255, 255));
    }

    #[test]
    fn luminance_threshold() {
        // Black → dark, white → light.
        assert_eq!(theme_for_rgb(0, 0, 0), TerminalTheme::Dark);
        assert_eq!(theme_for_rgb(255, 255, 255), TerminalTheme::Light);
        // #f8f8f8 (light gray) — luminance well above 0.5.
        assert_eq!(theme_for_rgb(248, 248, 248), TerminalTheme::Light);
        // Mid gray #808080: luminance ≈ 0.216 → dark.
        assert_eq!(theme_for_rgb(128, 128, 128), TerminalTheme::Dark);
    }

    #[test]
    fn parse_osc11_hex_forms() {
        let d = b"\x1b]11;#1f2328\x07";
        assert_eq!(parse_osc11_background_color(d), Some((0x1f, 0x23, 0x28)));
        // 4-digit channels: #ffffffffffff → white.
        let d = b"\x1b]11;#ffffffffffff\x07";
        assert_eq!(parse_osc11_background_color(d), Some((255, 255, 255)));
    }

    #[test]
    fn parse_osc11_rgb_forms() {
        let d = b"\x1b]11;rgb:0000/0000/0000\x07";
        assert_eq!(parse_osc11_background_color(d), Some((0, 0, 0)));
        let d = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\";
        assert_eq!(parse_osc11_background_color(d), Some((255, 255, 255)));
        let d = b"\x1b]11;rgba:1fff/1fff/1fff/ffff\x07";
        // 0x1fff / 0xffff * 255 = 31.87 → round 32 (TS parseOscHexChannel).
        assert_eq!(parse_osc11_background_color(d), Some((32, 32, 32)));
    }

    #[test]
    fn parse_osc11_scans_buffer_for_sequence() {
        // Junk bytes before the response (stray keystrokes) must not break parsing.
        let d = b"x\x1b]11;#1f2328\x07";
        assert_eq!(parse_osc11_background_color(d), Some((0x1f, 0x23, 0x28)));
        // Missing terminator / no query → None.
        assert_eq!(parse_osc11_background_color(b"abc"), None);
        assert_eq!(parse_osc11_background_color(b"\x1b]11;#123456"), None);
    }
}
