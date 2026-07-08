//! CLI execution flow — runs the agent in text or JSON mode.
//!
//! Mirrors packages/coding-agent/src/main.ts

use colored::*;

use crate::cli::args::{print_help, CliArgs, OutputMode};
use crate::core::project_trust::{resolve_project_trusted, ProjectTrustContext};
use crate::core::sdk::{create_agent_session, CreateAgentSessionOptions};
use crate::core::session_manager::SessionManager;
use crate::core::trust_manager::ProjectTrustStore;

/// Exit code for successful runs.
const EXIT_SUCCESS: i32 = 0;
/// Exit code for runtime errors.
const EXIT_FAILURE: i32 = 1;

/// Main entry point.
pub async fn run(args: &CliArgs) -> i32 {
    if args.help {
        print_help();
        return EXIT_SUCCESS;
    }

    if args.version {
        println!("{} v{}", crate::config::APP_NAME, crate::config::VERSION);
        return EXIT_SUCCESS;
    }

    if args.list_models {
        let search = args.messages.join(" ");
        let search_opt = if search.is_empty() {
            None
        } else {
            Some(search.as_str())
        };
        return list_available_models(search_opt).await;
    }

    let agent_dir = crate::config::get_agent_dir();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());

    if args.verbose {
        eprintln!("{} pi-coding-agent v{}", "[pi]".dimmed(), crate::config::VERSION);
        eprintln!("{} cwd: {}", "[pi]".dimmed(), cwd);
        eprintln!("{} agent_dir: {}", "[pi]".dimmed(), agent_dir.to_string_lossy());
    }

    // ── Project trust ────────────────────────────────────────────────────
    let trust_store = ProjectTrustStore::new(&agent_dir.to_string_lossy());
    let trusted = resolve_project_trusted(
        crate::core::project_trust::ResolveProjectTrustedOptions {
            cwd: &cwd,
            trust_store: &trust_store,
            trust_override: args.project_trust_override,
            default_project_trust: args.default_project_trust,
            project_trust_context: ProjectTrustContext::new(&cwd, false),
        },
    );
    // Subcommand handling (install, remove, list)
    if let Some(ref cmd) = args.subcommand {
        return handle_subcommand(cmd, &args.subcommand_args).await;
    }

    // Interactive TUI mode creates its own session
    if args.mode == OutputMode::Interactive {
        return run_interactive_mode_with_session(&cwd, &agent_dir.to_string_lossy(), args).await;
    }

    // RPC mode creates its own session internally
    if args.mode == OutputMode::Rpc {
        return crate::modes::rpc::run_rpc_mode().await;
    }

    if !trusted {
        eprintln!("{} Project not trusted. Use --trust to override.", "Error:".red().bold());
        return EXIT_FAILURE;
    }

    let message = args.messages.join(" ");
    if message.trim().is_empty() {
        eprintln!("{} No message provided. Use -h for help.", "Error:".red().bold());
        return EXIT_FAILURE;
    }

    // ── Resolve session options from CLI args ────────────────────────────
    let (persist_session, session_file, fork_from, session_dir) =
        resolve_session_opts(args, &cwd).await;

    // Build SDK options
    let sdk_options = CreateAgentSessionOptions {
        cwd: cwd.clone(),
        agent_dir: Some(agent_dir.to_string_lossy().to_string()),
        model: None,
        thinking_level: None,
        scoped_models: None,
        no_tools: None,
        tools: if args.tools.is_empty() { None } else { Some(args.tools.clone()) },
        exclude_tools: if args.exclude_tools.is_empty() { None } else { Some(args.exclude_tools.clone()) },
        custom_prompt: args.system_prompt.clone(),
        append_system_prompt: if args.append_system_prompt.is_empty() { None } else { Some(args.append_system_prompt.join("\n")) },
        session_name: args.name.clone(),
        stream_fn: None,
        convert_to_llm: None,
        extension_paths: args.extensions.clone(),
        enable_extensions: !args.no_extensions,
        persist_session,
        session_file,
        fork_from,
        session_dir,
        cli_provider: args.provider.clone(),
        cli_model: args.model.clone(),
    };

    let (session, result) = match create_agent_session(sdk_options).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} Failed to create session: {e}", "Error:".red().bold());
            return EXIT_FAILURE;
        }
    };

    if let Some(msg) = result.model_fallback_message {
        if args.verbose {
            eprintln!("{} {msg}", "[pi]".dimmed());
        }
    }

    let mode_str = match args.mode {
        OutputMode::Json => "json",
        _ => "text",
    };
    let print_opts = crate::modes::print_mode::PrintModeOptions {
        mode: mode_str,
        message: &message,
        messages: &[],
        session,
        verbose: args.verbose,
    };
    crate::modes::print_mode::run_print_mode(print_opts).await
}

