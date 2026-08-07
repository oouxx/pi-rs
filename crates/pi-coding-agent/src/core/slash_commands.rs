use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::APP_NAME;
use crate::core::source_info::SourceInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlashCommandSource {
    Extension,
    Prompt,
    Skill,
}

/// A discoverable slash command exposed to extensions/TUI, matching the TS
/// `SlashCommandInfo` (builtins are *not* represented here — they live in
/// `BuiltinSlashCommand` and never carry a `source`).
///
/// Serialized as camelCase to match the TS wire format consumed by the RPC
/// client and extensions (`name`, `description`, `source`, `sourceInfo`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: Option<String>,
    pub source: SlashCommandSource,
    pub source_info: SourceInfo,
}

/// A built-in interactive slash command (settings/model/quit/…). Built-ins are
/// a flat list with no `source` and never participate in extension-name
/// conflict resolution — they are presented as-is by the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuiltinSlashCommand {
    pub name: String,
    pub description: String,
    #[serde(rename = "argumentHint", skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
}

/// The 22 built-in interactive slash commands, matching
/// `BUILTIN_SLASH_COMMANDS` in `packages/coding-agent/src/core/slash-commands.ts`.
#[must_use]
pub fn builtin_slash_commands() -> Vec<BuiltinSlashCommand> {
    vec![
        BuiltinSlashCommand {
            name: "settings".into(),
            description: "Open settings menu".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "model".into(),
            description: "Select model (opens selector UI)".into(),
            argument_hint: Some("<provider/model>".into()),
        },
        BuiltinSlashCommand {
            name: "scoped-models".into(),
            description: "Enable/disable models for Ctrl+P cycling".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "export".into(),
            description: "Export session (HTML default, or specify path: .html/.jsonl)".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "import".into(),
            description: "Import and resume a session from a JSONL file".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "share".into(),
            description: "Share session as a secret GitHub gist".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "copy".into(),
            description: "Copy last agent message to clipboard".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "name".into(),
            description: "Set session display name".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "session".into(),
            description: "Show session info and stats".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "changelog".into(),
            description: "Show changelog entries".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "hotkeys".into(),
            description: "Show all keyboard shortcuts".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "fork".into(),
            description: "Create a new fork from a previous user message".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "clone".into(),
            description: "Duplicate the current session at the current position".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "tree".into(),
            description: "Navigate session tree (switch branches)".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "trust".into(),
            description: "Save project trust decision for future sessions".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "login".into(),
            description: "Configure provider authentication".into(),
            argument_hint: Some("<provider>".into()),
        },
        BuiltinSlashCommand {
            name: "logout".into(),
            description: "Remove provider authentication".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "new".into(),
            description: "Start a new session".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "compact".into(),
            description: "Manually compact the session context".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "resume".into(),
            description: "Resume a different session".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "reload".into(),
            description:
                "Reload keybindings, extensions, skills, prompts, themes, and context files".into(),
            argument_hint: None,
        },
        BuiltinSlashCommand {
            name: "quit".into(),
            description: format!("Quit {APP_NAME}"),
            argument_hint: None,
        },
    ]
}

/// A resolved command with its invocation name (may include a `:N` suffix
/// when multiple extensions register the same command name).
///
/// This mirrors the TS `ResolvedCommand extends RegisteredCommand`. The
/// `source_info` is carried from the original `RegisteredCommand`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCommand {
    pub invocation_name: String,
    pub name: String,
    pub description: Option<String>,
    pub source_info: SourceInfo,
}

/// Resolve extension command name conflicts, matching the TS
/// `ExtensionRunner.resolveRegisteredCommands()` (`runner.ts`).
///
/// Semantics (must match the original exactly):
/// - Only extension commands are considered; built-in commands never
///   participate (they are not part of this list and never get suffixed).
/// - `counts` tallies how many extensions register each *name*. If a name is
///   registered by more than one extension, every occurrence gets a
///   `:N` suffix where N is the 1-based occurrence index for that name.
/// - `takenInvocationNames` guards against collisions so the chosen
///   `invocationName` is always unique, bumping the suffix past any clash.
#[must_use]
pub fn resolve_extension_commands(
    extension_commands: &[crate::core::extensions::RegisteredCommand],
) -> Vec<ResolvedCommand> {
    // counts: how many extensions registered each command name.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for cmd in extension_commands {
        *counts.entry(cmd.name.clone()).or_insert(0) += 1;
    }

    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut taken_invocation_names: HashSet<String> = HashSet::new();
    let mut resolved: Vec<ResolvedCommand> = Vec::with_capacity(extension_commands.len());

    for cmd in extension_commands {
        let occurrence_slot = seen.entry(cmd.name.clone()).or_insert(0);
        *occurrence_slot += 1;
        let occurrence = *occurrence_slot;

        let conflict_count = *counts.get(&cmd.name).unwrap_or(&0);
        let mut invocation_name = if conflict_count > 1 {
            format!("{}:{}", cmd.name, occurrence)
        } else {
            cmd.name.clone()
        };

        if taken_invocation_names.contains(&invocation_name) {
            let mut suffix = occurrence;
            loop {
                suffix += 1;
                invocation_name = format!("{}:{}", cmd.name, suffix);
                if !taken_invocation_names.contains(&invocation_name) {
                    break;
                }
            }
        }

        taken_invocation_names.insert(invocation_name.clone());
        resolved.push(ResolvedCommand {
            invocation_name,
            name: cmd.name.clone(),
            description: Some(cmd.description.clone()),
            source_info: cmd.source_info.clone(),
        });
    }

    resolved
}

