use std::fmt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::core::tools::output_accumulator::{OutputAccumulator, OutputAccumulatorOptions};
use super::truncate::{format_size, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};

// ============================================================================
// Constants
// ============================================================================

const MAX_TIMEOUT_MS: u64 = 2_147_483_647;
const MAX_TIMEOUT_SECONDS: u64 = MAX_TIMEOUT_MS / 1000;
const BASH_UPDATE_THROTTLE_MS: u64 = 100;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashToolInput {
    pub command: String,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BashToolDetails {
    pub truncation: Option<TruncationResult>,
    pub full_output_path: Option<String>,
}

/// Context for spawning a bash command — can be modified by a spawn hook.
#[derive(Debug, Clone)]
pub struct BashSpawnContext {
    pub command: String,
    pub cwd: String,
    pub env: Vec<(String, String)>,
}

/// Hook to adjust command, cwd, or env before execution.
pub type BashSpawnHook = Arc<dyn Fn(BashSpawnContext) -> BashSpawnContext + Send + Sync>;

/// Options passed to [`BashOperations::exec`].
pub struct BashExecOptions {
    /// Callback invoked with raw bytes as they arrive from stdout/stderr.
    pub on_data: Option<Arc<dyn Fn(&[u8]) + Send + Sync>>,
    /// Signal receiver for cancellation.
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// Timeout in seconds (optional).
    pub timeout: Option<u64>,
    /// Environment variables.
    pub env: Option<Vec<(String, String)>>,
}

impl fmt::Debug for BashExecOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BashExecOptions")
            .field("timeout", &self.timeout)
            .field("signal", &self.signal.as_ref().map(|_| "Receiver"))
            .field("on_data", &self.on_data.as_ref().map(|_| "Fn"))
            .field("env", &self.env)
            .finish()
    }
}

/// Result of [`BashOperations::exec`].
#[derive(Debug, Clone)]
pub struct BashExecResult {
    pub exit_code: Option<i32>,
}

// ============================================================================
// BashOperations trait
// ============================================================================

/// Pluggable operations for the bash tool.
///
/// Override these to delegate command execution to remote systems (for example SSH).
pub trait BashOperations: Send + Sync {
    /// Execute a command and stream output via `on_data`.
    ///
    /// Returns the exit code (null if killed).
    fn exec(
        &self,
        command: &str,
        cwd: &str,
        options: BashExecOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<BashExecResult, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    >;
}

// ============================================================================
// LocalBashOperations
// ============================================================================

/// Shell configuration.
struct ShellConfig {
    shell: String,
    args: Vec<String>,
    command_transport: CommandTransport,
}

enum CommandTransport {
    Args,
    Stdin,
}

/// Resolve timeout in milliseconds, validating constraints.
fn resolve_timeout_ms(timeout: Option<u64>) -> Result<Option<u64>, String> {
    match timeout {
        None => Ok(None),
        Some(t) if t == 0 => {
            Err("Invalid timeout: must be a positive number of seconds".to_string())
        }
        Some(t) => {
            let timeout_ms = t.saturating_mul(1000);
            if timeout_ms > MAX_TIMEOUT_MS {
                Err(format!(
                    "Invalid timeout: maximum is {} seconds",
                    MAX_TIMEOUT_SECONDS
                ))
            } else {
                Ok(Some(timeout_ms))
            }
        }
    }
}

/// Get shell configuration for the current platform.
fn get_shell_config(shell_path: Option<&str>) -> ShellConfig {
    if let Some(path) = shell_path {
        let path_lower = path.to_lowercase();
        let shell_name = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // Determine if this shell supports stdin transport
        let supports_stdin = matches!(shell_name, "bash" | "zsh" | "sh" | "fish" | "nu");

        if cfg!(target_os = "windows") || path_lower.contains("cmd") {
            ShellConfig {
                shell: path.to_string(),
                args: vec!["/C".to_string()],
                command_transport: CommandTransport::Args,
            }
        } else if supports_stdin {
            ShellConfig {
                shell: path.to_string(),
                args: vec![],
                command_transport: CommandTransport::Stdin,
            }
        } else {
            ShellConfig {
                shell: path.to_string(),
                args: vec!["-c".to_string()],
                command_transport: CommandTransport::Args,
            }
        }
    } else if cfg!(target_os = "windows") {
        ShellConfig {
            shell: "cmd".to_string(),
            args: vec!["/C".to_string()],
            command_transport: CommandTransport::Args,
        }
    } else {
        // On Unix, use bash with stdin transport by default
        ShellConfig {
            shell: "bash".to_string(),
            args: vec![],
            command_transport: CommandTransport::Stdin,
        }
    }
}

/// Get the current process environment as a vector of key-value pairs.
fn get_shell_env() -> Vec<(String, String)> {
    std::env::vars().collect()
}

/// Resolve the spawn context, applying the spawn hook if provided.
fn resolve_spawn_context(
    command: &str,
    cwd: &str,
    spawn_hook: Option<&BashSpawnHook>,
) -> BashSpawnContext {
    let base = BashSpawnContext {
        command: command.to_string(),
        cwd: cwd.to_string(),
        env: get_shell_env(),
    };
    match spawn_hook {
        Some(hook) => hook(base),
        None => base,
    }
}

pub struct LocalBashOperations;

impl BashOperations for LocalBashOperations {
    fn exec(
        &self,
        command: &str,
        cwd: &str,
        options: BashExecOptions,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<BashExecResult, Box<dyn std::error::Error + Send + Sync>>,
                > + Send,
        >,
    > {
        let command = command.to_string();
        let cwd = cwd.to_string();
        let timeout_ms = match resolve_timeout_ms(options.timeout) {
            Ok(t) => t,
            Err(e) => {
                return Box::pin(async move {
                    Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        e,
                    )) as Box<dyn std::error::Error + Send + Sync>)
                });
            }
        };

