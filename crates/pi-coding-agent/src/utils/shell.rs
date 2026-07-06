//! Shell configuration and output handling.
//!
//! Mirrors packages/coding-agent/src/utils/shell.ts

use std::path::Path;
use std::sync::Mutex;

/// Set of tracked detached child process PIDs.
static TRACKED_PIDS: std::sync::LazyLock<Mutex<Vec<u32>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

/// Sanitize binary output for display/storage.
/// Removes control characters (except \t, \n, \r) and Unicode format characters.
pub fn sanitize_binary_output(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            let code = c as u32;
            // Allow tab, newline, carriage return
            if code == 0x09 || code == 0x0A || code == 0x0D {
                return true;
            }
            // Filter out control characters
            if code <= 0x1F {
                return false;
            }
            // Filter out Unicode format characters
            if (0xFFF9..=0xFFFB).contains(&code) {
                return false;
            }
            true
        })
        .collect()
}

/// Track a detached child process PID for cleanup on shutdown.
pub fn track_detached_child_pid(pid: u32) {
    if let Ok(mut pids) = TRACKED_PIDS.lock() {
        pids.push(pid);
    }
}

/// Untrack a detached child process PID.
pub fn untrack_detached_child_pid(pid: u32) {
    if let Ok(mut pids) = TRACKED_PIDS.lock() {
        pids.retain(|&p| p != pid);
    }
}

/// Kill all tracked detached child processes.
pub fn kill_tracked_detached_children() {
    let pids: Vec<u32> = TRACKED_PIDS.lock().map(|p| p.clone()).unwrap_or_default();
    for pid in pids {
        kill_process_tree(pid);
    }
    if let Ok(mut p) = TRACKED_PIDS.lock() {
        p.clear();
    }
}

/// Kill a process and its children (Unix: SIGKILL, Windows: taskkill).
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        use std::process::Command;
        // Try killing the process group first, then individual
        let _ = Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let _ = Command::new("kill")
            .args(["-9", &format!("{pid}")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Resolve the shell command to use.
pub fn resolve_shell() -> String {
    #[cfg(windows)]
    {
        // Try common bash locations on Windows
        for path in &[
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ] {
            if Path::new(path).exists() {
                return path.to_string();
            }
        }
        "bash".to_string()
    }

    #[cfg(unix)]
    {
        if Path::new("/bin/bash").exists() {
            "/bin/bash".to_string()
        } else {
            "sh".to_string()
        }
    }
}

/// Get shell arguments for running a command.
pub fn shell_args() -> Vec<String> {
    vec!["-c".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_binary_output() {
        let input = "hello\x00world\x1btest\n";
        let result = sanitize_binary_output(input);
        assert!(result.contains("hello"));
        assert!(result.contains("test"));
        assert!(result.contains("\n"));
        assert!(!result.contains('\x00'));
        assert!(!result.contains('\x1b'));
    }

    #[test]
    fn test_sanitize_keeps_normal_text() {
        let input = "Hello, 世界! \t\r\n";
        let result = sanitize_binary_output(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_resolve_shell() {
        let shell = resolve_shell();
        assert!(!shell.is_empty());
    }

    #[test]
    fn test_shell_args() {
        let args = shell_args();
        assert_eq!(args, vec!["-c"]);
    }
}