/// Returns true when `input` is a slash command invocation, i.e. starts with
/// a single `/` (a leading `//` is treated as a comment and is not a command).
#[must_use]
pub fn is_slash_command(input: &str) -> bool {
    input.starts_with('/') && input.len() > 1 && !input.starts_with("//")
}

/// Parse a slash command line into `(command, args)`, where `command` excludes
/// the leading `/` and `args` is the remainder after the first space (or `""`).
/// Returns `None` for non-slash-command input (including `//comment`).
#[must_use]
pub fn parse_slash_command(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    if !is_slash_command(trimmed) {
        return None;
    }
    let without_slash = &trimmed[1..];
    let parts: Vec<&str> = without_slash.splitn(2, ' ').collect();
    let command = parts[0];
    // `splitn(2, ' ')` leaves any leading spaces (from multiple spaces after
    // the command name) on the args slice; trim them so callers don't have
    // to. Internal spaces within args are preserved.
    let args = parts.get(1).copied().unwrap_or("").trim();
    Some((command, args))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_builtin_slash_commands() {
        let commands = builtin_slash_commands();
        // The TS BUILTIN_SLASH_COMMANDS array has exactly 22 entries.
        assert_eq!(commands.len(), 22);
        assert!(commands.iter().any(|c| c.name == "model"));
        assert!(commands.iter().any(|c| c.name == "quit"));
        // argumentHint wiring
        assert_eq!(
            commands
                .iter()
                .find(|c| c.name == "model")
                .unwrap()
                .argument_hint
                .as_deref(),
            Some("<provider/model>")
        );
        assert!(commands
            .iter()
            .find(|c| c.name == "settings")
            .unwrap()
            .argument_hint
            .is_none());
    }

    #[test]
    fn test_builtin_export_description_matches_ts() {
        let commands = builtin_slash_commands();
        let export = commands.iter().find(|c| c.name == "export").unwrap();
        assert_eq!(
            export.description,
            "Export session (HTML default, or specify path: .html/.jsonl)"
        );
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
        assert_eq!(
            parse_slash_command("/model gpt-4o"),
            Some(("model", "gpt-4o"))
        );
        assert_eq!(
            parse_slash_command("/export session.html"),
            Some(("export", "session.html"))
        );
        assert_eq!(parse_slash_command("hello"), None);
        // //comment is not a slash command
        assert_eq!(parse_slash_command("//comment"), None);
        // surrounding whitespace is trimmed
        assert_eq!(
            parse_slash_command("  /model   gpt-4o  "),
            Some(("model", "gpt-4o"))
        );
    }

    fn make_ext_cmd(name: &str, description: &str) -> crate::core::extensions::RegisteredCommand {
        crate::core::extensions::RegisteredCommand {
            name: name.into(),
            description: description.into(),
            execute: std::sync::Arc::new(|_| Box::pin(async move {})),
            source_info: crate::core::source_info::create_builtin_source_info("test"),
            get_argument_completions: None,
        }
    }

    #[test]
    fn test_resolve_extension_commands_no_conflict() {
        let ext_cmds = vec![make_ext_cmd("mycmd", "My custom command")];
        let resolved = resolve_extension_commands(&ext_cmds);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].invocation_name, "mycmd");
        assert_eq!(resolved[0].name, "mycmd");
        assert_eq!(
            resolved[0].source_info,
            crate::core::source_info::create_builtin_source_info("test")
        );
    }

    #[test]
    fn test_resolve_extension_commands_single_builtin_name_no_suffix() {
        // An extension registering a name that *happens* to match a builtin
        // (e.g. "model") is resolved among extensions only: since it is the
        // sole extension with that name, it keeps the bare name (no `:2`).
        // This matches TS resolveRegisteredCommands, which never folds in
        // builtins. The builtin/extension collision is reported separately by
        // getBuiltInCommandConflictDiagnostics in the TUI, not here.
        let ext_cmds = vec![make_ext_cmd("model", "My model command")];
        let resolved = resolve_extension_commands(&ext_cmds);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].invocation_name, "model");
        assert_eq!(resolved[0].name, "model");
    }

    #[test]
    fn test_resolve_extension_commands_two_extensions_same_name() {
        // Two extensions registering "model": counts=2 > 1, so both get a
        // `:N` suffix (1-based occurrence): "model:1" and "model:2".
        let ext_cmds = vec![
            make_ext_cmd("model", "First ext model"),
            make_ext_cmd("model", "Second ext model"),
        ];
        let resolved = resolve_extension_commands(&ext_cmds);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].invocation_name, "model:1");
        assert_eq!(resolved[1].invocation_name, "model:2");
    }

    #[test]
    fn test_resolve_extension_commands_taken_invocation_name_collision() {
        // Three extensions register "go"; occurrences 1,2,3 → "go:1","go:2","go:3".
        let ext_cmds = vec![
            make_ext_cmd("go", "a"),
            make_ext_cmd("go", "b"),
            make_ext_cmd("go", "c"),
        ];
        let resolved = resolve_extension_commands(&ext_cmds);
        let names: Vec<&str> = resolved
            .iter()
            .map(|c| c.invocation_name.as_str())
            .collect();
        assert_eq!(names, vec!["go:1", "go:2", "go:3"]);
        // all invocation names are unique
        let unique: HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn test_resolve_extension_commands_mixed_names() {
        let ext_cmds = vec![
            make_ext_cmd("alpha", "a1"),
            make_ext_cmd("beta", "b1"),
            make_ext_cmd("alpha", "a2"),
        ];
        let resolved = resolve_extension_commands(&ext_cmds);
        // alpha appears twice → "alpha:1","alpha:2"; beta once → "beta"
        let names: Vec<&str> = resolved
            .iter()
            .map(|c| c.invocation_name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha:1", "beta", "alpha:2"]);
    }
}
