use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

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

    /// Check if a key event matches a given action's keybinding.
    pub fn matches(&self, event: &KeyEvent, action_name: &str) -> bool {
        let resolved = self.get_resolved_key(action_name);
        if let Some(expected) = resolved {
            return event.code == expected.code && event.modifiers == expected.modifiers;
        }
        false
    }

    /// Get the default keys for an action.
    pub fn get_keys(&self, action_name: &str) -> Vec<KeyCode> {
        self.definitions
            .get(action_name)
            .map(|d| vec![d.default_key.code])
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
                    .map(|(_, k)| *k)
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
                resolved.insert(name.clone(), *user_key);
            } else {
                resolved.insert(name.clone(), def.default_key);
            }
        }
        resolved
    }

    fn get_resolved_key(&self, action_name: &str) -> Option<KeyEvent> {
        let user = self.user_bindings.read().unwrap();
        if let Some(key) = user.get(action_name) {
            return Some(*key);
        }
        self.definitions.get(action_name).map(|d| d.default_key)
    }
}

/// Default keybindings for editor context.
pub fn editor_keybindings() -> Vec<KeybindingDefinition> {
    vec![
        def(
            "cursorLeft",
            "Move cursor left",
            KeyCode::Left,
            KeyModifiers::NONE,
        ),
        def(
            "cursorRight",
            "Move cursor right",
            KeyCode::Right,
            KeyModifiers::NONE,
        ),
        def(
            "cursorUp",
            "Move cursor up",
            KeyCode::Up,
            KeyModifiers::NONE,
        ),
        def(
            "cursorDown",
            "Move cursor down",
            KeyCode::Down,
            KeyModifiers::NONE,
        ),
        def(
            "cursorLineStart",
            "Move to line start",
            KeyCode::Home,
            KeyModifiers::NONE,
        ),
        def(
            "cursorLineEnd",
            "Move to line end",
            KeyCode::End,
            KeyModifiers::NONE,
        ),
        def(
            "deleteBackward",
            "Delete backward",
            KeyCode::Backspace,
            KeyModifiers::NONE,
        ),
        def(
            "deleteForward",
            "Delete forward",
            KeyCode::Delete,
            KeyModifiers::NONE,
        ),
        def("pageUp", "Page up", KeyCode::PageUp, KeyModifiers::NONE),
        def(
            "pageDown",
            "Page down",
            KeyCode::PageDown,
            KeyModifiers::NONE,
        ),
        def("undo", "Undo", KeyCode::Char('z'), KeyModifiers::CONTROL),
        def("redo", "Redo", KeyCode::Char('y'), KeyModifiers::CONTROL),
    ]
}

/// Default keybindings for input context.
pub fn input_keybindings() -> Vec<KeybindingDefinition> {
    vec![
        def("submit", "Submit input", KeyCode::Enter, KeyModifiers::NONE),
        def(
            "newline",
            "Insert newline",
            KeyCode::Enter,
            KeyModifiers::ALT,
        ),
        def(
            "tab",
            "Insert tab or autocomplete",
            KeyCode::Tab,
            KeyModifiers::NONE,
        ),
        def("cancel", "Cancel input", KeyCode::Esc, KeyModifiers::NONE),
        def(
            "cursorLeft",
            "Move cursor left",
            KeyCode::Left,
            KeyModifiers::NONE,
        ),
        def(
            "cursorRight",
            "Move cursor right",
            KeyCode::Right,
            KeyModifiers::NONE,
        ),
        def(
            "cursorHome",
            "Move to start",
            KeyCode::Home,
            KeyModifiers::NONE,
        ),
        def("cursorEnd", "Move to end", KeyCode::End, KeyModifiers::NONE),
        def(
            "deleteBackward",
            "Delete backward",
            KeyCode::Backspace,
            KeyModifiers::NONE,
        ),
        def(
            "deleteForward",
            "Delete forward",
            KeyCode::Delete,
            KeyModifiers::NONE,
        ),
        def(
            "deleteWordBackward",
            "Delete word backward",
            KeyCode::Backspace,
            KeyModifiers::CONTROL,
        ),
    ]
}

