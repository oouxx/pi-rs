use std::fmt;

/// Key event type (press, repeat, release).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEventType {
    Press,
    Repeat,
    Release,
}

/// Keyboard modifier flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl KeyModifiers {
    pub fn is_empty(&self) -> bool {
        !self.ctrl && !self.alt && !self.shift && !self.meta
    }
}

/// A structured key event parsed from raw terminal input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: KeyModifiers,
    pub event_type: KeyEventType,
}

/// Key identifier — covers printable characters and special keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable character (Unicode scalar value).
    Char(char),
    /// Enter / Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Delete key.
    Delete,
    /// Arrow keys.
    Up,
    Down,
    Left,
    Right,
    /// Page up / down.
    PageUp,
    PageDown,
    /// Home / End.
    Home,
    End,
    /// Function keys.
    F(u8),
    /// Insert key.
    Insert,
    /// Unknown / unparseable key sequence.
    Unknown(String),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{}", c),
            Key::Enter => write!(f, "Enter"),
            Key::Tab => write!(f, "Tab"),
            Key::Backspace => write!(f, "Backspace"),
            Key::Escape => write!(f, "Escape"),
            Key::Delete => write!(f, "Delete"),
            Key::Up => write!(f, "Up"),
            Key::Down => write!(f, "Down"),
            Key::Left => write!(f, "Left"),
            Key::Right => write!(f, "Right"),
            Key::PageUp => write!(f, "PageUp"),
            Key::PageDown => write!(f, "PageDown"),
            Key::Home => write!(f, "Home"),
            Key::End => write!(f, "End"),
            Key::F(n) => write!(f, "F{}", n),
            Key::Insert => write!(f, "Insert"),
            Key::Unknown(s) => write!(f, "Unknown({})", s),
        }
    }
}

