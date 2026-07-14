//! CLI argument parsing and help display.
//!
//! Mirrors packages/coding-agent/src/cli/args.ts

use std::collections::HashMap;

use pi_coding_agent::config;
use pi_coding_agent::core::project_trust::DefaultProjectTrust;

/// Output mode for non-interactive runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Text,
    Json,
    Rpc,
    Interactive,
}

/// Parsed CLI arguments.
#[derive(Debug, Clone)]
pub struct CliArgs {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Vec<String>,
    pub thinking: Option<String>,
    pub continue_session: bool,
    pub resume_session: bool,
    pub help: bool,
    pub version: bool,
    pub mode: OutputMode,
    pub name: Option<String>,
    pub no_session: bool,
    pub session: Option<String>,
    pub session_id: Option<String>,
    pub fork: Option<String>,
    pub session_dir: Option<String>,
    pub list_models: bool,
    pub tools: Vec<String>,
    pub exclude_tools: Vec<String>,
    pub no_tools: bool,
    pub no_builtin_tools: bool,
    pub extensions: Vec<String>,
    pub no_extensions: bool,
    pub print: bool,
    pub no_skills: bool,
    pub verbose: bool,
    pub project_trust_override: Option<bool>,
    pub messages: Vec<String>,
    pub unknown_flags: HashMap<String, String>,
    pub diagnostics: Vec<String>,
    pub default_project_trust: DefaultProjectTrust,
    /// Subcommand (e.g. "install", "remove", "list")
    pub subcommand: Option<String>,
    /// Arguments to the subcommand
    pub subcommand_args: Vec<String>,
}

impl CliArgs {
    pub fn new() -> Self {
        CliArgs {
            provider: None,
            model: None,
            api_key: None,
            system_prompt: None,
            append_system_prompt: Vec::new(),
            thinking: None,
            continue_session: false,
            resume_session: false,
            help: false,
            version: false,
            mode: OutputMode::Text,
            name: None,
            no_session: false,
            session: None,
            session_id: None,
            fork: None,
            session_dir: None,
            list_models: false,
            tools: Vec::new(),
            exclude_tools: Vec::new(),
            no_tools: false,
            no_builtin_tools: false,
            extensions: Vec::new(),
            no_extensions: false,
            print: false,
            no_skills: false,
            verbose: false,
            project_trust_override: None,
            messages: Vec::new(),
            unknown_flags: HashMap::new(),
            diagnostics: Vec::new(),
            default_project_trust: DefaultProjectTrust::Ask,
            subcommand: None,
            subcommand_args: Vec::new(),
        }
    }

    pub fn should_run(&self) -> bool {
        !self.help && !self.version && !self.list_models
    }
}

/// Print help text to stdout.
pub fn print_help() {
    let name = config::APP_NAME;
    let title = config::APP_TITLE;
    println!("{title} — coding agent v{}", config::VERSION);
    println!();
    println!("USAGE:");
    println!("    {name} [OPTIONS] [MESSAGE...]");
    println!();
    println!("OPTIONS:");
    println!("    -p, --print           Print mode (single-shot, output to stdout)");
    println!("    -i, --interactive     Interactive TUI mode");
    println!("    --mode <MODE>         Output mode: text (default), json, rpc, or interactive/tui");
    println!("    -m, --model <MODEL>   Model to use (e.g. claude-sonnet-4-6)");
    println!("    -P, --provider <P>    Provider to use");
    println!("    -k, --api-key <KEY>   API key");
    println!("    -s, --system-prompt   System prompt");
    println!("    -A, --append-prompt   Append to system prompt (can be repeated)");
    println!("    -t, --thinking <LVL>  Thinking level: off|minimal|low|medium|high|xhigh");
    println!("    --continue            Continue from last session");
    println!("    --resume              Resume a session");
    println!("    --name <NAME>         Session name");
    println!("    --session <ID>        Resume session by ID");
    println!("    --fork <ID>           Fork from an existing session");
    println!("    --no-session          Don't persist session");
    println!("    --session-dir <DIR>   Custom session directory");
    println!("    --tool <NAME>         Allow specific tool (can be repeated)");
    println!("    --exclude-tool <NAME> Exclude specific tool (can be repeated)");
    println!("    --no-tools            Disable all tools");
    println!("    --no-builtin-tools    Disable built-in tools");
    println!("    --extension <PATH>    Load extension (can be repeated)");
    println!("    --no-extensions       Disable extension loading");
    println!("    --no-skills           Disable skill loading");
    println!("    --trust               Trust the project directory");
    println!("    --no-trust            Do not trust the project directory");
    println!("    --default-trust <V>   Default trust: always|never|ask (default: ask)");
    println!("    --list-models         List available models and exit");
    println!("    --verbose             Verbose output");
    println!("    -h, --help            Show this help");
    println!("    -v, --version         Show version");
    println!();
    println!("EXAMPLES:");
    println!("    {name} \"write a fibonacci function in rust\"");
    println!("    {name} -p \"explain this\" < file.rs");
    println!("    {name} --mode json \"list files\"");
    println!("    {name} --list-models");
}