/// Default keybindings for select list context.
pub fn select_list_keybindings() -> Vec<KeybindingDefinition> {
    vec![
        def(
            "selectUp",
            "Move selection up",
            KeyCode::Up,
            KeyModifiers::NONE,
        ),
        def(
            "selectDown",
            "Move selection down",
            KeyCode::Down,
            KeyModifiers::NONE,
        ),
        def(
            "selectConfirm",
            "Confirm selection",
            KeyCode::Enter,
            KeyModifiers::NONE,
        ),
        def(
            "selectCancel",
            "Cancel selection",
            KeyCode::Esc,
            KeyModifiers::NONE,
        ),
        def(
            "selectPageUp",
            "Page up",
            KeyCode::PageUp,
            KeyModifiers::NONE,
        ),
        def(
            "selectPageDown",
            "Page down",
            KeyCode::PageDown,
            KeyModifiers::NONE,
        ),
    ]
}

fn def(name: &str, desc: &str, code: KeyCode, modifiers: KeyModifiers) -> KeybindingDefinition {
    KeybindingDefinition {
        name: name.to_string(),
        description: desc.to_string(),
        default_key: KeyEvent::new(code, modifiers),
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
pub fn get_keybindings() -> Arc<KeybindingsManager> {
    GLOBAL_KEYBINDINGS.get().cloned().unwrap_or_else(|| {
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
        assert!(manager.matches(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), "cursorUp"));
        assert!(manager.matches(
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            "cursorDown"
        ));
    }

    #[test]
    fn test_manager_user_override() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "cursorUp".to_string(),
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: editor_keybindings(),
            }],
            Some(overrides),
        );
        assert!(manager.matches(
            &KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            "cursorUp"
        ));
        assert!(!manager.matches(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), "cursorUp"));
    }

    #[test]
    fn test_conflict_detection() {
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: vec![
                    def("action1", "First", KeyCode::Enter, KeyModifiers::NONE),
                    def("action2", "Second", KeyCode::Enter, KeyModifiers::NONE),
                ],
            }],
            None,
        );
        let conflicts = manager.get_conflicts();
        assert!(!conflicts.is_empty());
        assert_eq!(conflicts[0].actions.len(), 2);
    }

    #[test]
    fn test_rebinding_submit_does_not_evict_select_confirm() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "submit".to_string(),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        );
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: vec![input_keybindings(), select_list_keybindings()]
                    .into_iter()
                    .flatten()
                    .collect(),
            }],
            Some(overrides),
        );
        assert!(manager.matches(
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            "selectConfirm"
        ));
    }

    #[test]
    fn test_rebinding_select_up_does_not_evict_editor_cursor_up() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "selectUp".to_string(),
            KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL),
        );
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: vec![editor_keybindings(), select_list_keybindings()]
                    .into_iter()
                    .flatten()
                    .collect(),
            }],
            Some(overrides),
        );
        assert!(manager.matches(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), "cursorUp"));
    }

    #[test]
    fn test_user_conflicts_detected() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "submit".to_string(),
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        overrides.insert(
            "selectConfirm".to_string(),
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: vec![input_keybindings(), select_list_keybindings()]
                    .into_iter()
                    .flatten()
                    .collect(),
            }],
            Some(overrides),
        );
        let conflicts = manager.get_conflicts();
        assert!(!conflicts.is_empty());
        let all_actions: Vec<&String> = conflicts.iter().flat_map(|c| &c.actions).collect();
        assert!(all_actions.iter().any(|a| a.as_str() == "submit"));
        assert!(all_actions.iter().any(|a| a.as_str() == "selectConfirm"));
    }

    #[test]
    fn test_get_resolved_bindings_merges_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "cursorUp".to_string(),
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        );
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: editor_keybindings(),
            }],
            Some(overrides),
        );
        let resolved = manager.get_resolved_bindings();
        let cursor_up = resolved.get("cursorUp").unwrap();
        assert_eq!(cursor_up.code, KeyCode::Char('k'));
    }

    #[test]
    fn test_get_keys_returns_defaults() {
        let manager = KeybindingsManager::new(
            vec![KeybindingDefinitions {
                context: "test".into(),
                definitions: input_keybindings(),
            }],
            None,
        );
        let keys = manager.get_keys("submit");
        assert!(keys.contains(&KeyCode::Enter));
    }
}