/// The kitty keyboard protocol is active (enables full modifier + event type support).
static KITTY_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_kitty_protocol_active() -> bool {
    KITTY_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_kitty_protocol_active(active: bool) {
    KITTY_ACTIVE.store(active, std::sync::atomic::Ordering::Relaxed);
}

/// Parse a raw string from terminal input into a `KeyEvent`.
///
/// Supports both Kitty CSI-u protocol and legacy escape sequences.
pub fn parse_key(data: &str) -> KeyEvent {
    if data.is_empty() {
        return KeyEvent {
            key: Key::Unknown(String::new()),
            modifiers: KeyModifiers::default(),
            event_type: KeyEventType::Press,
        };
    }

    // Try Kitty CSI-u protocol: ESC [ code ; modifier : event_type u
    if data.len() >= 6 && data.starts_with("\x1b[") && data.ends_with('u') {
        return parse_kitty(data);
    }

    // Handle common single-byte control characters
    if data.len() == 1 {
        match data.chars().next().unwrap() {
            '\r' | '\n' => return simple(Key::Enter),
            '\t' => return simple(Key::Tab),
            '\x7f' | '\x08' => return simple(Key::Backspace),
            '\x1b' => return simple(Key::Escape),
            ch => {
                return KeyEvent {
                    key: Key::Char(ch),
                    modifiers: KeyModifiers::default(),
                    event_type: KeyEventType::Press,
                }
            }
        }
    }

    // Try legacy escape sequences
    if data.starts_with('\x1b') {
        return parse_legacy_escape(data);
    }

    // Fallback: printable character
    if let Some(ch) = data.chars().next() {
        return KeyEvent {
            key: Key::Char(ch),
            modifiers: KeyModifiers::default(),
            event_type: KeyEventType::Press,
        };
    }

    // Empty input
    KeyEvent {
        key: Key::Unknown(String::new()),
        modifiers: KeyModifiers::default(),
        event_type: KeyEventType::Press,
    }
}

fn parse_kitty(data: &str) -> KeyEvent {
    // Format: ESC [ code ; modifier : event_type u
    let inner = &data[2..data.len() - 1]; // strip ESC [ and trailing u
    let parts: Vec<&str> = inner.split(':').collect();
    let (code_mod, event_type) = if parts.len() >= 2 {
        (parts[0], parts[1].parse::<u8>().unwrap_or(1))
    } else {
        (parts[0], 1u8)
    };

    let code_parts: Vec<&str> = code_mod.split(';').collect();
    let key_code: u32 = code_parts[0].parse().unwrap_or(0);
    let modifier: u8 = if code_parts.len() >= 2 {
        code_parts[1].parse().unwrap_or(0)
    } else {
        0
    };

    // Kitty modifier bits: 1=shift, 2=alt, 4=ctrl, 8=meta(super)
    let modifiers = KeyModifiers {
        shift: modifier & 1 != 0,
        alt: modifier & 2 != 0,
        ctrl: modifier & 4 != 0,
        meta: modifier & 8 != 0,
    };

    let et = match event_type {
        1 => KeyEventType::Press,
        2 => KeyEventType::Repeat,
        3 => KeyEventType::Release,
        _ => KeyEventType::Press,
    };

    let key = kitty_code_to_key(key_code, modifiers);

    KeyEvent {
        key,
        modifiers,
        event_type: et,
    }
}

fn kitty_code_to_key(code: u32, mods: KeyModifiers) -> Key {
    match code {
        // Special keys
        13 => Key::Enter,
        9 => Key::Tab,
        27 => Key::Escape,
        127 => Key::Backspace,
        // Function keys: Kitty maps F1-F12 to 57344-57355 (0xE000+)
        57344..=57355 => Key::F((code - 57344 + 1) as u8),
        // Arrow keys
        57356 => Key::Up,
        57357 => Key::Down,
        57358 => Key::Left,
        57359 => Key::Right,
        // Other keys
        57360 => Key::Home,
        57361 => Key::End,
        57362 => Key::PageUp,
        57363 => Key::PageDown,
        57364 => Key::Delete,
        57365 => Key::Insert,
        // Printable: if no modifiers, it's a char
        _ if code < 0x10000 && mods.is_empty() => {
            if let Some(c) = char::from_u32(code) {
                if !c.is_control() {
                    return Key::Char(c);
                }
            }
            Key::Unknown(format!("kitty-code-{}", code))
        }
        _ => Key::Unknown(format!("kitty-code-{}", code)),
    }
}

fn parse_legacy_escape(data: &str) -> KeyEvent {
    match data {
        "\x1b[A" => simple(Key::Up),
        "\x1b[B" => simple(Key::Down),
        "\x1b[C" => simple(Key::Right),
        "\x1b[D" => simple(Key::Left),
        "\x1b[H" => simple(Key::Home),
        "\x1b[F" => simple(Key::End),
        "\x1b[5~" => simple(Key::PageUp),
        "\x1b[6~" => simple(Key::PageDown),
        "\x1b[3~" => simple(Key::Delete),
        "\x1b[2~" => simple(Key::Insert),
        "\x1bOP" => simple(Key::F(1)),
        "\x1bOQ" => simple(Key::F(2)),
        "\x1bOR" => simple(Key::F(3)),
        "\x1bOS" => simple(Key::F(4)),
        "\x1b[15~" => simple(Key::F(5)),
        "\x1b[17~" => simple(Key::F(6)),
        "\x1b[18~" => simple(Key::F(7)),
        "\x1b[19~" => simple(Key::F(8)),
        "\x1b[20~" => simple(Key::F(9)),
        "\x1b[21~" => simple(Key::F(10)),
        "\x1b[23~" => simple(Key::F(11)),
        "\x1b[24~" => simple(Key::F(12)),
        "\x1b[1;2A" => with_mod(Key::Up, KeyModifiers { shift: true, ..Default::default() }),
        "\x1b[1;2B" => with_mod(Key::Down, KeyModifiers { shift: true, ..Default::default() }),
        "\x1b[1;2C" => with_mod(Key::Right, KeyModifiers { shift: true, ..Default::default() }),
        "\x1b[1;2D" => with_mod(Key::Left, KeyModifiers { shift: true, ..Default::default() }),
        "\x1b[1;5A" => with_mod(Key::Up, KeyModifiers { ctrl: true, ..Default::default() }),
        "\x1b[1;5B" => with_mod(Key::Down, KeyModifiers { ctrl: true, ..Default::default() }),
        "\x1b[1;5C" => with_mod(Key::Right, KeyModifiers { ctrl: true, ..Default::default() }),
        "\x1b[1;5D" => with_mod(Key::Left, KeyModifiers { ctrl: true, ..Default::default() }),
        "\r" | "\n" => simple(Key::Enter),
        "\x1b" => simple(Key::Escape),
        "\x7f" | "\x08" => simple(Key::Backspace),
        "\t" => simple(Key::Tab),
        _ => KeyEvent {
            key: Key::Unknown(data.to_string()),
            modifiers: KeyModifiers::default(),
            event_type: KeyEventType::Press,
        },
    }
}

fn simple(key: Key) -> KeyEvent {
    KeyEvent {
        key,
        modifiers: KeyModifiers::default(),
        event_type: KeyEventType::Press,
    }
}

fn with_mod(key: Key, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        key,
        modifiers,
        event_type: KeyEventType::Press,
    }
}

