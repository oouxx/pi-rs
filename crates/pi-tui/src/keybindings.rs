use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::keys::{matches_key, Key, KeyEvent, KeyEventType, KeyModifiers};

/// A keybinding definition that maps a named action to a key sequence.
#[derive(Debug, Clone)]
pub struct KeybindingDefinition {
    pub name: String,
    pub description: String,
    pub default_key: KeyEvent,
}

/// A resolved keybinding mapping an action name to its current key.
#[derive(Debug, Clone)]
pub struct Keybinding {
    pub name: String,
    pub key: KeyEvent,
}

/// A set of keybinding definitions for a particular context.
#[derive(Debug, Clone)]
pub struct KeybindingDefinitions {
    pub context: String,
    pub definitions: Vec<KeybindingDefinition>,
}

/// User-provided keybinding overrides.
pub type KeybindingsConfig = HashMap<String, KeyEvent>;

/// A conflict between two keybindings that map to the same key sequence.
#[derive(Debug, Clone)]
pub struct KeybindingConflict {
    pub key: KeyEvent,
    pub actions: Vec<String>,
}

/// Manages keybinding definitions and user overrides.
pub struct KeybindingsManager {
    definitions: HashMap<String, KeybindingDefinition>,
    user_bindings: RwLock<KeybindingsConfig>,
}

impl KeybindingsManager {
    pub fn new(
        definitions: Vec<KeybindingDefinitions>,
        user_bindings: Option<KeybindingsConfig>,
    ) -> Self {
        let mut def_map = HashMap::new();
        for group in definitions {
            for def in group.definitions {
                def_map.insert(def.name.clone(), def);
            }
        }
        Self {
            definitions: def_map,
            user_bindings: RwLock::new(user_bindings.unwrap_or_default()),
        }
    }

    /// Check if a raw input string matches a given action's keybinding.
    pub fn matches(&self, data: &str, action_name: &str) -> bool {
        let resolved = self.get_resolved_key(action_name);
        if let Some(key_event) = resolved {
            return matches_key(data, &key_event);
        }
        false
    }

    /// Get the default keys for an action.
    pub fn get_keys(&self, action_name: &str) -> Vec<Key> {
        self.definitions
            .get(action_name)
            .map(|d| vec![d.default_key.key.clone()])
            .unwrap_or_default()
    }

    /// Get the full definition for an action.
    pub fn get_definition(&self, action_name: &str) -> Option<&KeybindingDefinition> {
        self.definitions.get(action_name)
    }

    /// Detect conflicts between resolved bindings.
    pub fn get_conflicts(&self) -> Vec<KeybindingConflict> {
        let mut key_to_actions: HashMap<String, Vec<String>> = HashMap::new();
        let resolved = self.get_resolved_bindings();

        for (name, key_event) in &resolved {
            let key_str = format!("{:?}", key_event);
            key_to_actions
                .entry(key_str)
                .or_default()
                .push(name.clone());
        }

        key_to_actions
            .into_iter()
            .filter(|(_, actions)| actions.len() > 1)
            .map(|(key_str, actions)| KeybindingConflict {
                key: resolved
                    .iter()
                    .find(|(_, k)| format!("{:?}", k) == key_str)
                    .map(|(_, k)| k.clone())
                    .unwrap(),
                actions,
            })
            .collect()
    }

    /// Set user-defined keybinding overrides.
    pub fn set_user_bindings(&self, bindings: KeybindingsConfig) {
        *self.user_bindings.write().unwrap() = bindings;
    }

    /// Get the current user-defined keybinding overrides.
    pub fn get_user_bindings(&self) -> KeybindingsConfig {
        self.user_bindings.read().unwrap().clone()
    }

    /// Get the fully resolved keybindings (defaults merged with user overrides).
    pub fn get_resolved_bindings(&self) -> KeybindingsConfig {
        let user = self.user_bindings.read().unwrap();
        let mut resolved = HashMap::new();
        for (name, def) in &self.definitions {
            if let Some(user_key) = user.get(name) {
                resolved.insert(name.clone(), user_key.clone());
            } else {
                resolved.insert(name.clone(), def.default_key.clone());
            }
        }
        resolved
    }

    fn get_resolved_key(&self, action_name: &str) -> Option<KeyEvent> {
        let user = self.user_bindings.read().unwrap();
        if let Some(key) = user.get(action_name) {
            return Some(key.clone());
        }
        self.definitions
            .get(action_name)
            .map(|d| d.default_key.clone())
    }
}

/// Default keybindings for editor context.
pub fn editor_keybindings() -> Vec<KeybindingDefinition> {
    vec![
        def("cursorLeft", "Move cursor left", Key::Left, KeyModifiers::default()),
        def("cursorRight", "Move cursor right", Key::Right, KeyModifiers::default()),
        def("cursorUp", "Move cursor up", Key::Up, KeyModifiers::default()),
        def("cursorDown", "Move cursor down", Key::Down, KeyModifiers::default()),
        def("cursorLineStart", "Move to line start", Key::Home, KeyModifiers::default()),
        def("cursorLineEnd", "Move to line end", Key::End, KeyModifiers::default()),
        def("deleteBackward", "Delete backward", Key::Backspace, KeyModifiers::default()),
        def("deleteForward", "Delete forward", Key::Delete, KeyModifiers::default()),
        def("pageUp", "Page up", Key::PageUp, KeyModifiers::default()),
        def("pageDown", "Page down", Key::PageDown, KeyModifiers::default()),
        def("undo", "Undo", Key::Char('z'), KeyModifiers { ctrl: true, ..Default::default() }),
        def("redo", "Redo", Key::Char('y'), KeyModifiers { ctrl: true, ..Default::default() }),
    ]
}

