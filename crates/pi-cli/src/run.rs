//! CLI execution flow — runs the agent in text or JSON mode.
//!
//! Mirrors packages/coding-agent/src/main.ts

use std::io::IsTerminal;

use colored::*;

use crate::args::{print_help, CliArgs, OutputMode};
use crate::file_processor::process_file_arguments;
use crate::initial_message::{build_initial_message, InitialMessageInput};
use pi_coding_agent::core::extensions::{create_builtin_source_info, ExtensionRegistry};
use pi_coding_agent::core::model_registry::{ModelRegistry, ProviderConfig};
use pi_coding_agent::core::project_trust::{resolve_project_trusted, ProjectTrustContext};
use pi_coding_agent::core::sdk::{create_agent_session, CreateAgentSessionOptions};
use pi_coding_agent::core::session_manager::{NewSessionOptions, SessionManager};
use pi_coding_agent::core::trust_manager::ProjectTrustStore;

/// Exit code for successful runs.
const EXIT_SUCCESS: i32 = 0;
/// Exit code for runtime errors.
const EXIT_FAILURE: i32 = 1;

/// Resolved application mode (matching TS `AppMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppMode {
    Print,
    Interactive,
    Json,
    Rpc,
    Acp,
}

/// Resolve the application mode, matching TS `resolveAppMode`:
/// rpc/json/acp are explicit; otherwise `--print` or a non-TTY stdin/stdout
/// forces print mode; a TTY defaults to interactive.
fn resolve_app_mode(args: &CliArgs, stdin_is_tty: bool, stdout_is_tty: bool) -> AppMode {
    match args.mode {
        OutputMode::Rpc => AppMode::Rpc,
        OutputMode::Json => AppMode::Json,
        OutputMode::Acp => AppMode::Acp,
        OutputMode::Interactive => AppMode::Interactive,
        OutputMode::Text => {
            if args.print || !stdin_is_tty || !stdout_is_tty {
                AppMode::Print
            } else {
                AppMode::Interactive
            }
        }
    }
}