        // Take ownership of signal so we can make it mutable
        let mut signal = options.signal;

        Box::pin(async move {
            // Check if working directory exists
            if !std::path::Path::new(&cwd).exists() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "Working directory does not exist: {}\nCannot execute bash commands.",
                        cwd
                    ),
                )) as Box<dyn std::error::Error + Send + Sync>);
            }

            // Check if aborted before starting
            if let Some(ref rx) = signal {
                if *rx.borrow() {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "aborted",
                    )) as Box<dyn std::error::Error + Send + Sync>);
                }
            }

            let shell_config = get_shell_config(None);
            let env = options.env.unwrap_or_else(get_shell_env);

            let mut cmd = tokio::process::Command::new(&shell_config.shell);
            cmd.current_dir(&cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env_clear();

            for (key, val) in &env {
                cmd.env(key, val);
            }

            match shell_config.command_transport {
                CommandTransport::Args => {
                    for arg in &shell_config.args {
                        cmd.arg(arg);
                    }
                    cmd.arg(&command);
                    cmd.stdin(Stdio::null());
                }
                CommandTransport::Stdin => {
                    for arg in &shell_config.args {
                        cmd.arg(arg);
                    }
                    cmd.stdin(Stdio::piped());
                }
            }

            // Set process group for Unix so we can kill the entire tree
            if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
                cmd.process_group(0);
            }

            let mut child = cmd
                .spawn()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

            // If using stdin transport, write the command to stdin
            if matches!(shell_config.command_transport, CommandTransport::Stdin) {
                if let Some(mut stdin) = child.stdin.take() {
                    tokio::spawn(async move {
                        use tokio::io::AsyncWriteExt;
                        let _ = stdin.write_all(command.as_bytes()).await;
                        let _ = stdin.shutdown().await;
                    });
                }
            }

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let on_data = options.on_data.clone();

            // Spawn tasks to read stdout and stderr, calling on_data for each chunk
            let stdout_handle = if let Some(mut out) = stdout {
                let on_data = on_data.clone();
                Some(tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    loop {
                        use tokio::io::AsyncReadExt;
                        match out.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Some(ref cb) = on_data {
                                    cb(&buf[..n]);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }))
            } else {
                None
            };

            let stderr_handle = if let Some(mut err) = stderr {
                Some(tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    loop {
                        use tokio::io::AsyncReadExt;
                        match err.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                if let Some(ref cb) = on_data {
                                    cb(&buf[..n]);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }))
            } else {
                None
            };

            // Wait for the process with timeout and cancellation support
            let exit_code = if let Some(ms) = timeout_ms {
                // With timeout
                let start = std::time::Instant::now();
                let result = loop {
                    // Check for cancellation
                    if let Some(ref mut rx) = signal {
                        if *rx.borrow() {
                            let _ = child.kill().await;
                            // Wait for stdout/stderr tasks to finish
                            if let Some(h) = stdout_handle {
                                let _ = h.await;
                            }
                            if let Some(h) = stderr_handle {
                                let _ = h.await;
                            }
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::Interrupted,
                                "aborted",
                            )) as Box<dyn std::error::Error + Send + Sync>);
                        }
                    }

                    let elapsed = start.elapsed().as_millis() as u64;
                    if elapsed >= ms {
                        let _ = child.kill().await;
                        // Wait for stdout/stderr tasks to finish
                        if let Some(h) = stdout_handle {
                            let _ = h.await;
                        }
                        if let Some(h) = stderr_handle {
                            let _ = h.await;
                        }
                        return Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("timeout:{}", options.timeout.unwrap_or(0)),
                        )) as Box<dyn std::error::Error + Send + Sync>);
                    }

                    let remaining = ms.saturating_sub(elapsed);
                    let poll_ms = std::cmp::min(remaining, 100);

                    match tokio::time::timeout(
                        std::time::Duration::from_millis(poll_ms as u64),
                        child.wait(),
                    )
                    .await
                    {
                        Ok(Ok(status)) => break status.code(),
                        Ok(Err(_)) => break None,
                        Err(_) => continue,
                    }
                };
                result
            } else {
                // Without timeout
                loop {
                    if let Some(ref mut rx) = signal {
                        if *rx.borrow() {
                            let _ = child.kill().await;
                            if let Some(h) = stdout_handle {
                                let _ = h.await;
                            }
                            if let Some(h) = stderr_handle {
                                let _ = h.await;
                            }
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::Interrupted,
                                "aborted",
                            )) as Box<dyn std::error::Error + Send + Sync>);
                        }
                    }

                    match tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        child.wait(),
                    )
                    .await
                    {
                        Ok(Ok(status)) => break status.code(),
                        Ok(Err(_)) => break None,
                        Err(_) => continue,
                    }
                }
            };

            // Wait for stdout/stderr tasks to finish
            if let Some(h) = stdout_handle {
                let _ = h.await;
            }
            if let Some(h) = stderr_handle {
                let _ = h.await;
            }

            Ok(BashExecResult { exit_code })
        })
    }
}