/// Parse CLI arguments from raw string slices.
pub fn parse_args(args: &[String]) -> CliArgs {
    let mut result = CliArgs::new();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        match arg.as_str() {
            "--help" | "-h" => result.help = true,
            "--version" | "-v" => result.version = true,
            "--interactive" | "-i" => result.mode = OutputMode::Interactive,
            "--print" | "-p" => result.print = true,
            "--verbose" => result.verbose = true,
            "--list-models" => result.list_models = true,
            "--continue" => result.continue_session = true,
            "--resume" => result.resume_session = true,
            "--no-session" => result.no_session = true,
            "--no-tools" => result.no_tools = true,
            "--no-builtin-tools" => result.no_builtin_tools = true,
            "--no-extensions" => result.no_extensions = true,
            "--no-skills" => result.no_skills = true,

            "--mode" => {
                i += 1;
                if i < args.len() {
                    match args[i].as_str() {
                        "json" => result.mode = OutputMode::Json,
                        "rpc" => result.mode = OutputMode::Rpc,
                        "interactive" | "tui" => result.mode = OutputMode::Interactive,
                        _ => result.mode = OutputMode::Text,
                    }
                }
            }

            "--model" | "-m" => {
                i += 1;
                if i < args.len() {
                    result.model = Some(args[i].clone());
                }
            }

            "--provider" | "-P" => {
                i += 1;
                if i < args.len() {
                    result.provider = Some(args[i].clone());
                }
            }

            "--api-key" | "-k" => {
                i += 1;
                if i < args.len() {
                    result.api_key = Some(args[i].clone());
                }
            }

            "--system-prompt" | "-s" => {
                i += 1;
                if i < args.len() {
                    result.system_prompt = Some(args[i].clone());
                }
            }

            "--append-prompt" | "-A" => {
                i += 1;
                if i < args.len() {
                    result.append_system_prompt.push(args[i].clone());
                }
            }

            "--thinking" | "-t" => {
                i += 1;
                if i < args.len() {
                    let level = args[i].to_lowercase();
                    match level.as_str() {
                        "off" | "minimal" | "low" | "medium" | "high" | "xhigh" => {
                            result.thinking = Some(level);
                        }
                        _ => {
                            result
                                .diagnostics
                                .push(format!("Invalid thinking level: {}", args[i]));
                        }
                    }
                }
            }

            "--trust" => result.project_trust_override = Some(true),
            "--no-trust" => result.project_trust_override = Some(false),

            "--default-trust" => {
                i += 1;
                if i < args.len() {
                    result.default_project_trust = DefaultProjectTrust::from_str(&args[i]);
                }
            }

            "--name" => {
                i += 1;
                if i < args.len() {
                    result.name = Some(args[i].clone());
                }
            }

            "--session" => {
                i += 1;
                if i < args.len() {
                    result.session = Some(args[i].clone());
                }
            }

            "--session-id" => {
                i += 1;
                if i < args.len() {
                    result.session_id = Some(args[i].clone());
                }
            }

            "--fork" => {
                i += 1;
                if i < args.len() {
                    result.fork = Some(args[i].clone());
                }
            }

            "--session-dir" => {
                i += 1;
                if i < args.len() {
                    result.session_dir = Some(args[i].clone());
                }
            }

            "--tool" => {
                i += 1;
                if i < args.len() {
                    result.tools.push(args[i].clone());
                }
            }

            "--exclude-tool" => {
                i += 1;
                if i < args.len() {
                    result.exclude_tools.push(args[i].clone());
                }
            }

            "--extension" => {
                i += 1;
                if i < args.len() {
                    result.extensions.push(args[i].clone());
                }
            }

            // Unknown flag that starts with --
            s if s.starts_with("--") || s.starts_with("-") && s.len() > 1 => {
                result.diagnostics.push(format!("Unknown flag: {s}"));
            }

            // Check for subcommands
            s if s == "install" || s == "remove" || s == "update" || s == "list" || s == "config" => {
                result.subcommand = Some(s.to_string());
                // Collect remaining args as subcommand args
                while i + 1 < args.len() {
                    i += 1;
                    result.subcommand_args.push(args[i].clone());
                }
            }

            // Positional argument = message
            _ => {
                result.messages.push(arg.clone());
            }
        }

        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty() {
        let args = parse_args(&[]);
        assert!(!args.help);
        assert!(!args.version);
        assert!(args.messages.is_empty());
    }

    #[test]
    fn test_parse_help() {
        let args = parse_args(&["--help".into()]);
        assert!(args.help);
    }

    #[test]
    fn test_parse_version() {
        let args = parse_args(&["-v".into()]);
        assert!(args.version);
    }

    #[test]
    fn test_parse_messages() {
        let args = parse_args(&["hello".into(), "world".into()]);
        assert_eq!(args.messages, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_model() {
        let args = parse_args(&["--model".into(), "claude-sonnet-4-6".into()]);
        assert_eq!(args.model, Some("claude-sonnet-4-6".into()));
    }

    #[test]
    fn test_parse_print() {
        let args = parse_args(&["-p".into(), "hello".into()]);
        assert!(args.print);
        assert_eq!(args.messages, vec!["hello"]);
    }

    #[test]
    fn test_parse_json_mode() {
        let args = parse_args(&["--mode".into(), "json".into()]);
        assert_eq!(args.mode, OutputMode::Json);
    }

    #[test]
    fn test_parse_thinking() {
        let args = parse_args(&["-t".into(), "high".into()]);
        assert_eq!(args.thinking, Some("high".into()));
    }

    #[test]
    fn test_parse_invalid_thinking() {
        let args = parse_args(&["-t".into(), "invalid".into()]);
        assert!(args.thinking.is_none());
        assert!(!args.diagnostics.is_empty());
    }

    #[test]
    fn test_parse_extensions() {
        let args = parse_args(&[
            "--extension".into(),
            "/path/to/ext1".into(),
            "--extension".into(),
            "/path/to/ext2".into(),
        ]);
        assert_eq!(args.extensions.len(), 2);
    }

    #[test]
    fn test_parse_trust_overrides() {
        let trusted = parse_args(&["--trust".into()]);
        assert_eq!(trusted.project_trust_override, Some(true));

        let not_trusted = parse_args(&["--no-trust".into()]);
        assert_eq!(not_trusted.project_trust_override, Some(false));
    }

    #[test]
    fn test_parse_default_trust() {
        let args = parse_args(&["--default-trust".into(), "always".into()]);
        assert_eq!(args.default_project_trust, DefaultProjectTrust::Always);
    }
}
