//! Slash commands for ACP sessions.
//!
//! Mirrors pi-acp's `src/acp/slash-commands.ts` + `src/acp/pi-commands.ts`:
//! file-based commands are loaded from pi's prompt directories (user
//! `{agent_dir}/prompts/**/*.md`, project `{cwd}/.pi-rs/prompts/**/*.md`),
//! converted to ACP `AvailableCommand`s for the `available_commands_update`
//! notification, and expanded in prompt text (`/name arg1 arg2` → file content
//! with `$1`/`$2`/`$@` substitution). A small set of built-in commands is
//! advertised alongside the file-based ones.

use std::path::Path;

use agent_client_protocol as acp;

use crate::config;

/// A file-based slash command (mirrors pi-acp's `FileSlashCommand`).
#[derive(Debug, Clone)]
pub struct FileSlashCommand {
    pub name: String,
    pub description: String,
    pub content: String,
    /// e.g. "(user)", "(project)", "(project:frontend)".
    pub source: String,
}

/// Parse YAML-ish frontmatter (`---\nkey: value\n---`) from a command file.
fn parse_frontmatter(content: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut frontmatter = std::collections::HashMap::new();
    if !content.starts_with("---") {
        return (frontmatter, content.to_string());
    }
    let Some(end_index) = content.find("\n---") else {
        return (frontmatter, content.to_string());
    };
    let block = &content[3..end_index];
    let remaining = content[end_index + 4..].trim_start().to_string();
    for line in block.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            if !key.is_empty() {
                frontmatter.insert(key.to_string(), v.trim().to_string());
            }
        }
    }
    (frontmatter, remaining)
}

/// Recursively load `*.md` command files from `dir`.
fn load_commands_from_dir(
    dir: &Path,
    source: &str,
    subdir: &str,
    out: &mut Vec<FileSlashCommand>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // silently skip unreadable dirs
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let new_subdir = if subdir.is_empty() {
                entry.file_name().to_string_lossy().to_string()
            } else {
                format!("{subdir}:{}", entry.file_name().to_string_lossy())
            };
            load_commands_from_dir(&path, source, &new_subdir, out);
            continue;
        }
        if !path.is_file() || !path.extension().map(|e| e == "md").unwrap_or(false) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue; // silently skip unreadable files
        };
        let (frontmatter, content) = parse_frontmatter(&raw);
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let source_str = if subdir.is_empty() {
            format!("({source})")
        } else {
            format!("({source}:{subdir})")
        };
        let mut description = frontmatter
            .get("description")
            .cloned()
            .unwrap_or_default();
        if description.is_empty() {
            if let Some(first_line) = content.lines().find(|l| !l.trim().is_empty()) {
                description = first_line.trim().to_string();
                if description.chars().count() > 60 {
                    description = description.chars().take(60).collect::<String>() + "...";
                }
            }
        }
        description = if description.is_empty() {
            source_str.clone()
        } else {
            format!("{description} {source_str}")
        };
        out.push(FileSlashCommand {
            name,
            description,
            content,
            source: source_str,
        });
    }
}

/// Load file-based slash commands from pi's prompt directories.
///
/// - user:    `{agent_dir}/prompts/**/*.md`
/// - project: `{cwd}/.pi-rs/prompts/**/*.md`
///
/// User commands are loaded first, then project (matching pi-acp ordering).
pub fn load_slash_commands(cwd: &str) -> Vec<FileSlashCommand> {
    let mut commands = Vec::new();
    let user_dir = config::get_agent_dir().join("prompts");
    load_commands_from_dir(&user_dir, "user", "", &mut commands);
    let project_dir = Path::new(cwd).join(config::CONFIG_DIR_NAME).join("prompts");
    load_commands_from_dir(&project_dir, "project", "", &mut commands);
    commands
}

/// Convert file-based commands to ACP `AvailableCommand`s, de-duping by name
/// (first wins, matching pi-acp).
pub fn to_available_commands(file_commands: &[FileSlashCommand]) -> Vec<acp::AvailableCommand> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in file_commands {
        if !seen.insert(c.name.clone()) {
            continue;
        }
        out.push(acp::AvailableCommand::new(c.name.clone(), c.description.clone()));
    }
    out
}

