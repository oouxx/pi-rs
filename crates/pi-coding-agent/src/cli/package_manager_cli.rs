//! CLI package management commands (install, remove, list, update).
//!
//! Mirrors packages/coding-agent/src/cli/package-manager-cli.ts

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
    let mut local = false;
    let mut force = false;
    let mut update_all = false;
    let mut help = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--local" | "-l" => local = true,
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

    // install requires a source
    if command == PackageCommand::Install && source.is_none() {
        return None;
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

/// Handle a package command. Returns true if the command was consumed.
pub async fn handle_package_command(args: &[String]) -> bool {
    let parsed = match parse_package_command(args) {
        Some(cmd) => cmd,
        None => return false,
    };

    if parsed.help {
        print_package_command_help(&parsed.command);
        return true;
    }

    match parsed.command {
        PackageCommand::Install => {
            if let Some(source) = &parsed.source {
                println!("Installing extension: {}", source);
                // TODO: delegate to PackageManager::install_and_persist
            }
        }
        PackageCommand::Remove => {
            if let Some(source) = &parsed.source {
                println!("Removing extension: {}", source);
                // TODO: delegate to PackageManager::remove_and_persist
            }
        }
        PackageCommand::List => {
            println!("Installed extensions:");
            println!("  (no extensions installed)");
            // TODO: delegate to PackageManager to show installed list
        }
        PackageCommand::Update => {
            if parsed.update_all {
                println!("Updating all extensions...");
            } else {
                println!("Checking for updates...");
            }
            // TODO: delegate to self-update and extension update logic
        }
    }

    true
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
        assert!(result.is_none());
    }
}