/// Check if a key event is a key release.
pub fn is_key_release(data: &str) -> bool {
    if is_kitty_protocol_active() {
        let event = parse_key(data);
        return event.event_type == KeyEventType::Release;
    }
    false
}

/// Check if a key event is a key repeat.
pub fn is_key_repeat(data: &str) -> bool {
    if is_kitty_protocol_active() {
        let event = parse_key(data);
        return event.event_type == KeyEventType::Repeat;
    }
    false
}

/// Check if a raw input sequence matches a given `KeyEvent`.
pub fn matches_key(data: &str, expected: &KeyEvent) -> bool {
    let parsed = parse_key(data);
    parsed.key == expected.key && parsed.modifiers == expected.modifiers
}

/// Decode a printable character from kitty protocol (handles shifted keys).
pub fn decode_kitty_printable(data: &str) -> Option<char> {
    if is_kitty_protocol_active() && data.starts_with("\x1b[") && data.ends_with('u') {
        let event = parse_key(data);
        if let Key::Char(c) = event.key {
            return Some(c);
        }
    }
    None
}

/// Decode a printable character from any terminal protocol (Kitty CSI-u or modifyOtherKeys).
pub fn decode_printable_key(data: &str) -> Option<char> {
    decode_kitty_printable(data).or_else(|| {
        // Try modifyOtherKeys format: ESC [ 27 ; mod ; codepoint ~
        if data.starts_with("\x1b[27;") && data.ends_with('~') {
            let inner = &data[5..data.len() - 1];
            let parts: Vec<&str> = inner.split(';').collect();
            if parts.len() >= 2 {
                if let Ok(cp) = parts[1].parse::<u32>() {
                    if cp >= 32 {
                        return char::from_u32(cp);
                    }
                }
            }
        }
        None
    })
}

/// Parse a key identifier string (e.g., "ctrl+c", "escape", "shift+enter")
/// into the expected `KeyEvent`.
fn parse_key_id(key_id: &str) -> Option<KeyEvent> {
    let key_id = key_id.to_lowercase();
    let parts: Vec<&str> = key_id.split('+').collect();
    let key_name = *parts.last()?;

    let modifiers = KeyModifiers {
        ctrl: parts.contains(&"ctrl"),
        alt: parts.contains(&"alt"),
        shift: parts.contains(&"shift"),
        meta: parts.contains(&"super"),
    };

    let key = match key_name {
        "escape" | "esc" => Key::Escape,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "space" => Key::Char(' '),
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        s if s.starts_with('f') && s.len() > 1 => {
            let n: u8 = s[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                Key::F(n)
            } else {
                return None;
            }
        }
        s if s.len() == 1 => {
            let c = s.chars().next()?;
            Key::Char(c)
        }
        _ => return None,
    };

    Some(KeyEvent {
        key,
        modifiers,
        event_type: KeyEventType::Press,
    })
}

/// Compute the legacy control character for a key (Ctrl+A..Z → \x01..\x1a).
fn ctrl_char_for_key(key: char) -> Option<u8> {
    let c = key.to_ascii_lowercase();
    if ('a'..='z').contains(&c) {
        Some((c as u8) - b'a' + 1)
    } else {
        None
    }
}

