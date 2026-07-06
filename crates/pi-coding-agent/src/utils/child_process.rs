//! Child process spawning with tracking and cleanup.
//!
//! Mirrors packages/coding-agent/src/utils/child-process.ts

use std::process::{Child, Command, Output, Stdio};

/// Spawn a child process with the given command and args.
pub fn spawn_process(command: &str, args: &[String]) -> Result<Child, std::io::Error> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.spawn()
}

/// Spawn a child process synchronously and wait for output.
pub fn spawn_process_sync(command: &str, args: &[&str]) -> Result<Output, std::io::Error> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.output()
}

/// Spawn a child process with inherited stdio (for interactive use).
pub fn spawn_process_inherited(command: &str, args: &[String]) -> Result<Child, std::io::Error> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    cmd.spawn()
}

/// Wait for a child process to exit, handling pipe closure gracefully.
pub fn wait_for_child_process(mut child: Child) -> Result<Option<i32>, std::io::Error> {
    let status = child.wait()?;
    Ok(status.code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_process_sync_echo() {
        let result = spawn_process_sync("echo", &["hello"]);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "hello");
    }

    #[test]
    fn test_spawn_process_nonexistent() {
        let result = spawn_process("/nonexistent/command", &[]);
        assert!(result.is_err());
    }
}