/// Main entry point.
pub async fn run(args: &CliArgs) -> i32 {
    if args.help {
        print_help();
        return EXIT_SUCCESS;
    }

    if args.version {
        println!("{} v{}", pi_coding_agent::config::APP_NAME, pi_coding_agent::config::VERSION);
        return EXIT_SUCCESS;
    }

    // --offline: skip network (matching TS: sets PI_OFFLINE + PI_SKIP_VERSION_CHECK).
    if args.offline {
        std::env::set_var("PI_OFFLINE", "1");
        std::env::set_var("PI_SKIP_VERSION_CHECK", "1");
    }

    // --export <session-file> [output-path]: export a session file to HTML
    // (matching TS main.ts `parsed.export` → `exportFromFile`).
    if let Some(ref export_file) = args.export {
        return export_session_file(export_file, args.messages.first().map(|s| s.as_str())).await;
    }

    // --list-models [pattern]
    if let Some(ref pattern) = args.list_models {
        let search = if pattern.is_empty() {
            None
        } else {
            Some(pattern.as_str())
        };
        return list_available_models(search).await;
    }

    // Report parse diagnostics (matching TS: warnings to stderr).
    for d in &args.diagnostics {
        eprintln!("{} {d}", "Warning:".yellow().bold());
    }

    // --name requires a non-empty value (matching TS).
    if let Some(ref name) = args.name {
        if name.trim().is_empty() {
            eprintln!("{} --name requires a non-empty value", "Error:".red().bold());
            return EXIT_FAILURE;
        }
    }

    // validateForkFlags / validateSessionIdFlags (matching TS).
    if args.fork.is_some() {
        let conflicts = conflicting_flags(args, &["--session", "--continue", "--resume", "--no-session"]);
        if !conflicts.is_empty() {
            eprintln!(
                "{} --fork cannot be combined with {}",
                "Error:".red().bold(),
                conflicts.join(", ")
            );
            return EXIT_FAILURE;
        }
    }
    if args.session_id.is_some() {
        let conflicts = conflicting_flags(args, &["--session", "--continue", "--resume"]);
        if !conflicts.is_empty() {
            eprintln!(
                "{} --session-id cannot be combined with {}",
                "Error:".red().bold(),
                conflicts.join(", ")
            );
            return EXIT_FAILURE;
        }
    }

    let agent_dir = pi_coding_agent::config::get_agent_dir();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());

    if args.verbose {
        eprintln!("{} pi-coding-agent v{}", "[pi]".dimmed(), pi_coding_agent::config::VERSION);
        eprintln!("{} cwd: {}", "[pi]".dimmed(), cwd);
        eprintln!("{} agent_dir: {}", "[pi]".dimmed(), agent_dir.to_string_lossy());
    }

    // Resolve the app mode (matching TS `resolveAppMode`).
    let stdin_is_tty = std::io::stdin().is_terminal();
    let stdout_is_tty = std::io::stdout().is_terminal();
    let app_mode = resolve_app_mode(args, stdin_is_tty, stdout_is_tty);

    // Subcommand handling (install, remove, list, config, refresh)
    if let Some(ref cmd) = args.subcommand {
        return handle_subcommand(cmd, &args.subcommand_args).await;
    }

    // Interactive TUI mode creates its own session
    if app_mode == AppMode::Interactive {
        #[cfg(feature = "interactive")]
        {
            return run_interactive_mode_with_session(&cwd, &agent_dir.to_string_lossy(), args).await;
        }
        #[cfg(not(feature = "interactive"))]
        {
            eprintln!("{} Interactive TUI mode requires building with the `interactive` feature.", "Error:".red().bold());
            return EXIT_FAILURE;
        }
    }

    // RPC mode creates its own session internally
    if app_mode == AppMode::Rpc {
        // @file arguments are not supported in RPC mode (matching TS).
        if !args.file_args.is_empty() {
            eprintln!(
                "{} @file arguments are not supported in RPC mode",
                "Error:".red().bold()
            );
            return EXIT_FAILURE;
        }
        return pi_coding_agent::modes::rpc::run_rpc_mode(
            args.extensions.clone(),
            args.unknown_flags.clone(),
        )
        .await;
    }

    // ACP mode: speak the Agent Client Protocol over stdio (Zed, JetBrains, …)
    if app_mode == AppMode::Acp {
        return pi_coding_agent::modes::acp::run_acp_mode().await;
    }

    // Read piped stdin (matching TS `readPipedStdin`; skipped for rpc/acp).
    let stdin_content = if !stdin_is_tty {
        read_piped_stdin().await
    } else {
        None
    };

    // Build the initial message: stdin → @file text → first CLI message
    // (matching TS `prepareInitialMessage` + `buildInitialMessage`).
    let (initial_message, initial_images) = prepare_initial_message(args, &cwd, stdin_content).await;

    // ── Project trust ────────────────────────────────────────────────────
    let trust_store = ProjectTrustStore::new(&agent_dir.to_string_lossy());
    let trusted = resolve_project_trusted(
        pi_coding_agent::core::project_trust::ResolveProjectTrustedOptions {
            cwd: &cwd,
            trust_store: &trust_store,
            trust_override: args.project_trust_override,
            default_project_trust: args.default_project_trust,
            project_trust_context: ProjectTrustContext::new(&cwd, false),
            extension_registry: None,
        },
    );

    if !trusted {
        eprintln!("{} Project not trusted. Use --trust to override.", "Error:".red().bold());
        return EXIT_FAILURE;
    }

    let message = initial_message.unwrap_or_default();
    if message.trim().is_empty() {
        eprintln!("{} No message provided. Use -h for help.", "Error:".red().bold());
        return EXIT_FAILURE;
    }
    // The first CLI message was consumed into `initial_message` (matching TS
    // `buildInitialMessage` which shifts `parsed.messages`).
    let remaining_messages: Vec<String> = if args.messages.is_empty() {
        Vec::new()
    } else {
        args.messages[1..].to_vec()
    };

    // ── Resolve session options from CLI args ────────────────────────────
    let (persist_session, session_file, fork_from, session_dir, session_manager) =
        resolve_session_opts(args, &cwd).await;

    // Build the model registry, registering the `--api-key` provider if given
    // (matching TS `modelRuntime.setRuntimeApiKey`).
    let model_registry = match build_model_registry(args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            return EXIT_FAILURE;
        }
    };

    // Resolve `--models` scope patterns (matching TS `resolveModelScope`).
    let scoped_models = if args.models.is_empty() {
        None
    } else {
        let scoped = pi_coding_agent::core::model_resolver::resolve_model_scope(
            &args.models,
            &model_registry.get_models(),
        );
        Some(
            scoped
                .into_iter()
                .map(|s| (s.model, s.thinking_level))
                .collect::<Vec<_>>(),
        )
    };

    // Build SDK options
    let sdk_options = CreateAgentSessionOptions {
        cwd: cwd.clone(),
        agent_dir: Some(agent_dir.to_string_lossy().to_string()),
        model: None,
        thinking_level: None,
        scoped_models,
        no_tools: if args.no_tools {
            Some(pi_coding_agent::core::sdk::NoToolsMode::All)
        } else if args.no_builtin_tools {
            Some(pi_coding_agent::core::sdk::NoToolsMode::Builtin)
        } else {
            None
        },
        tools: if args.tools.is_empty() { None } else { Some(args.tools.clone()) },
        exclude_tools: if args.exclude_tools.is_empty() { None } else { Some(args.exclude_tools.clone()) },
        custom_prompt: args.system_prompt.clone(),
        append_system_prompt: if args.append_system_prompt.is_empty() { None } else { Some(args.append_system_prompt.join("\n")) },
        session_name: args.name.clone(),
        stream_fn: None,
        convert_to_llm: None,
        extension_paths: args.extensions.clone(),
        extension_flags: Some(args.unknown_flags.clone()),
        enable_extensions: !args.no_extensions,
        extension_registry: {
            let mut reg = ExtensionRegistry::new();
            reg.register(
                Box::new(pi_extensions::goal::GoalExtension::new()),
                create_builtin_source_info("goal"),
            );
            reg.register(
                Box::new(pi_extensions::subagent::SubagentExtension::new()),
                create_builtin_source_info("subagent"),
            );
            Some(reg)
        },
        persist_session,
        session_file,
        fork_from,
        session_dir,
        cli_provider: args.provider.clone(),
        cli_model: args.model.clone(),
        auth_storage: None,
        model_registry: Some(model_registry),
        resource_loader: Some(pi_coding_agent::core::resource_loader::ResourceLoaderOptions {
            cwd: cwd.clone(),
            agent_dir: Some(agent_dir.to_string_lossy().to_string()),
            include_defaults: true,
            skill_paths: args.skills.clone(),
            prompt_paths: args.prompt_templates.clone(),
            extension_paths: args.extensions.clone(),
            theme_paths: args.themes.clone(),
            no_extensions: args.no_extensions,
            no_skills: args.no_skills,
            no_prompts: args.no_prompt_templates,
            no_themes: args.no_themes,
            no_context_files: args.no_context_files,
            system_prompt: args.system_prompt.clone(),
            append_system_prompt: args.append_system_prompt.clone(),
        }),
        session_manager,
        settings_manager: None,
        session_start_event: None,
        ui_context: None,
        custom_tools: None,
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

    let mode_str = match app_mode {
        AppMode::Json => "json",
        _ => "text",
    };
    let print_opts = pi_coding_agent::modes::print_mode::PrintModeOptions {
        mode: mode_str,
        message: &message,
        messages: &remaining_messages,
        images: if initial_images.is_empty() { None } else { Some(&initial_images) },
        session,
        verbose: args.verbose,
    };
    pi_coding_agent::modes::print_mode::run_print_mode(print_opts).await
}

