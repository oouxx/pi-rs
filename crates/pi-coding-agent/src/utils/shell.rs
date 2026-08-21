//! Shell configuration and output handling.
//!
//! Mirrors packages/coding-agent/src/utils/shell.ts

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

/// Snapshot of currently tracked detached child PIDs (test helper, mirrors
/// the internal set; also used by tests to assert track/untrack wiring).
pub fn tracked_pids_snapshot() -> Vec<u32> {
    TRACKED_PIDS.lock().map(|p| p.clone()).unwrap_or_default()
}

/// Serializes tests that touch the global `TRACKED_PIDS` set. Without this,
/// a test calling `kill_tracked_detached_children()` would kill pids tracked
/// by other concurrently running tests (and `exec_tracks_and_untracks_pid`
/// would observe foreign pids).
pub static TEST_TRACK_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Kill a process and its entire tree, matching TS `killProcessTree`
/// (`utils/shell.ts`): on Unix send SIGKILL to the process group (`-pid`)
/// since the child was spawned as a process-group leader (`detached` /
/// `process_group(0)`); on Windows use `taskkill /F /T`. Falls back to
/// killing just the child if the group kill fails (e.g. process already dead).
/// Run a probe command (`where` on Windows / `which` on Unix) with a timeout,
/// hiding the console window on Windows (matching TS `spawnSync` with
/// `timeout: 5000` + `windowsHide: true`). Returns the first line of stdout on
/// success, or None on failure/timeout.
///
/// The unbounded `Command::output()` variant could hang forever if a PATH
/// entry points at a stuck executable — TS guards with a 5s timeout.
pub fn run_path_probe(probe: &str, arg: &str) -> Option<String> {
    let mut cmd = std::process::Command::new(probe);
    cmd.arg(arg).stdout(std::process::Stdio::piped());
    // Hide the console window (matching TS `windowsHide: true`).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;

    // 5s timeout (matching TS `timeout: 5000`).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    if !status.success() {
        return None;
    }

    // Probe output is tiny (a few paths), so reading after exit is safe.
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        use std::io::Read;
        let _ = out.read_to_string(&mut stdout);
    }
    let first = stdout.trim().split('\n').next()?.trim().to_string();
    if first.is_empty() {
        None
    } else {
        Some(first)
    }
}

pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        let pid = pid as i32;
        // Kill the whole process group (negative pid).
        let group_result = unsafe { libc::kill(-pid, libc::SIGKILL) };
        if group_result != 0 {
            // Fallback: kill just the child (matching TS fallback).
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_run_path_probe_finds_existing_binary() {
        // `which sh` (or `where` on Windows) must return a real path.
        let probe = if cfg!(target_os = "windows") {
            "where"
        } else {
            "which"
        };
        let arg = if cfg!(target_os = "windows") {
            "sh.exe"
        } else {
            "sh"
        };
        let result = run_path_probe(probe, arg);
        assert!(
            result.is_some(),
            "probe for {arg} must succeed on {probe}"
        );
    }

    #[test]
    fn test_run_path_probe_missing_binary_returns_none() {
        let probe = if cfg!(target_os = "windows") {
            "where"
        } else {
            "which"
        };
        // A name that cannot exist on PATH.
        let result = run_path_probe(probe, "pi-rs-definitely-not-a-real-binary-xyz");
        assert!(result.is_none());
    }

    #[test]
    fn test_run_path_probe_timeout_kills_stuck_probe() {
        // A probe that never exits must be killed after the 5s deadline
        // instead of hanging forever (matching TS `spawnSync` timeout).
        let start = std::time::Instant::now();
        let result = run_path_probe("sleep", "30");
        assert!(result.is_none());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "probe must time out, not hang"
        );
    }

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

    /// kill_tracked_detached_children must kill every tracked pid and clear
    /// the set (matching TS killTrackedDetachedChildren). Serialized against
    /// other tests sharing the global TRACKED_PIDS set.
    #[tokio::test]
    async fn kill_tracked_children_kills_tracked_pids() {
        if cfg!(target_os = "windows") {
            return;
        }
        let _guard = TEST_TRACK_LOCK.lock().await;
        let mut child = std::process::Command::new("sleep")
            .arg("61.5")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        track_detached_child_pid(pid);
        assert!(tracked_pids_snapshot().contains(&pid));

        kill_tracked_detached_children();
        assert!(tracked_pids_snapshot().is_empty());

        // Reap the zombie so the pid is truly gone (SIGKILL'd children stay
        // as zombies until waited on), then assert it is no longer alive.
        let _ = child.wait();
        let result = unsafe { libc::kill(pid as i32, 0) };
        assert_ne!(result, 0, "tracked process should have been killed");
    }
}