/// The built-in headless/editor commands advertised alongside file commands
/// (mirrors pi-acp's `builtinAvailableCommands`).
pub fn builtin_available_commands() -> Vec<acp::AvailableCommand> {
    [
        ("compact", "Compact the session (optionally with custom instructions)"),
        ("autocompact", "Toggle automatic compaction: on|off|toggle"),
        ("export", "Export the current session to HTML"),
        ("session", "Show session stats (tokens/messages/cost/session file)"),
        ("name", "Set session display name: /name <name>"),
        ("queue", "Set pi queue mode: all|one-at-a-time"),
        ("changelog", "Print the installed pi changelog (best-effort)"),
        ("steering", "Get/set pi Steering Mode"),
        ("follow-up", "Get/set pi Follow-up Mode"),
    ]
    .into_iter()
    .map(|(name, description)| acp::AvailableCommand::new(name, description))
    .collect()
}

/// Parse command args with bash-style quote support (mirrors pi-acp).
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    for ch in args_string.chars() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_quote = Some(ch),
            ' ' | '\t' => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Substitute `$1`, `$2`, … and `$@` in command content (mirrors pi-acp).
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let joined = args.join(" ");
    let mut result = content.replace("$@", &joined);
    // Replace $N (single digits and multi-digit) — iterate from highest index
    // down so `$10` isn't clobbered by `$1`.
    let max = args.len();
    for i in (1..=max).rev() {
        result = result.replace(&format!("${i}"), &args[i - 1]);
    }
    result
}

/// Expand a leading `/command` using the loaded file commands. Returns the
/// original text when it is not a known slash command (mirrors pi-acp).
pub fn expand_slash_command(text: &str, file_commands: &[FileSlashCommand]) -> String {
    if !text.starts_with('/') {
        return text.to_string();
    }
    let space_index = text.find(' ');
    let command_name = match space_index {
        Some(i) => &text[1..i],
        None => &text[1..],
    };
    let args_string = match space_index {
        Some(i) => &text[i + 1..],
        None => "",
    };
    let Some(cmd) = file_commands.iter().find(|c| c.name == command_name) else {
        return text.to_string();
    };
    let args = parse_command_args(args_string);
    substitute_args(&cmd.content, &args)
}

/// Resolve the path of a file-based command by name (used by `/export`-style
/// lookups and tests).
pub fn find_command<'a>(
    file_commands: &'a [FileSlashCommand],
    name: &str,
) -> Option<&'a FileSlashCommand> {
    file_commands.iter().find(|c| c.name == name)
}

/// A resolved slash command: either a built-in or a file-based command.
pub enum ResolvedCommand {
    Builtin(String),
    File(FileSlashCommand),
}