/// Run interactive TUI mode with a session.
#[cfg(feature = "interactive")]
async fn run_interactive_mode_with_session(cwd: &str, agent_dir: &str, args: &CliArgs) -> i32 {
    let (persist_session, session_file, fork_from, session_dir, session_manager) =
        resolve_session_opts(args, cwd).await;

    let model_registry = match build_model_registry(args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            return EXIT_FAILURE;
        }
    };

    let sdk_options = CreateAgentSessionOptions {
        cwd: cwd.to_string(),
        agent_dir: Some(agent_dir.to_string()),
        model: None,
        thinking_level: None,
        scoped_models: None,
        no_tools: if args.no_tools {
            Some(pi_coding_agent::core::sdk::NoToolsMode::All)
        } else if args.no_builtin_tools {
            Some(pi_coding_agent::core::sdk::NoToolsMode::Builtin)
        } else {
            None
        },
        tools: if args.tools.is_empty() { None } else { Some(args.tools.clone()) },
        exclude_tools: if args.exclude_tools.is_empty() { None } else { Some(args.exclude_tools.clone()) },
        custom_prompt: args.system_prompt.clone(),
        append_system_prompt: if args.append_system_prompt.is_empty() { None } else { Some(args.append_system_prompt.join("\n")) },
        session_name: args.name.clone(),
        stream_fn: None,
        convert_to_llm: None,
        extension_paths: args.extensions.clone(),
        extension_flags: Some(args.unknown_flags.clone()),
        enable_extensions: !args.no_extensions,
        extension_registry: {
            let mut reg = ExtensionRegistry::new();
            reg.register(
                Box::new(pi_extensions::goal::GoalExtension::new()),
                create_builtin_source_info("goal"),
            );
            reg.register(
                Box::new(pi_extensions::subagent::SubagentExtension::new()),
                create_builtin_source_info("subagent"),
            );
            Some(reg)
        },
        persist_session,
        session_file,
        fork_from,
        session_dir,
        cli_provider: args.provider.clone(),
        cli_model: args.model.clone(),
        custom_tools: None,
        auth_storage: None,
        model_registry: Some(model_registry),
        resource_loader: None,
        session_manager,
        settings_manager: None,
        session_start_event: None,
        ui_context: None,
    };

    let (session, _result) = match create_agent_session(sdk_options).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} Failed to create session: {e}", "Error:".red().bold());
            return EXIT_FAILURE;
        }
    };

    pi_coding_agent::modes::interactive::run_interactive_mode(session).await
}