/// Run interactive TUI mode with a session.
async fn run_interactive_mode_with_session(cwd: &str, agent_dir: &str, args: &CliArgs) -> i32 {
    let (persist_session, session_file, fork_from, session_dir) =
        resolve_session_opts(args, cwd).await;

    let sdk_options = CreateAgentSessionOptions {
        cwd: cwd.to_string(),
        agent_dir: Some(agent_dir.to_string()),
        model: None,
        thinking_level: None,
        scoped_models: None,
        no_tools: None,
        tools: if args.tools.is_empty() { None } else { Some(args.tools.clone()) },
        exclude_tools: if args.exclude_tools.is_empty() { None } else { Some(args.exclude_tools.clone()) },
        custom_prompt: args.system_prompt.clone(),
        append_system_prompt: if args.append_system_prompt.is_empty() { None } else { Some(args.append_system_prompt.join("\n")) },
        session_name: args.name.clone(),
        stream_fn: None,
        convert_to_llm: None,
        extension_paths: args.extensions.clone(),
        enable_extensions: !args.no_extensions,
        persist_session,
        session_file,
        fork_from,
        session_dir,
        cli_provider: args.provider.clone(),
        cli_model: args.model.clone(),
    };

    let (session, _result) = match create_agent_session(sdk_options).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} Failed to create session: {e}", "Error:".red().bold());
            return EXIT_FAILURE;
        }
    };

    crate::modes::interactive::run_interactive_mode(session).await
}

/// Resolve session persistence options from CLI arguments.
///
/// Returns `(persist_session, session_file, fork_from, session_dir)`.
async fn resolve_session_opts(
    args: &CliArgs,
    cwd: &str,
) -> (bool, Option<String>, Option<String>, Option<String>) {
    let persist_session = if args.no_session {
        false
    } else if args.session.is_some() || args.fork.is_some() {
        true
    } else if args.continue_session || args.resume_session {
        true
    } else {
        // Persistent by default in interactive mode
        args.mode == OutputMode::Interactive
    };

    // --continue / --resume: find the most recent session for this cwd
    let session_file = if args.session.is_some() {
        args.session.clone()
    } else if (args.continue_session || args.resume_session) && !args.no_session {
        let sessions = SessionManager::list(cwd, args.session_dir.as_deref()).await;
        sessions.first().map(|s| s.path.to_string_lossy().to_string())
    } else {
        None
    };

    let fork_from = if args.no_session {
        None
    } else {
        args.fork.clone()
    };

    let session_dir = if args.no_session {
        None
    } else {
        args.session_dir.clone()
    };

    (persist_session, session_file, fork_from, session_dir)
}

/// Handle subcommands (install, remove, list).
/// Delegates to `handle_package_command` from the shared module
/// to avoid duplicating the package management logic.
async fn handle_subcommand(cmd: &str, args: &[String]) -> i32 {
    let agent_dir = crate::config::get_agent_dir();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());

    // Reconstruct full args so the shared module can re-parse them
    let mut full_args = vec![cmd.to_string()];
    full_args.extend(args.iter().cloned());

    let code = crate::cli::package_manager_cli::handle_package_command(
        &full_args,
        &cwd,
        &agent_dir.to_string_lossy(),
    )
    .await;

    // handle_package_command returns -1 when the command is not a package
    // command (e.g. "config" or something unknown). Return the exit code
    // directly for known commands.
    if code >= 0 {
        return code;
    }

    // Not handled by the package manager module
    match cmd {
        "config" => {
            handle_config_command(args, &cwd, &agent_dir.to_string_lossy()).await
        }
        _ => {
            eprintln!("{} Unknown subcommand: {cmd}", "Error:".red().bold());
            EXIT_FAILURE
        }
    }
}

/// Handle the `config` subcommand: show or set configuration values.
async fn handle_config_command(args: &[String], cwd: &str, agent_dir: &str) -> i32 {
    use crate::core::settings_manager::SettingsManager;

    let settings = SettingsManager::create(cwd, Some(agent_dir));

    if args.is_empty() || args.first().map(|s| s.as_str()) == Some("list") {
        // Show current configuration
        let global = settings.get_global_settings();
        let project = settings.get_project_settings();

        println!("Configuration:");
        println!("  Agent directory: {agent_dir}");
        println!("  Working directory: {cwd}");
        println!();
        println!("Global settings:");
        println!("  default_model: {:?}", global.default_model);
        println!("  default_provider: {:?}", global.default_provider);
        println!("  thinking_level: {:?}", global.thinking_level);
        println!("  custom_system_prompt: {:?}", global.custom_system_prompt.as_ref().map(|_| "(set)"));
        println!();
        println!("Project settings:");
        println!("  default_model: {:?}", project.default_model);
        println!("  default_provider: {:?}", project.default_provider);
        println!("  thinking_level: {:?}", project.thinking_level);
        println!("  custom_system_prompt: {:?}", project.custom_system_prompt.as_ref().map(|_| "(set)"));

        EXIT_SUCCESS
    } else if args.len() >= 2 {
        // Set a configuration value: config <key> <value>
        let key = &args[0];
        let value = &args[1];

        match key.as_str() {
            "model" | "provider" | "theme" | "thinking_level" => {
                // These are stored in settings
                eprintln!("Setting {key} to {value}...");
                // TODO: wire up actual settings persistence
                EXIT_SUCCESS
            }
            _ => {
                eprintln!("{} Unknown config key: {key}", "Error:".red().bold());
                eprintln!("  Valid keys: model, provider, theme, thinking_level");
                EXIT_FAILURE
            }
        }
    } else {
        eprintln!("{} Usage: pi config [list|<key> <value>]", "Error:".red().bold());
        EXIT_FAILURE
    }
}

/// List available models, delegating to the `list_models` module.
async fn list_available_models(search: Option<&str>) -> i32 {
    let model_registry = crate::core::model_registry::ModelRegistry::new(
        crate::core::model_registry::ModelRegistry::builtin_models_list(),
    );

    crate::cli::list_models::list_models(&model_registry, search).await
}
