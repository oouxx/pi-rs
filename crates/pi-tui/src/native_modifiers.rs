#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierKey {
    Shift,
    Command,
    Control,
    Option,
}

impl ModifierKey {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "shift" => Some(Self::Shift),
            "command" => Some(Self::Command),
            "control" => Some(Self::Control),
            "option" => Some(Self::Option),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Shift => "shift",
            Self::Command => "command",
            Self::Control => "control",
            Self::Option => "option",
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::ModifierKey;

    const SHIFT_MASK: u64 = 0x0002_0000;
    const CONTROL_MASK: u64 = 0x0004_0000;
    const ALT_MASK: u64 = 0x0008_0000;
    const CMD_MASK: u64 = 0x0010_0000;

    type CGEventFlags = u64;
    type CGEventSourceStateID = i32;

    const KCG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE: CGEventSourceStateID = 0;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceFlagsState(stateID: CGEventSourceStateID) -> CGEventFlags;
    }

    pub fn is_modifier_pressed_raw(key: &ModifierKey) -> bool {
        let flags = unsafe { CGEventSourceFlagsState(KCG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE) };
        let mask = match key {
            ModifierKey::Shift => SHIFT_MASK,
            ModifierKey::Command => CMD_MASK,
            ModifierKey::Control => CONTROL_MASK,
            ModifierKey::Option => ALT_MASK,
        };
        flags & mask != 0
    }
}

#[cfg(not(target_os = "macos"))]
mod macos {
    use super::ModifierKey;
    pub fn is_modifier_pressed_raw(_key: &ModifierKey) -> bool {
        false
    }
}

pub fn is_native_modifier_pressed(key: &ModifierKey) -> bool {
    macos::is_modifier_pressed_raw(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modifier_key_from_str() {
        assert_eq!(ModifierKey::from_str("shift"), Some(ModifierKey::Shift));
        assert_eq!(ModifierKey::from_str("command"), Some(ModifierKey::Command));
        assert_eq!(ModifierKey::from_str("control"), Some(ModifierKey::Control));
        assert_eq!(ModifierKey::from_str("option"), Some(ModifierKey::Option));
        assert_eq!(ModifierKey::from_str("invalid"), None);
    }

    #[test]
    fn test_modifier_key_as_str() {
        assert_eq!(ModifierKey::Shift.as_str(), "shift");
        assert_eq!(ModifierKey::Command.as_str(), "command");
        assert_eq!(ModifierKey::Control.as_str(), "control");
        assert_eq!(ModifierKey::Option.as_str(), "option");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn test_is_modifier_pressed_always_false_on_non_macos() {
        assert!(!is_native_modifier_pressed(&ModifierKey::Shift));
        assert!(!is_native_modifier_pressed(&ModifierKey::Command));
        assert!(!is_native_modifier_pressed(&ModifierKey::Control));
        assert!(!is_native_modifier_pressed(&ModifierKey::Option));
    }
}