/// Resolve session persistence options from CLI arguments.
///
/// Returns `(persist_session, session_file, fork_from, session_dir,
/// session_manager)`. A `session_manager` is returned only when `--session-id`
/// is given (matching TS `createSessionManager`).
async fn resolve_session_opts(
    args: &CliArgs,
    cwd: &str,
) -> (bool, Option<String>, Option<String>, Option<String>, Option<SessionManager>) {
    let persist_session = if args.no_session {
        false
    } else if args.session.is_some() || args.fork.is_some() || args.continue_session || args.resume_session {
        true
    } else {
        // Persistent by default in interactive mode
        args.mode == OutputMode::Interactive
    };

    // --session <arg>: a path, or a session id prefix resolved against the
    // local session list (matching TS `resolveSessionPath`).
    let session_file = if let Some(ref s) = args.session {
        if s.contains('/') || s.contains('\\') || s.ends_with(".jsonl") {
            Some(pi_coding_agent::config::resolve_path(s))
        } else {
            let sessions = SessionManager::list(cwd, args.session_dir.as_deref()).await;
            sessions
                .iter()
                .find(|si| si.id == *s || si.id.starts_with(s))
                .map(|si| si.path.to_string_lossy().to_string())
                .or_else(|| Some(s.to_string()))
        }
    } else if (args.continue_session || args.resume_session) && !args.no_session {
        let sessions = SessionManager::list(cwd, args.session_dir.as_deref()).await;
        sessions.first().map(|s| s.path.to_string_lossy().to_string())
    } else {
        None
    };

    let fork_from = if args.no_session {
        None
    } else if let Some(ref f) = args.fork {
        // `--fork` accepts a path or a session id prefix (matching TS
        // `resolveSessionPath`).
        if f.contains('/') || f.contains('\\') || f.ends_with(".jsonl") {
            Some(pi_coding_agent::config::resolve_path(f))
        } else {
            let sessions = SessionManager::list(cwd, args.session_dir.as_deref()).await;
            sessions
                .iter()
                .find(|si| si.id == *f || si.id.starts_with(f))
                .map(|si| si.path.to_string_lossy().to_string())
                .or_else(|| Some(f.to_string()))
        }
    } else {
        None
    };

    let session_dir = if args.no_session {
        None
    } else {
        args.session_dir.clone()
    };

    // --session-id: fork with a specific id, or an in-memory session with that
    // id (matching TS `createSessionManager`).
    let session_manager = if let Some(ref sid) = args.session_id {
        let opts = NewSessionOptions {
            id: Some(sid.clone()),
            parent_session: None,
        };
        if let Some(ref fork) = fork_from {
            match SessionManager::fork_from(fork, cwd, session_dir.as_deref(), Some(opts)) {
                Ok(sm) => Some(sm),
                Err(e) => {
                    eprintln!("{} Failed to fork session: {e}", "Error:".red().bold());
                    None
                }
            }
        } else {
            let dir = session_dir
                .clone()
                .unwrap_or_else(|| SessionManager::default_session_dir(cwd, &pi_coding_agent::config::get_agent_dir().to_string_lossy()));
            Some(SessionManager::new(cwd, &dir, None, false, Some(opts)))
        }
    } else {
        None
    };

    (persist_session, session_file, fork_from, session_dir, session_manager)
}

