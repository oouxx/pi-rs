//! `pi update` — self-update the pi-rs binary from the latest GitHub release.
//!
//! Mirrors the update mechanism in `install.sh update`: detect OS/arch, fetch
//! the latest release from GitHub Releases, and atomically replace the running
//! binary with the matching `pi-rs-{os}-{arch}` asset. Driven from inside the
//! CLI so users don't have to re-run the installer script.
//!
//! Usage:
//!   pi update            Check and update to the latest release if newer
//!   pi update --check    Only report whether an update is available
//!   pi update --force    Update even if the version is the same

use std::path::Path;
use std::time::Duration;

use colored::*;

use pi_coding_agent::config;
use pi_coding_agent::utils::version_check::compare_package_versions;

const RELEASES_API: &str = "https://api.github.com/repos/oouxx/pi-rs/releases/latest";
const USER_AGENT: &str = concat!("pi-rs-update/", env!("CARGO_PKG_VERSION"));

/// Map Rust's target OS to the asset OS segment used by `install.sh`.
fn asset_os() -> Option<&'static str> {
    match std::env::consts::OS {
        "linux" => Some("linux"),
        "macos" => Some("macos"),
        "windows" => Some("windows"),
        _ => None,
    }
}

/// Map Rust's target arch to the asset arch segment used by `install.sh`.
fn asset_arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" | "amd64" => Some("x86_64"),
        "aarch64" | "arm64" => Some("aarch64"),
        _ => None,
    }
}

/// Latest release info parsed from the GitHub Releases API.
struct LatestRelease {
    /// Tag with any leading `v` stripped, for version comparison.
    version: String,
    /// Browser download URL of the matching asset (or a `latest` fallback).
    download_url: String,
}

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))
}

/// Fetch the latest release and the matching asset URL for this OS/arch.
async fn fetch_latest_release(client: &reqwest::Client) -> Result<LatestRelease, String> {
    let resp = client
        .get(RELEASES_API)
        .send()
        .await
        .map_err(|e| format!("failed to reach GitHub Releases API: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("GitHub Releases API returned HTTP {status}"));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse GitHub Releases API response: {e}"))?;

    let tag = json
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "no `tag_name` in GitHub Releases response".to_string())?
        .to_string();
    let version = tag.trim_start_matches('v').to_string();

    let asset_name = format!("pi-rs-{}-{}", asset_os().unwrap_or("unknown"), asset_arch().unwrap_or("unknown"));
    let download_url = json
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| {
                    a.get("name").and_then(serde_json::Value::as_str) == Some(asset_name.as_str())
                })
                .and_then(|a| {
                    a.get("browser_download_url").and_then(serde_json::Value::as_str)
                })
        })
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "https://github.com/oouxx/pi-rs/releases/download/{tag}/{asset_name}"
            )
        });

    Ok(LatestRelease {
        version,
        download_url,
    })
}

/// Download `url` to `dest`.
async fn download(client: &reqwest::Client, url: &str, dest: &Path) -> Result<(), String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("failed to download {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("download returned HTTP {status} for {url}"));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed to read download body: {e}"))?;
    std::fs::write(dest, &bytes).map_err(|e| format!("failed to write {}: {e}", dest.display()))
}

/// Replace the running binary with `downloaded`, atomically on Unix.
/// On Windows a running .exe cannot be replaced in place, so we stage the new
/// binary next to it and report the path for the user to swap on next launch.
fn replace_binary(downloaded: &Path) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate current executable: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current executable has no parent directory".to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(downloaded)
            .map_err(|e| format!("cannot stat downloaded binary: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(downloaded, perms)
            .map_err(|e| format!("cannot chmod downloaded binary: {e}"))?;
        // Rename over the running binary is safe on Unix: the process keeps its
        // mapped inode until exit; new launches get the updated file.
        std::fs::rename(downloaded, &exe).map_err(|e| {
            format!(
                "failed to replace {}: {e}. Try running with write access to {}.",
                exe.display(),
                dir.display()
            )
        })?;
        Ok(())
    }

    #[cfg(windows)]
    {
        let file_name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("pi-rs");
        let staged = dir.join(format!(".{file_name}.new"));
        std::fs::copy(downloaded, &staged)
            .map_err(|e| format!("failed to stage new binary: {e}"))?;
        return Err(format!(
            "Windows cannot replace a running executable. New binary staged at {}; replace {} on next launch.",
            staged.display(),
            exe.display()
        ));
    }
}

fn print_help() {
    println!("Usage: {} update [options]", config::APP_NAME);
    println!("Update the {} binary to the latest GitHub release.", config::APP_NAME);
    println!();
    println!("Options:");
    println!("  --check, -c   Only report whether an update is available");
    println!("  --force, -f   Update even if the installed version matches the latest");
    println!("  --help, -h    Show this help");
}

/// Parsed self-update options.
#[derive(Debug, Default, PartialEq, Eq)]
struct UpdateOptions {
    check_only: bool,
    force: bool,
    help: bool,
}

