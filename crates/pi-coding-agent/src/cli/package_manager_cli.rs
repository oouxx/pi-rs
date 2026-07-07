//! CLI package management commands (install, remove, list, update).
//!
//! Mirrors packages/coding-agent/src/cli/package-manager-cli.ts

use colored::*;

use crate::core::package_manager::{is_npm_available, DefaultPackageManager, PackageManager};

/// Supported package commands.
#[derive(Debug, Clone, PartialEq)]
pub enum PackageCommand {
    Install,
    Remove,
    List,
    Update,
}

/// Parsed package command with options.
#[derive(Debug, Clone)]
pub struct ParsedPackageCommand {
    pub command: PackageCommand,
    pub source: Option<String>,
    pub local: bool,
    pub force: bool,
    pub update_all: bool,
    pub help: bool,
}

/// Parse raw CLI args into a package command.
pub fn parse_package_command(args: &[String]) -> Option<ParsedPackageCommand> {
    if args.is_empty() {
        return None;
    }

    let cmd_str = args[0].to_lowercase();
    let command = match cmd_str.as_str() {
        "install" => PackageCommand::Install,
        "remove" | "uninstall" => PackageCommand::Remove,
        "list" => PackageCommand::List,
        "update" | "upgrade" => PackageCommand::Update,
        _ => return None,
    };

    let mut source: Option<String> = None;
    let mut local = true; // default to local (project-level) install
    let mut force = false;
    let mut update_all = false;
    let mut help = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--local" | "-l" => local = true,
            "--global" | "-g" => local = false,
            "--force" | "-f" => force = true,
            "--all" | "-a" => update_all = true,
            "--help" | "-h" => help = true,
            s if !s.starts_with('-') => {
                if source.is_none() {
                    source = Some(s.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }

    Some(ParsedPackageCommand {
        command,
        source,
        local,
        force,
        update_all,
        help,
    })
}

/// Handle a package command. Returns an exit code (0 = success, 1 = failure,
/// -1 = command not recognized, caller should report an error).
pub async fn handle_package_command(args: &[String], cwd: &str, agent_dir: &str) -> i32 {
    let parsed = match parse_package_command(args) {
        Some(cmd) => cmd,
        None => return -1,
    };

    if parsed.help {
        print_package_command_help(&parsed.command);
        return 0;
    }

    match parsed.command {
        PackageCommand::Install => {
            let source = match &parsed.source {
                Some(s) => s,
                None => {
                    eprintln!("{} Usage: pi install <source>", "Error:".red().bold());
                    return 1;
                }
            };

            if !is_npm_available() {
                eprintln!("{} npm is not available. Install Node.js first.", "Error:".red().bold());
                return 1;
            }

            let pm = DefaultPackageManager::new(cwd, agent_dir);

            println!("Installing {source}...");
            match pm.install(source, parsed.local) {
                Ok(()) => {
                    println!("{} Installed {source}", "✓".green());
                    if let Err(e) = link_extension_to_agent(source, agent_dir, cwd, !parsed.local) {
                        eprintln!("  {} Warning: could not link extension: {e}", "!".yellow());
                    }
                    0
                }
                Err(e) => {
                    eprintln!("{} Failed to install {source}: {e}", "Error:".red().bold());
                    1
                }
            }
        }

        PackageCommand::Remove => {
            let source = match &parsed.source {
                Some(s) => s,
                None => {
                    eprintln!("{} Usage: pi remove <source>", "Error:".red().bold());
                    return 1;
                }
            };

            let pm = DefaultPackageManager::new(cwd, agent_dir);
            match pm.remove(source, parsed.local) {
                Ok(()) => {
                    println!("{} Removed {source}", "✓".green());
                    0
                }
                Err(e) => {
                    eprintln!("{} Failed to remove {source}: {e}", "Error:".red().bold());
                    1
                }
            }
        }

        PackageCommand::List => {
            let pm = DefaultPackageManager::new(cwd, agent_dir);
            let packages = pm.list_configured_packages();

            // List agent extensions (symlinked)
            let ext_dir = std::path::Path::new(agent_dir).join("extensions");
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
            0
        }

        PackageCommand::Update => {
            if parsed.update_all {
                println!("Updating all extensions...");
            } else {
                println!("Checking for updates...");
            }
            println!("Update not yet implemented");
            1
        }
    }
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

fn print_package_command_help(command: &PackageCommand) {
    match command {
        PackageCommand::Install => {
            println!("Usage: pi install <source> [options]");
            println!("Options:");
            println!("  --local, -l   Install from local path");
        }
        PackageCommand::Remove => {
            println!("Usage: pi remove <extension-name>");
        }
        PackageCommand::List => {
            println!("Usage: pi list");
            println!("List all installed extensions.");
        }
        PackageCommand::Update => {
            println!("Usage: pi update [options]");
            println!("Options:");
            println!("  --all, -a     Update all extensions");
            println!("  --force, -f   Force update");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_install_command() {
        let result = parse_package_command(&[String::from("install"), String::from("some-package")]);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, PackageCommand::Install);
        assert_eq!(cmd.source, Some("some-package".to_string()));
    }

    #[test]
    fn test_parse_list_command() {
        let result = parse_package_command(&[String::from("list")]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().command, PackageCommand::List);
    }

    #[test]
    fn test_parse_remove_command() {
        let result = parse_package_command(&[String::from("remove"), String::from("ext-to-remove")]);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, PackageCommand::Remove);
        assert_eq!(cmd.source, Some("ext-to-remove".to_string()));
    }

    #[test]
    fn test_parse_update_command() {
        let result = parse_package_command(&[String::from("update")]);
        assert!(result.is_some());
        assert_eq!(result.unwrap().command, PackageCommand::Update);
    }

    #[test]
    fn test_parse_update_all_command() {
        let result = parse_package_command(&[String::from("update"), String::from("--all")]);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, PackageCommand::Update);
        assert!(cmd.update_all);
    }

    #[test]
    fn test_parse_unknown_command() {
        let result = parse_package_command(&[String::from("unknown")]);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_install_missing_source() {
        let result = parse_package_command(&[String::from("install")]);
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command, PackageCommand::Install);
        assert!(cmd.source.is_none());
    }
}
