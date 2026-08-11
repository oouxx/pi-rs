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
pub static TEST_TRACK_LOCK: Mutex<()> = Mutex::new(());

/// Kill a process and its entire tree, matching TS `killProcessTree`
/// (`utils/shell.ts`): on Unix send SIGKILL to the process group (`-pid`)
/// since the child was spawned as a process-group leader (`detached` /
/// `process_group(0)`); on Windows use `taskkill /F /T`. Falls back to
/// killing just the child if the group kill fails (e.g. process already dead).
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
    #[test]
    fn kill_tracked_children_kills_tracked_pids() {
        if cfg!(target_os = "windows") {
            return;
        }
        let _guard = TEST_TRACK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
