//! Tools manager -- fd/rg tool path resolver and download manager.
//!
//! Mirrors packages/coding-agent/src/utils/tools-manager.ts

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a downloadable CLI tool.
struct ToolConfig {
    /// Human-readable name (e.g. "fd", "ripgrep").
    name: &'static str,
    /// GitHub repository (e.g. "sharkdp/fd").
    repo: &'static str,
    /// Name of the binary inside the release archive.
    binary_name: &'static str,
    /// Alternative system command names to try before downloading.
    system_binary_names: &'static [&'static str],
    /// Prefix for release tags (e.g. "v" for v1.0.0).
    tag_prefix: &'static str,
}

const TOOLS: &[ToolConfig] = &[
    ToolConfig {
        name: "fd",
        repo: "sharkdp/fd",
        binary_name: "fd",
        system_binary_names: &["fd", "fdfind"],
        tag_prefix: "v",
    },
    ToolConfig {
        name: "ripgrep",
        repo: "BurntSushi/ripgrep",
        binary_name: "rg",
        system_binary_names: &["rg"],
        tag_prefix: "",
    },
];

const NETWORK_TIMEOUT_SECS: u64 = 10;
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Look up the `ToolConfig` for a tool name or binary name.
fn get_tool_config(tool: &str) -> Option<&'static ToolConfig> {
    TOOLS.iter().find(|c| c.name == tool || c.binary_name == tool)
}

/// Check whether `cmd` exists in the system PATH.
fn command_exists(cmd: &str) -> bool {
    let probe = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    Command::new(probe)
        .arg(cmd)
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return the binary filename for the current platform (adds `.exe` on Windows).
fn binary_filename(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{}.exe", name)
    } else {
        name.to_string()
    }
}

/// Determine the GitHub release asset name for the given tool / version / platform.
fn get_asset_name(
    tool_name: &str,
    version: &str,
    plat: &str,
    arch: &str,
) -> Option<String> {
    let arch_str = if arch == "aarch64" { "aarch64" } else { "x86_64" };

    match tool_name {
        "fd" => match plat {
            "macos" => Some(format!("fd-v{version}-{arch_str}-apple-darwin.tar.gz")),
            "linux" => Some(format!("fd-v{version}-{arch_str}-unknown-linux-gnu.tar.gz")),
            "windows" => Some(format!("fd-v{version}-{arch_str}-pc-windows-msvc.zip")),
            _ => None,
        },
        "ripgrep" => match plat {
            "macos" => Some(format!("ripgrep-{version}-{arch_str}-apple-darwin.tar.gz")),
            "linux" if arch == "aarch64" => {
                Some(format!("ripgrep-{version}-aarch64-unknown-linux-gnu.tar.gz"))
            }
            "linux" => Some(format!("ripgrep-{version}-x86_64-unknown-linux-musl.tar.gz")),
            "windows" => Some(format!("ripgrep-{version}-{arch_str}-pc-windows-msvc.zip")),
            _ => None,
        },
        _ => None,
    }
}

/// Build a curl subprocess command.
fn curl_command() -> Command {
    let exe = if cfg!(target_os = "windows") { "curl.exe" } else { "curl" };
    let mut cmd = Command::new(exe);
    cmd.args(["-sS", "-L"]);
    cmd
}

/// Fetch the latest release version string from a GitHub repository.
fn get_latest_version(repo: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");

    let output = curl_command()
        .args([
            "--connect-timeout",
            &NETWORK_TIMEOUT_SECS.to_string(),
            "-H",
            &format!("User-Agent: {}/coding-agent", config::APP_NAME),
            "--max-time",
            &NETWORK_TIMEOUT_SECS.to_string(),
            &url,
        ])
        .output()
        .map_err(|e| format!("Failed to curl GitHub API: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("GitHub API request failed: {stderr}"));
    }

    let body = String::from_utf8_lossy(&output.stdout);
    let data: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse GitHub response: {e}"))?;

    let tag_name = data["tag_name"]
        .as_str()
        .ok_or_else(|| "Missing tag_name in GitHub response".to_string())?;

    Ok(tag_name.trim_start_matches('v').to_string())
}

/// Download a file from `url` to `dest` using curl.
fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    let output = curl_command()
        .args([
            "--connect-timeout",
            &NETWORK_TIMEOUT_SECS.to_string(),
            "--max-time",
            &DOWNLOAD_TIMEOUT_SECS.to_string(),
            "-o",
            &dest.to_string_lossy(),
            url,
        ])
        .output()
        .map_err(|e| format!("Failed to run curl: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Download failed: {stderr}"));
    }
    Ok(())
}