/// Build the model registry, registering the `--api-key` provider if given
/// (matching TS `modelRuntime.setRuntimeApiKey`).
fn build_model_registry(args: &CliArgs) -> Result<ModelRegistry, String> {
    let registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
    if let Some(key) = &args.api_key {
        // Resolve the provider: --provider, the provider of --model, or the
        // first model matched by --models (matching TS, where `--api-key`
        // requires a resolved `sessionOptions.model`).
        let provider = if let Some(p) = &args.provider {
            p.clone()
        } else if let Some(m) = &args.model {
            registry
                .get_models()
                .iter()
                .find(|m2| m2.id == *m)
                .map(|m2| m2.provider.clone())
                .ok_or_else(|| {
                    "--api-key requires a model to be specified via --model, --provider/--model, or --models"
                        .to_string()
                })?
        } else if !args.models.is_empty() {
            let scoped = pi_coding_agent::core::model_resolver::resolve_model_scope(
                &args.models,
                &registry.get_models(),
            );
            scoped
                .first()
                .map(|s| s.model.provider.clone())
                .ok_or_else(|| {
                    "--api-key requires a model to be specified via --model, --provider/--model, or --models"
                        .to_string()
                })?
        } else {
            return Err(
                "--api-key requires a model to be specified via --model, --provider/--model, or --models"
                    .to_string(),
            );
        };
        registry.register_provider(
            &provider,
            ProviderConfig {
                name: None,
                base_url: None,
                api_key: Some(key.clone()),
                api: None,
                headers: None,
                auth_header: None,
            },
        );
    }
    Ok(registry)
}

/// Read all content from piped stdin (matching TS `readPipedStdin`).
/// Returns None if stdin is a TTY or there is no content.
async fn read_piped_stdin() -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut data = String::new();
    if tokio::io::stdin().read_to_string(&mut data).await.is_err() {
        return None;
    }
    let trimmed = data.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Build the initial message from stdin, @file args, and the first CLI
/// message (matching TS `prepareInitialMessage` + `buildInitialMessage`).
async fn prepare_initial_message(
    args: &CliArgs,
    cwd: &str,
    stdin_content: Option<String>,
) -> (Option<String>, Vec<pi_coding_agent::pi_agent_core::pi_ai_types::ContentBlock>) {
    let (file_text, file_images) = if args.file_args.is_empty() {
        (None, Vec::new())
    } else {
        let processed = process_file_arguments(&args.file_args, cwd);
        (Some(processed.text), processed.images)
    };
    let result = build_initial_message(InitialMessageInput {
        parsed: args,
        file_text,
        file_images,
        stdin_content,
    });
    let images: Vec<pi_coding_agent::pi_agent_core::pi_ai_types::ContentBlock> = result
        .initial_images
        .into_iter()
        .map(|img| pi_coding_agent::pi_agent_core::pi_ai_types::ContentBlock::Image {
            data: img.data,
            mime_type: img.mime_type,
        })
        .collect();
    (result.initial_message, images)
}

/// `--export <session-file> [output-path]`: export a session file to HTML,
/// matching TS `exportFromFile`.
async fn export_session_file(input_path: &str, output_path: Option<&str>) -> i32 {
    let resolved = pi_coding_agent::config::resolve_path(input_path);
    if !std::path::Path::new(&resolved).exists() {
        eprintln!("{} File not found: {resolved}", "Error:".red().bold());
        return EXIT_FAILURE;
    }
    let mgr = SessionManager::open(&resolved, None, None);
    let html = pi_coding_agent::core::agent_session::render_session_html(&mgr);
    let path = output_path
        .map(|p| p.to_string())
        .unwrap_or_else(|| format!("session_{}.html", mgr.get_session_id()));
    match std::fs::write(&path, &html) {
        Ok(_) => {
            println!("Exported to: {path}");
            EXIT_SUCCESS
        }
        Err(e) => {
            eprintln!("{} Failed to write HTML file: {e}", "Error:".red().bold());
            EXIT_FAILURE
        }
    }
}