// ============================================================================
// BashToolOptions
// ============================================================================

#[derive(Clone)]
pub struct BashToolOptions {
    pub operations: Arc<dyn BashOperations>,
    pub command_prefix: Option<String>,
    pub shell_path: Option<String>,
    pub spawn_hook: Option<BashSpawnHook>,
}

impl fmt::Debug for BashToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BashToolOptions")
            .field("command_prefix", &self.command_prefix)
            .field("shell_path", &self.shell_path)
            .field("spawn_hook", &self.spawn_hook.as_ref().map(|_| "BashSpawnHook"))
            .finish()
    }
}

impl Default for BashToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalBashOperations),
            command_prefix: None,
            shell_path: None,
            spawn_hook: None,
        }
    }
}

// ============================================================================
// Parameters schema
// ============================================================================

fn bash_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Bash command to execute"
            },
            "timeout": {
                "type": "number",
                "description": "Timeout in seconds (optional, no default timeout)"
            }
        },
        "required": ["command"]
    })
}

// ============================================================================
// create_bash_tool
// ============================================================================

/// Format output from a snapshot, matching the original TypeScript behavior.
fn format_output(
    snapshot: &crate::core::tools::output_accumulator::OutputSnapshot,
    last_line_bytes: usize,
    empty_text: &str,
) -> (String, Option<BashToolDetails>) {
    let truncation = &snapshot.truncation;
    let mut text = if snapshot.content.is_empty() {
        empty_text.to_string()
    } else {
        snapshot.content.clone()
    };

    let mut details: Option<BashToolDetails> = None;

    if truncation.truncated {
        let full_output_path = snapshot.full_output_path.as_deref().unwrap_or("");
        let start_line = truncation.total_lines.saturating_sub(truncation.output_lines) + 1;
        let end_line = truncation.total_lines;

        let notice = if truncation.last_line_partial {
            let last_line_size = format_size(last_line_bytes);
            format!(
                "\n\n[Showing last {} of line {} (line is {}). Full output: {}]",
                format_size(truncation.output_bytes),
                end_line,
                last_line_size,
                full_output_path
            )
        } else if truncation.truncated_by.as_deref() == Some("lines") {
            format!(
                "\n\n[Showing lines {}-{} of {}. Full output: {}]",
                start_line, end_line, truncation.total_lines, full_output_path
            )
        } else {
            format!(
                "\n\n[Showing lines {}-{} of {} ({} limit). Full output: {}]",
                start_line,
                end_line,
                truncation.total_lines,
                format_size(DEFAULT_MAX_BYTES),
                full_output_path
            )
        };

        text.push_str(&notice);
        details = Some(BashToolDetails {
            truncation: Some(truncation.clone()),
            full_output_path: snapshot.full_output_path.clone(),
        });
    }

    (text, details)
}