/// Run an extraction command and return an error message on failure.
fn run_extraction(args: &[&str]) -> Result<(), String> {
    let cmd_name = args[0];
    // The cmd_name is "tar" or "unzip" so we spell it literally.
    let output = Command::new(args[0])
        .args(&args[1..])
        .output()
        .map_err(|e| format!("Failed to run {cmd_name}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{cmd_name} failed: {stderr}"));
    }
    Ok(())
}

/// Extract a `.tar.gz` archive into `extract_dir`.
fn extract_tar_gz(archive_path: &Path, extract_dir: &Path) -> Result<(), String> {
    run_extraction(&[
        "tar",
        "xzf",
        &archive_path.to_string_lossy(),
        "-C",
        &extract_dir.to_string_lossy(),
    ])
}

/// Extract a `.zip` archive into `extract_dir`.
fn extract_zip(archive_path: &Path, extract_dir: &Path) -> Result<(), String> {
    // Try unzip first, fall back to tar (bsdtar on Windows / macOS).
    let result = run_extraction(&[
        "unzip",
        "-q",
        &archive_path.to_string_lossy(),
        "-d",
        &extract_dir.to_string_lossy(),
    ]);
    if result.is_ok() {
        return Ok(());
    }

    // Fallback: tar (bsdtar on many systems supports zip).
    run_extraction(&[
        "tar",
        "xf",
        &archive_path.to_string_lossy(),
        "-C",
        &extract_dir.to_string_lossy(),
    ])
}

/// Recursively search for a binary file under `root_dir`.
fn find_binary_recursively(root_dir: &Path, binary_name: &str) -> Option<PathBuf> {
    if !root_dir.is_dir() {
        return None;
    }

    let mut stack: Vec<PathBuf> = vec![root_dir.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            let path = entry.path();
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            } else if entry.file_name().to_string_lossy() == binary_name {
                return Some(path);
            }
        }
    }
    None
}

// ===========================================================================
// Public API
// ===========================================================================

/// Return the path to a tool, checking:
/// 1. The local tools directory (managed installation).
/// 2. The system PATH.
///
/// Returns `None` if the tool is unknown or cannot be found.
pub fn get_tool_path(tool: &str) -> Option<String> {
    let config = get_tool_config(tool)?;
    let bin_dir = config::get_bin_dir();

    // 1. Check local tools directory first -- our managed installation.
    let local_path = bin_dir.join(binary_filename(config.binary_name));
    if local_path.exists() {
        return Some(local_path.to_string_lossy().to_string());
    }

    // 2. Check system PATH.
    for name in config.system_binary_names {
        if command_exists(name) {
            return Some(name.to_string());
        }
    }

    None
}

/// Ensure a tool is available, downloading it if necessary.
///
/// First checks the local tools directory and system PATH. If the tool is not
/// found, attempts to download the latest release from GitHub and install it
/// into the local tools directory.
///
/// Returns `Some(path)` if the tool is now available, or `None` if it could
/// not be found or downloaded.
pub fn ensure_tool(tool: &str) -> Option<String> {
    // Offline mode -- skip download.
    if std::env::var("PI_OFFLINE").as_deref() == Ok("1") {
        return None;
    }

    // Fast path -- already available.
    if let Some(path) = get_tool_path(tool) {
        return Some(path);
    }

    let config = get_tool_config(tool)?;

    // Attempt download.
    match download_tool(config) {
        Ok(path) => Some(path),
        Err(_) => {
            eprintln!(
                "Warning: {} not found. Install it manually or set PI_OFFLINE=1 to suppress.",
                tool
            );
            None
        }
    }
}