/// Default keybindings for input context.
pub fn input_keybindings() -> Vec<KeybindingDefinition> {
    vec![
        def("submit", "Submit input", Key::Enter, KeyModifiers::default()),
        def("newline", "Insert newline", Key::Enter, KeyModifiers { alt: true, ..Default::default() }),
        def("tab", "Insert tab or autocomplete", Key::Tab, KeyModifiers::default()),
        def("cancel", "Cancel input", Key::Escape, KeyModifiers::default()),
        def("cursorLeft", "Move cursor left", Key::Left, KeyModifiers::default()),
        def("cursorRight", "Move cursor right", Key::Right, KeyModifiers::default()),
        def("cursorHome", "Move to start", Key::Home, KeyModifiers::default()),
        def("cursorEnd", "Move to end", Key::End, KeyModifiers::default()),
        def("deleteBackward", "Delete backward", Key::Backspace, KeyModifiers::default()),
        def("deleteForward", "Delete forward", Key::Delete, KeyModifiers::default()),
        def("deleteWordBackward", "Delete word backward", Key::Backspace, KeyModifiers { ctrl: true, ..Default::default() }),
    ]
}

/// Default keybindings for select list context.
pub fn select_list_keybindings() -> Vec<KeybindingDefinition> {
    vec![
        def("selectUp", "Move selection up", Key::Up, KeyModifiers::default()),
        def("selectDown", "Move selection down", Key::Down, KeyModifiers::default()),
        def("selectConfirm", "Confirm selection", Key::Enter, KeyModifiers::default()),
        def("selectCancel", "Cancel selection", Key::Escape, KeyModifiers::default()),
        def("selectPageUp", "Page up", Key::PageUp, KeyModifiers::default()),
        def("selectPageDown", "Page down", Key::PageDown, KeyModifiers::default()),
    ]
}

fn def(name: &str, desc: &str, key: Key, modifiers: KeyModifiers) -> KeybindingDefinition {
    KeybindingDefinition {
        name: name.to_string(),
        description: desc.to_string(),
        default_key: KeyEvent {
            key,
            modifiers,
            event_type: KeyEventType::Press,
        },
    }
}

/// Global keybindings manager singleton.
static GLOBAL_KEYBINDINGS: OnceLock<Arc<KeybindingsManager>> = OnceLock::new();

/// Initialize the global keybindings manager. Call once at startup.
pub fn init_keybindings(user_bindings: Option<KeybindingsConfig>) {
    let definitions = vec![
        KeybindingDefinitions {
            context: "editor".into(),
            definitions: editor_keybindings(),
        },
        KeybindingDefinitions {
            context: "input".into(),
            definitions: input_keybindings(),
        },
        KeybindingDefinitions {
            context: "select".into(),
            definitions: select_list_keybindings(),
        },
    ];
    let manager = Arc::new(KeybindingsManager::new(definitions, user_bindings));
    let _ = GLOBAL_KEYBINDINGS.set(manager);
}

/// Get a reference to the global keybindings manager.
/// Auto-initializes with defaults if not yet initialized.
pub fn get_keybindings() -> Arc<KeybindingsManager> {
    GLOBAL_KEYBINDINGS
        .get()
        .cloned()
        .unwrap_or_else(|| {
            init_keybindings(None);
            GLOBAL_KEYBINDINGS.get().cloned().unwrap()
        })
}

/// Set user keybindings on the global manager.
pub fn set_keybindings_config(config: KeybindingsConfig) {
    if let Some(manager) = GLOBAL_KEYBINDINGS.get() {
        manager.set_user_bindings(config);
    } else {
        init_keybindings(Some(config));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_resolves_default() {
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: editor_keybindings(),
            }],
            None,
        );
        assert!(manager.matches("\x1b[A", "cursorUp"));
        assert!(manager.matches("\x1b[B", "cursorDown"));
    }

    #[test]
    fn test_manager_user_override() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "cursorUp".to_string(),
            KeyEvent {
                key: Key::Char('k'),
                modifiers: KeyModifiers::default(),
                event_type: KeyEventType::Press,
            },
        );
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: editor_keybindings(),
            }],
            Some(overrides),
        );
        // Now cursorUp should match 'k' instead of up arrow
        assert!(manager.matches("k", "cursorUp"));
        assert!(!manager.matches("\x1b[A", "cursorUp"));
    }

    #[test]
    fn test_conflict_detection() {
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: vec![
                    def("action1", "First", Key::Enter, KeyModifiers::default()),
                    def("action2", "Second", Key::Enter, KeyModifiers::default()),
                ],
            }],
            None,
        );
        let conflicts = manager.get_conflicts();
        assert!(!conflicts.is_empty());
        assert_eq!(conflicts[0].actions.len(), 2);
    }
}