/// List the flags that conflict with a given flag (matching TS
/// `validateForkFlags` / `validateSessionIdFlags`).
fn conflicting_flags(args: &CliArgs, flags: &[&str]) -> Vec<String> {
    let mut conflicts = Vec::new();
    for f in flags {
        let present = match *f {
            "--session" => args.session.is_some(),
            "--continue" => args.continue_session,
            "--resume" => args.resume_session,
            "--no-session" => args.no_session,
            _ => false,
        };
        if present {
            conflicts.push(f.to_string());
        }
    }
    conflicts
}

/// Handle subcommands (install, remove, list).
/// Delegates to `handle_package_command` from the shared module
/// to avoid duplicating the package management logic.
async fn handle_subcommand(cmd: &str, args: &[String]) -> i32 {
    let agent_dir = pi_coding_agent::config::get_agent_dir();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());

    // Reconstruct full args so the shared module can re-parse them
    let mut full_args = vec![cmd.to_string()];
    full_args.extend(args.iter().cloned());

    let code = crate::package_manager_cli::handle_package_command(
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
        "refresh" => {
            handle_refresh_command(args, &cwd, &agent_dir).await
        }
        _ => {
            eprintln!("{} Unknown subcommand: {cmd}", "Error:".red().bold());
            EXIT_FAILURE
        }
    }
}

/// Handle the `refresh` subcommand: manually refresh the remote model catalog
/// (bounded subset of TS `ModelRuntime.refresh` / `remote-catalog-provider`).
/// Fetches the latest per-provider models from the catalog service, merges
/// them into the registry, and persists `models-store.json`.
///
/// Usage: `pi refresh [--force] [--catalog-url <url>] [--offline]`
async fn handle_refresh_command(args: &[String], cwd: &str, agent_dir: &std::path::Path) -> i32 {
    use pi_coding_agent::core::model_registry::ModelRegistry;
    use pi_coding_agent::core::remote_catalog;

    let mut force = false;
    let mut offline = false;
    let mut catalog_base_url = remote_catalog::DEFAULT_CATALOG_BASE_URL.to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--force" | "-f" => force = true,
            "--offline" => offline = true,
            "--catalog-url" => {
                i += 1;
                if i < args.len() {
                    catalog_base_url = args[i].clone();
                }
            }
            other => {
                eprintln!("{} Unknown refresh option: {other}", "Error:".red().bold());
                return EXIT_FAILURE;
            }
        }
        i += 1;
    }

    let registry = ModelRegistry::new(pi_coding_agent::core::model_registry::builtin_models());
    let store_path = agent_dir.join("models-store.json");

    if offline {
        // Restore cached catalogs without any network access.
        let store = remote_catalog::load_models_store(&store_path);
        let mut restored = 0;
        for (provider, entry) in &store {
            if !entry.models.is_empty() {
                restored += registry.upsert_models(provider, &entry.models);
            }
        }
        println!("Restored {restored} cached models from {}", store_path.display());
        return EXIT_SUCCESS;
    }

    println!(
        "Refreshing model catalogs from {} (force: {})…",
        catalog_base_url, force
    );
    let summary = remote_catalog::refresh_remote_catalog(
        &registry,
        &catalog_base_url,
        &store_path,
        force,
    )
    .await;

    println!(
        "Checked {} providers: {} updated ({} models added/changed), {} failed",
        summary.providers_checked,
        summary.providers_updated,
        summary.models_added_or_updated,
        summary.providers_failed
    );
    for e in &summary.errors {
        eprintln!("  {e}");
    }
    if summary.providers_failed > 0 {
        eprintln!("{} Some catalog refreshes failed; cached catalogs were kept.", "Warning:".yellow().bold());
    }

    let _ = cwd;
    EXIT_SUCCESS
}

/// Handle the `config` subcommand: show or set configuration values.
async fn handle_config_command(args: &[String], cwd: &str, agent_dir: &str) -> i32 {
    use pi_coding_agent::core::settings_manager::SettingsManager;

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
        println!("  thinking_level: {:?}", global.default_thinking_level);
        println!();
        println!("Project settings:");
        println!("  default_model: {:?}", project.default_model);
        println!("  default_provider: {:?}", project.default_provider);
        println!("  thinking_level: {:?}", project.default_thinking_level);

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
    // Ensure API providers are registered so pi-ai models are available
    pi_coding_agent::pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();

    let model_registry = pi_coding_agent::core::model_registry::ModelRegistry::new(
        pi_coding_agent::core::model_registry::ModelRegistry::builtin_models_list(),
    );

    crate::list_models::list_models(&model_registry, search).await
}
