//! CLI execution flow — runs the agent in text or JSON mode.
//!
//! Mirrors packages/coding-agent/src/main.ts

use colored::*;

use crate::cli::args::{print_help, CliArgs, OutputMode};
use crate::core::project_trust::{resolve_project_trusted, ProjectTrustContext};
use crate::core::sdk::{create_agent_session, CreateAgentSessionOptions};
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
        session,
        verbose: args.verbose,
    };
    crate::modes::print_mode::run_print_mode(print_opts).await
}

/// Run interactive TUI mode with a session.
async fn run_interactive_mode_with_session(cwd: &str, agent_dir: &str, args: &CliArgs) -> i32 {
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
            eprintln!("Config subcommand not yet implemented in Rust port");
            EXIT_FAILURE
        }
        _ => {
            eprintln!("{} Unknown subcommand: {cmd}", "Error:".red().bold());
            EXIT_FAILURE
        }
    }
}

/// List available models, delegating to the `list_models` module.
async fn list_available_models(search: Option<&str>) -> i32 {
    let model_registry = crate::core::model_registry::ModelRegistry::new(
        crate::core::model_registry::ModelRegistry::builtin_models_list(),
    );

    crate::cli::list_models::list_models(&model_registry, search).await
}
