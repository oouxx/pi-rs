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
}