/// Parse self-update flags. `Err(msg)` for unknown flags or stray positional
/// arguments (a positional means the caller wanted extension update, which is
/// handled upstream in `handle_subcommand`).
fn parse_update_options(args: &[String]) -> Result<UpdateOptions, String> {
    let mut o = UpdateOptions::default();
    for a in args {
        match a.as_str() {
            "--check" | "-c" => o.check_only = true,
            "--force" | "-f" => o.force = true,
            "--help" | "-h" => o.help = true,
            s if s.starts_with('-') => {
                return Err(format!("unknown option: {s}"));
            }
            s => {
                return Err(format!(
                    "unexpected positional argument `{s}` (extension updates go through `pi update --all` / `pi update <source>`)"
                ));
            }
        }
    }
    Ok(o)
}

/// Run the self-update. Returns a process exit code.
pub async fn run_update(args: &[String]) -> i32 {
    let opts = match parse_update_options(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            print_help();
            return 1;
        }
    };
    if opts.help {
        print_help();
        return 0;
    }
    let check_only = opts.check_only;
    let force = opts.force;

    let Some(os) = asset_os() else {
        eprintln!(
            "{} Unsupported OS ({}) for auto-update.",
            "Error:".red().bold(),
            std::env::consts::OS
        );
        return 1;
    };
    let Some(arch) = asset_arch() else {
        eprintln!(
            "{} Unsupported architecture ({}) for auto-update.",
            "Error:".red().bold(),
            std::env::consts::ARCH
        );
        return 1;
    };

    let current = config::VERSION;
    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            return 1;
        }
    };

    println!(
        "{} {} v{current} — checking latest release ({os}/{arch})...",
        "Checking".cyan().bold(),
        config::APP_NAME
    );

    let latest = match fetch_latest_release(&client).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{} {e}", "Error:".red().bold());
            return 1;
        }
    };

    let newer = match compare_package_versions(&latest.version, current) {
        Some(cmp) => cmp > 0,
        None => latest.version != current,
    };

    if !newer && !force {
        println!(
            "{} Already up to date ({} v{current}).",
            "✓".green(),
            config::APP_NAME
        );
        return 0;
    }

    if check_only {
        println!(
            "{} {} v{} available (current: v{current}). Run `{} update` to install.",
            "↑".yellow().bold(),
            config::APP_NAME,
            latest.version,
            config::APP_NAME
        );
        return 0;
    }

    if force && !newer {
        println!(
            "{} Forcing reinstall of {} v{current} (matches latest release).",
            "!".yellow(),
            config::APP_NAME
        );
    } else {
        println!(
            "{} {} v{current} → v{}.",
            "Updating".green().bold(),
            config::APP_NAME,
            latest.version
        );
    }

    let exe = std::env::current_exe()
        .map_err(|e| {
            eprintln!("{} Cannot locate current executable: {e}", "Error:".red().bold());
        })
        .unwrap_or_default();
    if exe.as_os_str().is_empty() {
        return 1;
    }
    let Some(dir) = exe.parent() else {
        eprintln!("{} Cannot determine install directory.", "Error:".red().bold());
        return 1;
    };
    let file_name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("pi-rs");
    let tmp = dir.join(format!(".{file_name}.update.tmp"));

    println!("  Downloading {}...", latest.download_url);
    if let Err(e) = download(&client, &latest.download_url, &tmp).await {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("{} {e}", "Error:".red().bold());
        return 1;
    }

    if let Err(e) = replace_binary(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("{} {e}", "Error:".red().bold());
        return 1;
    }

    println!(
        "{} Updated {} to v{}. Restart {} to use the new version.",
        "✓".green(),
        config::APP_NAME,
        latest.version,
        config::APP_NAME
    );
    0
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn parse_defaults_to_update() {
        let o = parse_update_options(&[]).expect("no args");
        assert!(!o.check_only && !o.force && !o.help);
    }

    #[test]
    fn parse_check_and_force_flags() {
        let o = parse_update_options(&["--check".into(), "--force".into()]).expect("flags");
        assert!(o.check_only);
        assert!(o.force);
        // short aliases
        let o = parse_update_options(&["-c".into(), "-f".into()]).expect("short flags");
        assert!(o.check_only && o.force);
    }

    #[test]
    fn parse_help() {
        let o = parse_update_options(&["--help".into()]).expect("help");
        assert!(o.help);
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        assert!(parse_update_options(&["--bogus".into()]).is_err());
    }

    #[test]
    fn parse_rejects_positional_argument() {
        // A positional argument means the caller wants extension update
        // (handled upstream); self-update must not silently swallow it.
        assert!(parse_update_options(&["somepkg".into()]).is_err());
    }

    #[test]
    fn asset_mapping_supports_build_target() {
        // install.sh only ships linux/macos/windows × x86_64/aarch64. This
        // guards that the running build maps to a published asset.
        assert!(asset_os().is_some(), "unsupported OS: {}", std::env::consts::OS);
        assert!(asset_arch().is_some(), "unsupported arch: {}", std::env::consts::ARCH);
    }

    #[test]
    fn newer_decision_is_greater_than() {
        // Mirrors the `newer` computation in run_update.
        let newer = |latest: &str, cur: &str| match compare_package_versions(latest, cur) {
            Some(cmp) => cmp > 0,
            None => latest != cur,
        };
        assert!(newer("1.84.0", "1.83.7"));
        assert!(!newer("1.83.7", "1.83.7"));
        assert!(!newer("1.83.0", "1.83.7"));
        // non-semver fallback: different is "newer" (conservative reinstall)
        assert!(newer("next", "1.83.7"));
    }
}