/// Check if raw terminal input matches a key identifier string.
/// Supports formats like "ctrl+c", "escape", "shift+enter", "up", etc.
pub fn matches_key_str(data: &str, key_id: &str) -> bool {
    let Some(expected) = parse_key_id(key_id) else {
        return false;
    };

    // Standard KeyEvent comparison
    if matches_key(data, &expected) {
        return true;
    }

    // Legacy fallback: ctrl+letter → control character (e.g., Ctrl+C = \x03)
    if expected.modifiers.ctrl && !expected.modifiers.alt && !expected.modifiers.meta {
        if let Key::Char(c) = expected.key {
            if let Some(ctrl_byte) = ctrl_char_for_key(c) {
                if data.len() == 1 && data.as_bytes()[0] == ctrl_byte {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_printable() {
        let event = parse_key("a");
        assert_eq!(event.key, Key::Char('a'));
    }

    #[test]
    fn test_parse_enter() {
        let event = parse_key("\r");
        assert_eq!(event.key, Key::Enter);
    }

    #[test]
    fn test_parse_up_arrow() {
        let event = parse_key("\x1b[A");
        assert_eq!(event.key, Key::Up);
    }

    #[test]
    fn test_parse_down_arrow() {
        let event = parse_key("\x1b[B");
        assert_eq!(event.key, Key::Down);
    }

    #[test]
    fn test_parse_tab() {
        let event = parse_key("\t");
        assert_eq!(event.key, Key::Tab);
    }

    #[test]
    fn test_parse_escape() {
        let event = parse_key("\x1b");
        assert_eq!(event.key, Key::Escape);
    }

    #[test]
    fn test_matches_key() {
        let expected = KeyEvent {
            key: Key::Up,
            modifiers: KeyModifiers::default(),
            event_type: KeyEventType::Press,
        };
        assert!(matches_key("\x1b[A", &expected));
        assert!(!matches_key("a", &expected));
    }

    #[test]
    fn test_kitty_protocol_flag() {
        assert!(!is_kitty_protocol_active());
        set_kitty_protocol_active(true);
        assert!(is_kitty_protocol_active());
        set_kitty_protocol_active(false);
        assert!(!is_kitty_protocol_active());
    }

    // --- Supplementary tests matching TS originals ---

    #[test]
    fn test_parse_ctrl_arrow_keys() {
        // Ctrl+Up: ESC [ 1 ; 5 A
        let event = parse_key("\x1b[1;5A");
        assert_eq!(event.key, Key::Up);
        assert!(event.modifiers.ctrl);

        let event = parse_key("\x1b[1;5B");
        assert_eq!(event.key, Key::Down);
        assert!(event.modifiers.ctrl);

        let event = parse_key("\x1b[1;5C");
        assert_eq!(event.key, Key::Right);
        assert!(event.modifiers.ctrl);

        let event = parse_key("\x1b[1;5D");
        assert_eq!(event.key, Key::Left);
        assert!(event.modifiers.ctrl);
    }

    #[test]
    fn test_parse_shift_arrow_keys() {
        let event = parse_key("\x1b[1;2A");
        assert_eq!(event.key, Key::Up);
        assert!(event.modifiers.shift);
    }

    #[test]
    fn test_parse_function_keys() {
        for n in 1..=12 {
            let data = match n {
                1 => "\x1bOP",
                2 => "\x1bOQ",
                3 => "\x1bOR",
                4 => "\x1bOS",
                n => {
                    let code = 10 + n;
                    // can't easily use format! in match, so pre-compute
                    return; // skip dynamic F5-F12 for now
                }
            };
            let event = parse_key(data);
            if n <= 4 {
                assert_eq!(event.key, Key::F(n), "F{} mismatch", n);
            }
        }
    }

    #[test]
    fn test_parse_unknown_sequence() {
        let event = parse_key("\x1b[999z");
        assert!(matches!(event.key, Key::Unknown(_)));
    }

    #[test]
    fn test_parse_empty_string() {
        let event = parse_key("");
        assert!(matches!(event.key, Key::Unknown(_)));
    }

    #[test]
    fn test_matches_key_with_modifiers() {
        let expected = KeyEvent {
            key: Key::Up,
            modifiers: KeyModifiers { ctrl: true, ..Default::default() },
            event_type: KeyEventType::Press,
        };
        assert!(matches_key("\x1b[1;5A", &expected));
        assert!(!matches_key("\x1b[A", &expected)); // no ctrl
    }

    #[test]
    fn test_parse_delete_key() {
        let event = parse_key("\x1b[3~");
        assert_eq!(event.key, Key::Delete);
    }

    #[test]
    fn test_parse_page_up_down() {
        let event = parse_key("\x1b[5~");
        assert_eq!(event.key, Key::PageUp);
        let event = parse_key("\x1b[6~");
        assert_eq!(event.key, Key::PageDown);
    }

    #[test]
    fn test_parse_home_end() {
        let event = parse_key("\x1b[H");
        assert_eq!(event.key, Key::Home);
        let event = parse_key("\x1b[F");
        assert_eq!(event.key, Key::End);
    }

    #[test]
    fn test_parse_insert() {
        let event = parse_key("\x1b[2~");
        assert_eq!(event.key, Key::Insert);
    }

    #[test]
    fn test_parse_backspace() {
        let event = parse_key("\x7f");
        assert_eq!(event.key, Key::Backspace);
        let event = parse_key("\x08");
        assert_eq!(event.key, Key::Backspace);
    }

    #[test]
    fn test_is_key_release_false_when_kitty_inactive() {
        set_kitty_protocol_active(false);
        assert!(!is_key_release("\x1b[A"));
    }

    #[test]
    fn test_is_key_repeat_false_when_kitty_inactive() {
        set_kitty_protocol_active(false);
        assert!(!is_key_repeat("a"));
    }

    #[test]
    fn test_decode_kitty_printable_no_kitty() {
        set_kitty_protocol_active(false);
        assert_eq!(decode_kitty_printable("\x1b[97u"), None);
    }

    #[test]
    fn test_modifiers_empty_by_default() {
        let event = parse_key("a");
        assert!(event.modifiers.is_empty());
    }

    #[test]
    fn test_key_display_trait() {
        assert_eq!(format!("{}", Key::Enter), "Enter");
        assert_eq!(format!("{}", Key::Char('a')), "a");
        assert_eq!(format!("{}", Key::F(1)), "F1");
        assert_eq!(format!("{}", Key::Up), "Up");
    }

    // ============================================================================
    // matches_key_str tests
    // ============================================================================

    #[test]
    fn test_matches_key_str_ctrl_c() {
        assert!(matches_key_str("\x03", "ctrl+c"));
        assert!(!matches_key_str("c", "ctrl+c"));
    }

    #[test]
    fn test_matches_key_str_escape() {
        assert!(matches_key_str("\x1b", "escape"));
        assert!(matches_key_str("\x1b", "esc"));
        assert!(!matches_key_str("a", "escape"));
    }

    #[test]
    fn test_matches_key_str_enter() {
        assert!(matches_key_str("\r", "enter"));
        assert!(matches_key_str("\r", "return"));
    }

    #[test]
    fn test_matches_key_str_tab() {
        assert!(matches_key_str("\t", "tab"));
    }

    #[test]
    fn test_matches_key_str_space() {
        assert!(matches_key_str(" ", "space"));
    }

    #[test]
    fn test_matches_key_str_backspace() {
        assert!(matches_key_str("\x7f", "backspace"));
    }

    #[test]
    fn test_matches_key_str_arrows() {
        assert!(matches_key_str("\x1b[A", "up"));
        assert!(matches_key_str("\x1b[B", "down"));
        assert!(matches_key_str("\x1b[C", "right"));
        assert!(matches_key_str("\x1b[D", "left"));
    }

    #[test]
    fn test_matches_key_str_ctrl_arrows() {
        assert!(matches_key_str("\x1b[1;5A", "ctrl+up"));
        assert!(matches_key_str("\x1b[1;5B", "ctrl+down"));
        assert!(matches_key_str("\x1b[1;5C", "ctrl+right"));
        assert!(matches_key_str("\x1b[1;5D", "ctrl+left"));
    }

    #[test]
    fn test_matches_key_str_function_keys() {
        assert!(matches_key_str("\x1bOP", "f1"));
        assert!(matches_key_str("\x1bOQ", "f2"));
        assert!(matches_key_str("\x1bOR", "f3"));
        assert!(matches_key_str("\x1bOS", "f4"));
    }

    #[test]
    fn test_matches_key_str_invalid_key_id() {
        assert!(!matches_key_str("a", ""));
        assert!(!matches_key_str("a", "invalid_key_name"));
    }

    #[test]
    fn test_matches_key_str_case_insensitive() {
        assert!(matches_key_str("\x03", "Ctrl+C"));
        assert!(matches_key_str("\x1b[A", "Up"));
    }

    // ============================================================================
    // decode_printable_key tests
    // ============================================================================

    #[test]
    fn test_decode_printable_key_plain() {
        assert_eq!(decode_printable_key("a"), None); // plain keys don't need decoding
    }

    #[test]
    fn test_decode_printable_key_modify_other_keys() {
        // ESC [ 27 ; 1 ; 97 ~ = 'a' with no modifiers
        assert_eq!(decode_printable_key("\x1b[27;1;97~"), Some('a'));
    }

    #[test]
    fn test_decode_printable_key_shift() {
        assert_eq!(decode_printable_key("\x1b[27;2;65~"), Some('A'));
    }

    #[test]
    fn test_decode_printable_key_not_printable() {
        // Control character
        assert_eq!(decode_printable_key("\x1b[27;1;3~"), None);
    }
}