/// Download a tool from GitHub releases and install it into the bin directory.
fn download_tool(config: &ToolConfig) -> Result<String, String> {
    let plat = std::env::consts::OS;
    let architecture = std::env::consts::ARCH;

    // Fetch latest version from GitHub.
    let mut version = get_latest_version(config.repo)?;

    // Special case: fd on macOS x86_64 is pinned to 10.3.0 (last compatible build).
    if config.name == "fd" && plat == "macos" && architecture == "x86_64" {
        version = "10.3.0".to_string();
    }

    // Determine asset name for this platform.
    let asset_name =
        get_asset_name(config.name, &version, plat, architecture).ok_or_else(|| {
            format!("Unsupported platform: {plat}/{architecture}")
        })?;

    // Create bin directory.
    let bin_dir = config::get_bin_dir();
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("Failed to create bin dir: {e}"))?;

    let download_url = format!(
        "https://github.com/{repo}/releases/download/{prefix}{version}/{asset}",
        repo = config.repo,
        prefix = config.tag_prefix,
        asset = &asset_name,
    );

    let archive_path = bin_dir.join(&asset_name);
    let binary_ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let binary_path = bin_dir.join(binary_filename(config.binary_name));

    // Download the archive.
    download_file(&download_url, &archive_path)?;

    // Use a unique temp directory for extraction to avoid races when tools
    // download concurrently.
    let extract_dir = bin_dir.join(format!(
        "extract_tmp_{}_{}_{}",
        config.binary_name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create extract dir: {e}"))?;

    let cleanup = || {
        let _ = std::fs::remove_file(&archive_path);
        let _ = std::fs::remove_dir_all(&extract_dir);
    };

    let result = (|| -> Result<(), String> {
        // Extract.
        if asset_name.ends_with(".tar.gz") {
            extract_tar_gz(&archive_path, &extract_dir)?;
        } else if asset_name.ends_with(".zip") {
            extract_zip(&archive_path, &extract_dir)?;
        } else {
            return Err(format!("Unsupported archive format: {asset_name}"));
        }

        // Find the binary inside the extracted tree.
        // Some archives nest under a versioned subdirectory.
        let expected_subdir_name = asset_name.trim_end_matches(".tar.gz").trim_end_matches(".zip");
        let expected_subdir = extract_dir.join(expected_subdir_name);

        let found = find_binary_recursively(&expected_subdir, &binary_filename(config.binary_name))
            .or_else(|| find_binary_recursively(&extract_dir, &binary_filename(config.binary_name)));

        let src = found.ok_or_else(|| {
            format!(
                "Binary {} not found in archive under {}",
                config.binary_name,
                extract_dir.display()
            )
        })?;

        std::fs::rename(&src, &binary_path)
            .map_err(|e| format!("Failed to move binary: {e}"))?;

        // Make executable on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("Failed to set permissions: {e}"))?;
        }

        Ok(())
    })();

    cleanup();
    result.map(|_| binary_path.to_string_lossy().to_string())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // get_tool_config
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_tool_config_known() {
        let fd = get_tool_config("fd").expect("fd config should exist");
        assert_eq!(fd.name, "fd");
        assert_eq!(fd.binary_name, "fd");
        assert_eq!(fd.repo, "sharkdp/fd");

        let rg = get_tool_config("rg").expect("rg config should exist");
        assert_eq!(rg.name, "ripgrep");
        assert_eq!(rg.binary_name, "rg");
        assert_eq!(rg.repo, "BurntSushi/ripgrep");

        // Look up by binary name
        let by_binary = get_tool_config("rg").expect("rg by binary name");
        assert_eq!(by_binary.name, "ripgrep");
    }

    #[test]
    fn test_get_tool_config_unknown() {
        assert!(get_tool_config("nonexistent").is_none());
        assert!(get_tool_config("").is_none());
    }

    #[test]
    fn test_get_tool_config_fullname() {
        let fd = get_tool_config("ripgrep").expect("ripgrep by full name");
        assert_eq!(fd.name, "ripgrep");
    }

    // -----------------------------------------------------------------------
    // get_tool_path
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_tool_path_unknown() {
        assert!(get_tool_path("nonexistent").is_none());
        assert!(get_tool_path("").is_none());
    }

    #[test]
    fn test_get_tool_path_for_unknown_tool_returns_none() {
        assert_eq!(get_tool_path("bogus_tool"), None);
    }

    #[test]
    fn test_get_tool_path_system_tool_when_in_path() {
        // For a known tool, the function should either find it or return None.
        let path = get_tool_path("fd");
        match path {
            Some(p) => assert!(!p.is_empty(), "found path must not be empty"),
            None => {} // acceptable when tool is not installed
        }
    }

    // -----------------------------------------------------------------------
    // binary_filename
    // -----------------------------------------------------------------------

    #[test]
    fn test_binary_filename_no_exe_on_unix() {
        // This test validates the branching logic; on non-Windows we expect
        // the name unchanged.
        if !cfg!(target_os = "windows") {
            assert_eq!(binary_filename("fd"), "fd");
            assert_eq!(binary_filename("rg"), "rg");
        }
    }

    #[test]
    fn test_binary_filename_exe_on_windows() {
        if cfg!(target_os = "windows") {
            assert_eq!(binary_filename("fd"), "fd.exe");
            assert_eq!(binary_filename("rg"), "rg.exe");
        }
    }

    // -----------------------------------------------------------------------
    // command_exists
    // -----------------------------------------------------------------------

    #[test]
    fn test_command_exists_sh() {
        if cfg!(unix) {
            assert!(command_exists("sh"), "sh should be in PATH on Unix");
        }
    }

    #[test]
    fn test_command_exists_bogus() {
        assert!(!command_exists("this_command_does_not_exist_xyzzy"));
    }

    // -----------------------------------------------------------------------
    // ensure_tool
    // -----------------------------------------------------------------------

    #[test]
    fn test_ensure_tool_unknown() {
        assert_eq!(ensure_tool("no_such_tool"), None);
        assert_eq!(ensure_tool(""), None);
    }

    #[test]
    fn test_ensure_tool_for_unknown_returns_none() {
        assert_eq!(ensure_tool("made_up_tool"), None);
    }

    #[test]
    fn test_ensure_tool_offline_mode() {
        std::env::set_var("PI_OFFLINE", "1");
        let result = ensure_tool("fd");
        assert!(result.is_none());
        std::env::remove_var("PI_OFFLINE");
    }

    // -----------------------------------------------------------------------
    // get_asset_name
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_asset_name_fd_macos() {
        let name = get_asset_name("fd", "10.3.0", "macos", "x86_64");
        assert_eq!(name.as_deref(), Some("fd-v10.3.0-x86_64-apple-darwin.tar.gz"));
    }

    #[test]
    fn test_get_asset_name_fd_macos_arm64() {
        let name = get_asset_name("fd", "10.3.0", "macos", "aarch64");
        assert_eq!(name.as_deref(), Some("fd-v10.3.0-aarch64-apple-darwin.tar.gz"));
    }

    #[test]
    fn test_get_asset_name_fd_linux() {
        let name = get_asset_name("fd", "9.0.0", "linux", "x86_64");
        assert_eq!(name.as_deref(), Some("fd-v9.0.0-x86_64-unknown-linux-gnu.tar.gz"));
    }

    #[test]
    fn test_get_asset_name_fd_windows() {
        let name = get_asset_name("fd", "9.0.0", "windows", "x86_64");
        assert_eq!(name.as_deref(), Some("fd-v9.0.0-x86_64-pc-windows-msvc.zip"));
    }

    #[test]
    fn test_get_asset_name_rg_macos() {
        let name = get_asset_name("ripgrep", "14.1.0", "macos", "x86_64");
        assert_eq!(name.as_deref(), Some("ripgrep-14.1.0-x86_64-apple-darwin.tar.gz"));
    }

    #[test]
    fn test_get_asset_name_rg_linux() {
        let name = get_asset_name("ripgrep", "14.1.0", "linux", "x86_64");
        assert_eq!(name.as_deref(), Some("ripgrep-14.1.0-x86_64-unknown-linux-musl.tar.gz"));
    }

    #[test]
    fn test_get_asset_name_rg_linux_arm64() {
        let name = get_asset_name("ripgrep", "14.1.0", "linux", "aarch64");
        assert_eq!(name.as_deref(), Some("ripgrep-14.1.0-aarch64-unknown-linux-gnu.tar.gz"));
    }

    #[test]
    fn test_get_asset_name_rg_windows() {
        let name = get_asset_name("ripgrep", "14.1.0", "windows", "x86_64");
        assert_eq!(name.as_deref(), Some("ripgrep-14.1.0-x86_64-pc-windows-msvc.zip"));
    }

    #[test]
    fn test_get_asset_name_unsupported_platform() {
        assert_eq!(get_asset_name("fd", "1.0.0", "freebsd", "x86_64"), None);
        assert_eq!(get_asset_name("ripgrep", "1.0.0", "freebsd", "x86_64"), None);
    }

    #[test]
    fn test_get_asset_name_unknown_tool() {
        assert_eq!(get_asset_name("bogus", "1.0.0", "linux", "x86_64"), None);
    }

    // -----------------------------------------------------------------------
    // find_binary_recursively
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_binary_recursively_nonexistent_dir() {
        let tmp = PathBuf::from("/tmp/nonexistent_dir_xyzzy_12345");
        assert_eq!(find_binary_recursively(&tmp, "fd"), None);
    }

    #[test]
    fn test_find_binary_recursively_in_temp() {
        let dir = std::env::temp_dir().join("test_find_binary_recursively");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("fd"), "binary").unwrap();

        let found = find_binary_recursively(&dir, "fd");
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("sub/fd"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    // -----------------------------------------------------------------------
    // ToolConfig integrity
    // -----------------------------------------------------------------------

    #[test]
    fn test_tool_configs_non_empty() {
        assert!(!TOOLS.is_empty(), "should have at least one tool config");
        for tc in TOOLS {
            assert!(!tc.name.is_empty());
            assert!(!tc.repo.is_empty());
            assert!(!tc.binary_name.is_empty());
            assert!(!tc.system_binary_names.is_empty());
        }
    }

    #[test]
    fn test_tool_configs_fd() {
        let fd = TOOLS.iter().find(|t| t.name == "fd").unwrap();
        assert!(fd.system_binary_names.contains(&"fd"));
        assert!(fd.system_binary_names.contains(&"fdfind"));
        assert_eq!(fd.tag_prefix, "v");
    }

    #[test]
    fn test_tool_configs_rg() {
        let rg = TOOLS.iter().find(|t| t.name == "ripgrep").unwrap();
        assert!(rg.system_binary_names.contains(&"rg"));
        assert_eq!(rg.tag_prefix, "");
    }
}
