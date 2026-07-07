//! CLI execution flow — runs the agent in text or JSON mode.
//!
//! Mirrors packages/coding-agent/src/main.ts

use colored::*;

use crate::cli::args::{print_help, CliArgs, OutputMode};
use crate::core::package_manager::{DefaultPackageManager, PackageManager};
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
        return list_available_models().await;
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

/// After installing a package, link it to the agent's extensions directory
/// so the RPC sidecar can discover it automatically.
fn link_extension_to_agent(source: &str, agent_dir: &str, cwd: &str, global: bool) -> Result<(), String> {
    // Find the installed package path
    let pkg_name = source.trim_start_matches("npm:").trim_start_matches("https://");
    let pkg_dir = if global {
        // Find global npm root
        let global_root = std::process::Command::new("npm")
            .args(["root", "-g"])
            .output()
            .map_err(|e| format!("npm root -g failed: {e}"))?;
        let root = String::from_utf8_lossy(&global_root.stdout).trim().to_string();
        std::path::Path::new(&root).join(pkg_name)
    } else {
        std::path::Path::new(cwd).join("node_modules").join(pkg_name)
    };

    if !pkg_dir.join("package.json").exists() {
        return Err(format!("Package not found at {}", pkg_dir.display()));
    }

    // Check if it has a pi manifest with extensions
    let pkg_json = std::fs::read_to_string(pkg_dir.join("package.json"))
        .map_err(|e| format!("Failed to read package.json: {e}"))?;
    let pkg: serde_json::Value = serde_json::from_str(&pkg_json)
        .map_err(|e| format!("Failed to parse package.json: {e}"))?;

    let has_pi_extensions = pkg.get("pi")
        .and_then(|pi| pi.get("extensions"))
        .is_some();

    if !has_pi_extensions {
        return Ok(()); // Not a pi extension, nothing to link
    }

    // Create agent extensions directory and symlink
    let ext_dir = std::path::Path::new(agent_dir).join("extensions");
    std::fs::create_dir_all(&ext_dir)
        .map_err(|e| format!("Failed to create {ext_dir:?}: {e}"))?;

    let pkg_name_short = pkg_name.split('/').last().unwrap_or(pkg_name);
    let link_path = ext_dir.join(format!("{pkg_name_short}.pkg"));

    // Remove old link if exists
    let _ = std::fs::remove_file(&link_path);

    // Create symlink
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&pkg_dir, &link_path)
            .map_err(|e| format!("Failed to link {pkg_name} to {ext_dir:?}: {e}"))?;
        println!("  {} Linked to {ext_dir:?}", "🔗".to_string());
    }
    #[cfg(not(unix))]
    {
        eprintln!("  {} Symlinking not supported on this platform", "!".yellow());
    }

    Ok(())
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
async fn handle_subcommand(cmd: &str, args: &[String]) -> i32 {
    match cmd {
        "install" => {
            if args.is_empty() {
                eprintln!("{} Usage: pi install <source>", "Error:".red().bold());
                return EXIT_FAILURE;
            }
            let source = &args[0];
            // Default to local install (project node_modules), --global (-g) for global
            let global = args.contains(&"-g".to_string()) || args.contains(&"--global".to_string());

            // Check npm availability
            if !crate::core::package_manager::is_npm_available() {
                eprintln!("{} npm is not available. Install Node.js first.", "Error:".red().bold());
                return EXIT_FAILURE;
            }

            let agent_dir = crate::config::get_agent_dir();
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string());

            let pm = DefaultPackageManager::new(
                &cwd,
                &agent_dir.to_string_lossy(),
            );

            println!("Installing {source}...");
            match pm.install(source, !global) {
                Ok(()) => {
                    println!("{} Installed {source}", "✓".green());
                    // Symlink to agent's extensions directory for discovery
                    if let Err(e) = link_extension_to_agent(source, &agent_dir.to_string_lossy(), &cwd, global) {
                        eprintln!("  {} Warning: could not link extension: {e}", "!".yellow());
                    }
                    EXIT_SUCCESS
                }
                Err(e) => {
                    eprintln!("{} Failed to install {source}: {e}", "Error:".red().bold());
                    EXIT_FAILURE
                }
            }
        }

        "remove" => {
            if args.is_empty() {
                eprintln!("{} Usage: pi remove <source>", "Error:".red().bold());
                return EXIT_FAILURE;
            }
            let source = &args[0];
            let global = args.contains(&"-g".to_string()) || args.contains(&"--global".to_string());
            let agent_dir = crate::config::get_agent_dir();
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string());
            let pm = DefaultPackageManager::new(
                &cwd,
                &agent_dir.to_string_lossy(),
            );
            match pm.remove(source, !global) {
                Ok(()) => {
                    println!("{} Removed {source}", "✓".green());
                    EXIT_SUCCESS
                }
                Err(e) => {
                    eprintln!("{} Failed to remove {source}: {e}", "Error:".red().bold());
                    EXIT_FAILURE
                }
            }
        }

        "list" => {
            let agent_dir = crate::config::get_agent_dir();
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string());

            // List npm packages
            let pm = DefaultPackageManager::new(&cwd, &agent_dir.to_string_lossy());
            let packages = pm.list_configured_packages();

            // List agent extensions (symlinked)
            let ext_dir = std::path::Path::new(&agent_dir).join("extensions");
            let ext_links: Vec<String> = if ext_dir.is_dir() {
                std::fs::read_dir(&ext_dir).ok()
                    .map(|e| e.flatten()
                        .map(|f| f.file_name().to_string_lossy().to_string())
                        .collect())
                    .unwrap_or_default()
            } else {
                vec![]
            };

            if packages.is_empty() && ext_links.is_empty() {
                println!("No extensions installed.");
                println!("\nUSAGE: pi install <source>");
                println!("  <source> can be:");
                println!("    npm:<package>   — Install from npm registry");
            } else {
                if !packages.is_empty() {
                    println!("npm packages:");
                    for pkg in &packages {
                        let path = pkg.installed_path.as_deref().unwrap_or("-");
                        println!("  {} [{}]: {path}", pkg.source, pkg.scope);
                    }
                }
                if !ext_links.is_empty() {
                    println!("\nagent extensions:");
                    for link in &ext_links {
                        println!("  {link}");
                    }
                }
            }
            EXIT_SUCCESS
        }

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

/// List available models.
async fn list_available_models() -> i32 {
    let agent_dir = crate::config::get_agent_dir();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/tmp".to_string());

    let settings_manager = crate::core::settings_manager::SettingsManager::create(
        &cwd,
        Some(&agent_dir.to_string_lossy()),
    );

    let model_registry =
        crate::core::model_registry::ModelRegistry::new(
            crate::core::model_registry::ModelRegistry::builtin_models_list(),
        );

    let available = model_registry.get_available();
    if available.is_empty() {
        eprintln!("No models available. Configure an API key first.");
        return EXIT_FAILURE;
    }

    println!("Available models:");
    for model in &available {
        let default_mark = if settings_manager.get_settings().default_model.as_deref() == Some(&model.id) {
            " (default)"
        } else {
            ""
        };
        println!("  {} [{:?}]{default_mark}", model.id, model.provider);
    }

    EXIT_SUCCESS
}
