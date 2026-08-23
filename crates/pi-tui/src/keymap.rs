//! Keymap — configurable keybinding definitions.
//!
//! Mirrors plans.md Section 7 "键位设计".
//! Allows users to customize keybindings for common actions.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Named actions the TUI can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    SubmitMessage,
    AbortStream,
    Quit,
    ScrollUp,
    ScrollDown,
    ScrollToTop,
    ScrollToBottom,
    CompleteNext,
    CompletePrev,
    CompleteConfirm,
    CompleteCancel,
    DialogConfirm,
    DialogNext,
    DialogPrev,
    DialogDismiss,
}

/// A keybinding: a key + modifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyBind {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBind {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn matches(&self, event: &KeyEvent) -> bool {
        self.code == event.code && self.modifiers == event.modifiers
    }
}

/// A map from Action to list of keybindings.
#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: HashMap<Action, Vec<KeyBind>>,
}

impl Keymap {
    /// Default keybindings matching plans.md Section 7.
    pub fn default() -> Self {
        let mut bindings: HashMap<Action, Vec<KeyBind>> = HashMap::new();

        bindings.insert(Action::SubmitMessage, vec![
            KeyBind::new(KeyCode::Enter, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::AbortStream, vec![
            KeyBind::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ]);
        bindings.insert(Action::Quit, vec![
            KeyBind::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            KeyBind::new(KeyCode::Esc, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::ScrollUp, vec![
            KeyBind::new(KeyCode::PageUp, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::ScrollDown, vec![
            KeyBind::new(KeyCode::PageDown, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::ScrollToTop, vec![
            KeyBind::new(KeyCode::Home, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::ScrollToBottom, vec![
            KeyBind::new(KeyCode::End, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::CompleteNext, vec![
            KeyBind::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyBind::new(KeyCode::Down, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::CompletePrev, vec![
            KeyBind::new(KeyCode::Up, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::CompleteConfirm, vec![
            KeyBind::new(KeyCode::Enter, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::CompleteCancel, vec![
            KeyBind::new(KeyCode::Esc, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::DialogConfirm, vec![
            KeyBind::new(KeyCode::Enter, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::DialogNext, vec![
            KeyBind::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyBind::new(KeyCode::Right, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::DialogPrev, vec![
            KeyBind::new(KeyCode::Left, KeyModifiers::NONE),
        ]);
        bindings.insert(Action::DialogDismiss, vec![
            KeyBind::new(KeyCode::Esc, KeyModifiers::NONE),
        ]);

        Self { bindings }
    }

    /// Look up the first matching action for a key event.
    pub fn action_for(&self, event: &KeyEvent) -> Option<Action> {
        for (action, keys) in &self.bindings {
            if keys.iter().any(|k| k.matches(event)) {
                return Some(*action);
            }
        }
        None
    }

    /// Check if a key event triggers the given action.
    pub fn triggers(&self, event: &KeyEvent, action: Action) -> bool {
        self.bindings
            .get(&action)
            .map(|keys| keys.iter().any(|k| k.matches(event)))
            .unwrap_or(false)
    }

    /// Add a custom keybinding.
    pub fn bind(&mut self, action: Action, key: KeyBind) {
        self.bindings.entry(action).or_default().push(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_bindings() {
        let km = Keymap::default();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(km.action_for(&ctrl_c), Some(Action::AbortStream));
    }

    #[test]
    fn test_abort_binding() {
        let km = Keymap::default();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(km.action_for(&ctrl_c), Some(Action::AbortStream));
    }

    #[test]
    fn test_custom_binding() {
        let mut km = Keymap::default();
        km.bind(Action::Quit, KeyBind::new(KeyCode::Char('q'), KeyModifiers::NONE));
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(km.triggers(&q, Action::Quit));
    }

    #[test]
    fn test_triggers() {
        let km = Keymap::default();
        let pgup = KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE);
        assert!(km.triggers(&pgup, Action::ScrollUp));
        assert!(!km.triggers(&pgup, Action::ScrollDown));
    }
}