/// Resolve a prompt text's leading command against built-ins and file
/// commands. Returns `None` when the text is not a slash command.
pub fn resolve_command(
    text: &str,
    file_commands: &[FileSlashCommand],
) -> Option<(ResolvedCommand, Vec<String>)> {
    if !text.starts_with('/') {
        return None;
    }
    let space_index = text.find(' ');
    let command_name = match space_index {
        Some(i) => &text[1..i],
        None => &text[1..],
    };
    let args_string = match space_index {
        Some(i) => &text[i + 1..],
        None => "",
    };
    let args = parse_command_args(args_string);
    const BUILTINS: &[&str] = &[
        "compact",
        "autocompact",
        "export",
        "session",
        "name",
        "queue",
        "changelog",
        "steering",
        "follow-up",
    ];
    if BUILTINS.contains(&command_name) {
        return Some((ResolvedCommand::Builtin(command_name.to_string()), args));
    }
    if let Some(cmd) = find_command(file_commands, command_name) {
        return Some((ResolvedCommand::File(cmd.clone()), args));
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn write_cmd(dir: &Path, name: &str, content: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn loads_user_and_project_commands_recursively() {
        let base = tempfile::tempdir().expect("tempdir");
        // User dir: {agent_dir}/prompts
        let agent_dir = base.path().join("agent");
        write_cmd(&agent_dir.join("prompts"), "review.md", "---\ndescription: Code review\n---\nReview the code.\n");
        // Project dir: {cwd}/.pi-rs/prompts (nested subdir)
        let cwd = base.path().join("proj");
        write_cmd(&cwd.join(".pi-rs/prompts/frontend"), "fix.md", "Fix the frontend.\n");

        // Point the agent dir at the temp location.
        std::env::set_var("PI_CODING_AGENT_DIR", agent_dir.to_string_lossy().to_string());
        let commands = load_slash_commands(&cwd.to_string_lossy());
        std::env::remove_var("PI_CODING_AGENT_DIR");

        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"review"), "user command missing: {names:?}");
        assert!(names.contains(&"fix"), "project command missing: {names:?}");
        let review = commands.iter().find(|c| c.name == "review").unwrap();
        assert!(review.description.contains("Code review"));
        assert!(review.description.contains("(user)"));
        let fix = commands.iter().find(|c| c.name == "fix").unwrap();
        assert!(fix.description.contains("(project:frontend)"));
    }

    #[test]
    fn to_available_commands_dedupes_by_name() {
        let commands = vec![
            FileSlashCommand {
                name: "dup".into(),
                description: "first".into(),
                content: "a".into(),
                source: "(user)".into(),
            },
            FileSlashCommand {
                name: "dup".into(),
                description: "second".into(),
                content: "b".into(),
                source: "(project)".into(),
            },
        ];
        let available = to_available_commands(&commands);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].name, "dup");
        assert_eq!(available[0].description, "first");
    }

    #[test]
    fn parse_command_args_handles_quotes() {
        assert_eq!(parse_command_args("a b c"), vec!["a", "b", "c"]);
        assert_eq!(parse_command_args("a \"b c\" d"), vec!["a", "b c", "d"]);
        assert_eq!(parse_command_args("'x y'"), vec!["x y"]);
        assert_eq!(parse_command_args(""), Vec::<String>::new());
    }

    #[test]
    fn substitute_args_replaces_positional_and_at() {
        let args = vec!["one".to_string(), "two".to_string()];
        assert_eq!(substitute_args("$1 and $2", &args), "one and two");
        assert_eq!(substitute_args("all: $@", &args), "all: one two");
        // $10 must not be clobbered by $1.
        let many: Vec<String> = (1..=10).map(|i| i.to_string()).collect();
        assert_eq!(substitute_args("$10", &many), "10");
    }

    #[test]
    fn expand_slash_command_substitutes_args() {
        let commands = vec![FileSlashCommand {
            name: "greet".into(),
            description: "greet".into(),
            content: "Hello $1!".into(),
            source: "(user)".into(),
        }];
        assert_eq!(
            expand_slash_command("/greet world", &commands),
            "Hello world!"
        );
        // Unknown command passes through unchanged.
        assert_eq!(expand_slash_command("/nope x", &commands), "/nope x");
        // Non-command text passes through.
        assert_eq!(expand_slash_command("plain text", &commands), "plain text");
    }

    #[test]
    fn resolve_command_detects_builtin_and_file() {
        let commands = vec![FileSlashCommand {
            name: "greet".into(),
            description: "greet".into(),
            content: "Hello $1!".into(),
            source: "(user)".into(),
        }];
        let (cmd, args) = resolve_command("/compact now", &commands).expect("builtin");
        assert!(matches!(cmd, ResolvedCommand::Builtin(ref n) if n == "compact"));
        assert_eq!(args, vec!["now"]);

        let (cmd, args) = resolve_command("/greet world", &commands).expect("file");
        match cmd {
            ResolvedCommand::File(f) => assert_eq!(f.name, "greet"),
            _ => panic!("expected file command"),
        }
        assert_eq!(args, vec!["world"]);

        assert!(resolve_command("/unknown", &commands).is_none());
        assert!(resolve_command("no slash", &commands).is_none());
    }
}