/// Append a status line to the output text.
fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{}\n\n{}", text, status)
    }
}

pub fn create_bash_tool(
    cwd: &str,
    options: Option<BashToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();
    let command_prefix = opts.command_prefix.clone();
    let spawn_hook = opts.spawn_hook.clone();

    AgentTool {
        name: "bash".to_string(),
        description: format!(
            "Execute a bash command on the local machine. Returns stdout and stderr. \
             Output is truncated to last {} lines or {}KB (whichever is hit first). \
             If truncated, full output is saved to a temp file. \
             Optionally provide a timeout in seconds.",
            DEFAULT_MAX_LINES,
            DEFAULT_MAX_BYTES / 1024
        ),
        label: "Bash".to_string(),
        parameters_schema: bash_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(
            move |_tool_call_id: String,
                  params: serde_json::Value,
                  signal: Option<tokio::sync::watch::Receiver<bool>>,
                  on_update: Option<
                Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>,
            >| {
                let cwd = cwd.clone();
                let operations = operations.clone();
                let command_prefix = command_prefix.clone();
                let spawn_hook = spawn_hook.clone();
                Box::pin(async move {
                    let command = params
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let timeout = params.get("timeout").and_then(|v| v.as_u64());

                    let resolved_command = if let Some(ref prefix) = command_prefix {
                        format!("{}\n{}", prefix, command)
                    } else {
                        command.clone()
                    };

                    // Resolve spawn context (applies spawn hook)
                    let spawn_ctx = resolve_spawn_context(&resolved_command, &cwd, spawn_hook.as_ref());

                    // Create OutputAccumulator for streaming output
                    let output = Arc::new(Mutex::new(OutputAccumulator::new(
                        OutputAccumulatorOptions {
                            temp_file_prefix: Some("pi-bash".to_string()),
                            ..Default::default()
                        },
                    )));

                    // Streaming update state
                    let update_dirty = Arc::new(AtomicBool::new(false));
                    let last_update_at = Arc::new(AtomicU64::new(0));

                    // Emit an initial empty update
                    if let Some(ref cb) = on_update {
                        cb(AgentToolResult {
                            content: vec![],
                            details: serde_json::Value::Null,
                            terminate: None,
                        });
                    }

                    // Create on_data callback that feeds the OutputAccumulator
                    let on_data_output = output.clone();
                    let on_data_dirty = update_dirty.clone();
                    let on_data_last_update = last_update_at.clone();
                    let on_data_cb = on_update.clone();

                    let on_data = {
                        let on_data_cb = on_data_cb.clone();
                        let on_data_output = on_data_output.clone();
                        let on_data_dirty = on_data_dirty.clone();
                        let on_data_last_update = on_data_last_update.clone();

                        Arc::new(move |data: &[u8]| {
                            // Append to accumulator. Scope the guard so it is
                            // released before the throttled snapshot re-locks
                            // below — std::sync::Mutex is not reentrant, so
                            // holding it across the second lock() would
                            // deadlock the stdout/stderr reader tasks and hang
                            // the bash tool forever.
                            {
                                let mut acc = on_data_output.lock().unwrap();
                                acc.append(data);
                            }

                            // Schedule throttled update
                            if on_data_cb.is_some() {
                                on_data_dirty.store(true, Ordering::SeqCst);
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                let last = on_data_last_update.load(Ordering::SeqCst);
                                if now.saturating_sub(last) >= BASH_UPDATE_THROTTLE_MS {
                                    on_data_last_update.store(now, Ordering::SeqCst);
                                    on_data_dirty.store(false, Ordering::SeqCst);
                                    let snapshot = {
                                        let acc = on_data_output.lock().unwrap();
                                        acc.snapshot(true)
                                    };
                                    if let Some(ref cb) = on_data_cb {
                                        let text = snapshot.content.clone();
                                        let trunc = snapshot.truncation.clone();
                                        let details = if trunc.truncated {
                                            Some(BashToolDetails {
                                                truncation: Some(trunc),
                                                full_output_path: snapshot.full_output_path.clone(),
                                            })
                                        } else {
                                            None
                                        };
                                        cb(AgentToolResult {
                                            content: vec![ContentBlock::text(text)],
                                            details: serde_json::to_value(details)
                                                .unwrap_or(serde_json::Value::Null),
                                            terminate: None,
                                        });
                                    }
                                }
                            }
                        }) as Arc<dyn Fn(&[u8]) + Send + Sync>
                    };

                    // Execute the command
                    let result = operations
                        .exec(
                            &spawn_ctx.command,
                            &spawn_ctx.cwd,
                            BashExecOptions {
                                on_data: Some(on_data),
                                signal,
                                timeout,
                                env: Some(spawn_ctx.env),
                            },
                        )
                        .await;

                    // Finish output accumulation
                    {
                        let mut acc = output.lock().unwrap();
                        acc.finish();
                    }

                    // Emit final update
                    let snapshot = {
                        let acc = output.lock().unwrap();
                        acc.snapshot(true)
                    };
                    let last_line_bytes = {
                        let acc = output.lock().unwrap();
                        acc.get_last_line_bytes()
                    };

                    match result {
                        Ok(exec_result) => {
                            let (output_text, details) = format_output(&snapshot, last_line_bytes, "(no output)");

                            let final_text = if let Some(code) = exec_result.exit_code {
                                if code != 0 {
                                    append_status(&output_text, &format!("Command exited with code {}", code))
                                } else {
                                    output_text
                                }
                            } else {
                                output_text
                            };

                            // If exit code is non-zero, treat as error
                            if let Some(code) = exec_result.exit_code {
                                if code != 0 {
                                    // Emit final error update
                                    if let Some(ref cb) = on_update {
                                        cb(AgentToolResult {
                                            content: vec![ContentBlock::text(&final_text)],
                                            details: serde_json::to_value(&details)
                                                .unwrap_or(serde_json::Value::Null),
                                            terminate: None,
                                        });
                                    }
                                    return Err(Box::new(std::io::Error::new(
                                        std::io::ErrorKind::Other,
                                        format!("Command failed with exit code {}", code),
                                    )) as Box<dyn std::error::Error + Send + Sync>);
                                }
                            }

                            Ok(AgentToolResult {
                                content: vec![ContentBlock::text(final_text)],
                                details: serde_json::to_value(details)
                                    .unwrap_or(serde_json::Value::Null),
                                terminate: None,
                            })
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            let (output_text, _) = format_output(&snapshot, last_line_bytes, "");

                            let final_text = if err_msg == "aborted" {
                                append_status(&output_text, "Command aborted")
                            } else if err_msg.starts_with("timeout:") {
                                let timeout_secs = err_msg.split(':').nth(1).unwrap_or("?");
                                append_status(
                                    &output_text,
                                    &format!("Command timed out after {} seconds", timeout_secs),
                                )
                            } else {
                                // Pass through other errors
                                err_msg
                            };

                            // Emit final error update
                            if let Some(ref cb) = on_update {
                                cb(AgentToolResult {
                                    content: vec![ContentBlock::text(&final_text)],
                                    details: serde_json::to_value(BashToolDetails::default())
                                        .unwrap_or(serde_json::Value::Null),
                                    terminate: None,
                                });
                            }

                            Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                final_text,
                            )) as Box<dyn std::error::Error + Send + Sync>)
                        }
                    }
                })
            },
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_timeout_ms_none() {
        assert_eq!(resolve_timeout_ms(None).unwrap(), None);
    }

    #[test]
    fn test_resolve_timeout_ms_valid() {
        assert_eq!(resolve_timeout_ms(Some(5)).unwrap(), Some(5000));
    }

    #[test]
    fn test_resolve_timeout_ms_zero() {
        assert!(resolve_timeout_ms(Some(0)).is_err());
    }

    #[test]
    fn test_resolve_timeout_ms_too_large() {
        assert!(resolve_timeout_ms(Some(MAX_TIMEOUT_SECONDS + 1)).is_err());
    }

    #[test]
    fn test_resolve_timeout_ms_max() {
        let result = resolve_timeout_ms(Some(MAX_TIMEOUT_SECONDS)).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), MAX_TIMEOUT_SECONDS * 1000);
    }

    #[test]
    fn test_append_status_empty() {
        assert_eq!(append_status("", "error"), "error");
    }

    #[test]
    fn test_append_status_non_empty() {
        assert_eq!(
            append_status("some output", "error"),
            "some output\n\nerror"
        );
    }

    #[test]
    fn test_get_shell_config_default_unix() {
        // On non-Windows, default should be bash with stdin transport
        if !cfg!(target_os = "windows") {
            let config = get_shell_config(None);
            assert_eq!(config.shell, "bash");
        }
    }

    #[test]
    fn test_get_shell_config_with_path() {
        let config = get_shell_config(Some("/bin/zsh"));
        assert_eq!(config.shell, "/bin/zsh");
    }

    #[test]
    fn test_resolve_spawn_context_no_hook() {
        let ctx = resolve_spawn_context("echo hello", "/tmp", None);
        assert_eq!(ctx.command, "echo hello");
        assert_eq!(ctx.cwd, "/tmp");
        assert!(!ctx.env.is_empty());
    }

    #[test]
    fn test_resolve_spawn_context_with_hook() {
        let hook: BashSpawnHook = Arc::new(|mut ctx| {
            ctx.command = format!("echo 'wrapped: {}'", ctx.command);
            ctx
        });
        let ctx = resolve_spawn_context("hello", "/tmp", Some(&hook));
        assert_eq!(ctx.command, "echo 'wrapped: hello'");
    }

    #[test]
    fn test_format_output_no_truncation() {
        let snapshot = crate::core::tools::output_accumulator::OutputSnapshot {
            content: "hello\nworld".to_string(),
            truncation: TruncationResult {
                content: "hello\nworld".to_string(),
                truncated: false,
                truncated_by: None,
                total_lines: 2,
                total_bytes: 11,
                output_lines: 2,
                output_bytes: 11,
                last_line_partial: false,
                first_line_exceeds_limit: false,
                max_lines: DEFAULT_MAX_LINES,
                max_bytes: DEFAULT_MAX_BYTES,
            },
            full_output_path: None,
        };
        let (text, details) = format_output(&snapshot, 0, "(no output)");
        assert_eq!(text, "hello\nworld");
        assert!(details.is_none());
    }

    #[test]
    fn test_format_output_empty() {
        let snapshot = crate::core::tools::output_accumulator::OutputSnapshot {
            content: String::new(),
            truncation: TruncationResult {
                content: String::new(),
                truncated: false,
                truncated_by: None,
                total_lines: 0,
                total_bytes: 0,
                output_lines: 0,
                output_bytes: 0,
                last_line_partial: false,
                first_line_exceeds_limit: false,
                max_lines: DEFAULT_MAX_LINES,
                max_bytes: DEFAULT_MAX_BYTES,
            },
            full_output_path: None,
        };
        let (text, details) = format_output(&snapshot, 0, "(no output)");
        assert_eq!(text, "(no output)");
        assert!(details.is_none());
    }
}
