use serde::{Deserialize, Serialize};

use crate::config::APP_NAME;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlashCommandSource {
    Extension,
    Prompt,
    Skill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: Option<String>,
    pub source: SlashCommandSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinSlashCommand {
    pub name: String,
    pub description: String,
}

pub fn builtin_slash_commands() -> Vec<BuiltinSlashCommand> {
    vec![
        BuiltinSlashCommand { name: "settings".into(), description: "Open settings menu".into() },
        BuiltinSlashCommand { name: "model".into(), description: "Select model (opens selector UI)".into() },
        BuiltinSlashCommand { name: "scoped-models".into(), description: "Enable/disable models for Ctrl+P cycling".into() },
        BuiltinSlashCommand { name: "export".into(), description: "Export session (HTML default or specify path: .html/.jsonl)".into() },
        BuiltinSlashCommand { name: "import".into(), description: "Import and resume a session from a JSONL file".into() },
        BuiltinSlashCommand { name: "share".into(), description: "Share session as a secret GitHub gist".into() },
        BuiltinSlashCommand { name: "copy".into(), description: "Copy last agent message to clipboard".into() },
        BuiltinSlashCommand { name: "name".into(), description: "Set session display name".into() },
        BuiltinSlashCommand { name: "session".into(), description: "Show session info and stats".into() },
        BuiltinSlashCommand { name: "changelog".into(), description: "Show changelog entries".into() },
        BuiltinSlashCommand { name: "hotkeys".into(), description: "Show all keyboard shortcuts".into() },
        BuiltinSlashCommand { name: "fork".into(), description: "Create a new fork from a previous user message".into() },
        BuiltinSlashCommand { name: "clone".into(), description: "Duplicate the current session at the current position".into() },
        BuiltinSlashCommand { name: "tree".into(), description: "Navigate session tree (switch branches)".into() },
        BuiltinSlashCommand { name: "login".into(), description: "Configure provider authentication".into() },
        BuiltinSlashCommand { name: "logout".into(), description: "Remove provider authentication".into() },
        BuiltinSlashCommand { name: "new".into(), description: "Start a new session".into() },
        BuiltinSlashCommand { name: "compact".into(), description: "Manually compact the session context".into() },
        BuiltinSlashCommand { name: "resume".into(), description: "Resume a different session".into() },
        BuiltinSlashCommand { name: "reload".into(), description: "Reload keybindings, extensions, skills, prompts, and themes".into() },
        BuiltinSlashCommand { name: "quit".into(), description: format!("Quit {}", APP_NAME) },
    ]
}

pub fn is_slash_command(input: &str) -> bool {
    input.starts_with('/') && input.len() > 1 && !input.starts_with("//")
}

pub fn parse_slash_command(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    if !is_slash_command(trimmed) {
        return None;
    }
    let without_slash = &trimmed[1..];
    let parts: Vec<&str> = without_slash.splitn(2, ' ').collect();
    let command = parts[0];
    let args = parts.get(1).copied().unwrap_or("");
    Some((command, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_slash_commands() {
        let commands = builtin_slash_commands();
        assert!(!commands.is_empty());
        assert!(commands.iter().any(|c| c.name == "model"));
        assert!(commands.iter().any(|c| c.name == "quit"));
    }

    #[test]
    fn test_is_slash_command() {
        assert!(is_slash_command("/model"));
        assert!(is_slash_command("/quit"));
        assert!(!is_slash_command("//comment"));
        assert!(!is_slash_command("hello"));
        assert!(!is_slash_command("/"));
    }

    #[test]
    fn test_parse_slash_command() {
        assert_eq!(parse_slash_command("/model"), Some(("model", "")));
        assert_eq!(parse_slash_command("/model gpt-4o"), Some(("model", "gpt-4o")));
        assert_eq!(parse_slash_command("/export session.html"), Some(("export", "session.html")));
        assert_eq!(parse_slash_command("hello"), None);
    }
}