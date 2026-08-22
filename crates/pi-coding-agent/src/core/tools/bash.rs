use std::fmt;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::core::tools::output_accumulator::{OutputAccumulator, OutputAccumulatorOptions};
use crate::utils::shell::{
    kill_process_tree, track_detached_child_pid, untrack_detached_child_pid,
};
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
    pub timeout: Option<f64>,
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
/// Callback invoked with raw bytes as they arrive from stdout/stderr.
type DataCallback = Arc<dyn Fn(&[u8]) + Send + Sync>;

pub struct BashExecOptions {
    /// Callback invoked with raw bytes as they arrive from stdout/stderr.
    pub on_data: Option<DataCallback>,
    /// Signal receiver for cancellation.
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
    /// Timeout in seconds (optional; fractional seconds supported, matching
    /// TS where the timeout is a plain `number`).
    pub timeout: Option<f64>,
    /// Environment variables.
    pub env: Option<Vec<(String, String)>>,
    /// Explicit shell path (matching TS `createLocalBashOperations`
    /// `options.shellPath` — resolved via `getShellConfig`).
    pub shell_path: Option<String>,
}

impl fmt::Debug for BashExecOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BashExecOptions")
            .field("timeout", &self.timeout)
            .field("signal", &self.signal.as_ref().map(|_| "Receiver"))
            .field("on_data", &self.on_data.as_ref().map(|_| "Fn"))
            .field("env", &self.env)
            .field("shell_path", &self.shell_path)
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
    ) -> crate::core::tools::AsyncOpResult<BashExecResult>;
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

/// Resolve timeout in milliseconds, validating constraints. Matches TS
/// `resolveTimeoutMs`: non-finite or `<= 0` values error; fractional seconds
/// are supported (`timeout * 1000`).
fn resolve_timeout_ms(timeout: Option<f64>) -> Result<Option<u64>, String> {
    match timeout {
        None => Ok(None),
        Some(t) => {
            if !t.is_finite() || t <= 0.0 {
                // Match TS error wording exactly (`resolveTimeoutMs`):
                // "Invalid timeout: must be a finite number of seconds".
                return Err("Invalid timeout: must be a finite number of seconds".to_string());
            }
            let timeout_ms = (t * 1000.0) as u64;
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
/// Resolve the shell configuration, matching TS `getShellConfig`
/// (`utils/shell.ts`). Resolution order:
/// 1. User-specified shellPath (must exist, else `Custom shell path not found`);
///    non-WSL bash gets `-c` (args transport), legacy WSL bash gets `-s`
///    (stdin transport) — matching TS `getBashShellConfig`.
/// 2. Windows: Git Bash in known locations, then bash on PATH, else a
///    descriptive error (TS has no cmd.exe fallback).
/// 3. Unix: /bin/bash, then bash on PATH, then fallback `sh -c`.
fn get_shell_config(shell_path: Option<&str>) -> Result<ShellConfig, String> {
    if let Some(path) = shell_path {
        if std::path::Path::new(path).exists() {
            return Ok(shell_config_for_bash(path));
        }
        return Err(format!("Custom shell path not found: {}", path));
    }

    if cfg!(target_os = "windows") {
        // Try Git Bash in known locations (matching TS).
        let mut candidates = Vec::new();
        if let Ok(pf) = std::env::var("ProgramFiles") {
            candidates.push(format!("{}\\Git\\bin\\bash.exe", pf));
        }
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(format!("{}\\Git\\bin\\bash.exe", pf86));
        }
        for c in candidates {
            if std::path::Path::new(&c).exists() {
                return Ok(shell_config_for_bash(&c));
            }
        }
        // Fallback: search bash.exe on PATH (Cygwin, MSYS2, WSL, etc.).
        if let Some(bash_on_path) = find_bash_on_path() {
            return Ok(shell_config_for_bash(&bash_on_path));
        }
        let searched: Vec<String> = ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .map(|k| {
                std::env::var(k)
                    .map(|v| format!("  {}\\Git\\bin\\bash.exe", v))
                    .unwrap_or_default()
            })
            .collect();
        return Err(format!(
            "No bash shell found. Options:\n  1. Install Git for Windows: https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n{}",
            searched.join("\n")
        ));
    }

    // Unix: try /bin/bash, then bash on PATH, then fallback to sh (matching TS).
    if std::path::Path::new("/bin/bash").exists() {
        return Ok(shell_config_for_bash("/bin/bash"));
    }
    if let Some(bash_on_path) = find_bash_on_path() {
        return Ok(shell_config_for_bash(&bash_on_path));
    }
    Ok(ShellConfig {
        shell: "sh".to_string(),
        args: vec!["-c".to_string()],
        command_transport: CommandTransport::Args,
    })
}

/// Matching TS `getBashShellConfig`: legacy WSL bash paths use `-s` (stdin
/// transport); everything else uses `-c` (args transport).
fn shell_config_for_bash(shell: &str) -> ShellConfig {
    if is_legacy_wsl_bash_path(shell) {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-s".to_string()],
            command_transport: CommandTransport::Stdin,
        }
    } else {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-c".to_string()],
            command_transport: CommandTransport::Args,
        }
    }
}

/// Matching TS `isLegacyWslBashPath`:
/// `/^[a-z]:\windows\(?:system32|sysnative)\bash\.exe$/` (case-insensitive).
fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.to_lowercase().replace('\\', "/");
    normalized.starts_with("c:/windows/system32/bash.exe")
        || normalized.starts_with("c:/windows/sysnative/bash.exe")
}

/// Find bash on PATH — matching TS `findBashOnPath`: Windows uses `where`,
/// Unix uses `which`; the first existing result wins.
fn find_bash_on_path() -> Option<String> {
    if cfg!(target_os = "windows") {
        // `where bash.exe` with a 5s timeout + hidden window (matching TS
        // `findBashOnPath`: where can return non-existent paths, so verify
        // the first match actually exists).
        let first = crate::utils::shell::run_path_probe("where", "bash.exe")?;
        if std::path::Path::new(&first).exists() {
            return Some(first);
        }
        None
    } else {
        let first = crate::utils::shell::run_path_probe("which", "bash")?;
        if !first.is_empty() {
            return Some(first);
        }
        None
    }
}

/// Get the current process environment as a vector of key-value pairs, with
/// the pi `bin` directory prepended to PATH (matching TS `getShellEnv`).
fn get_shell_env() -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = std::env::vars().collect();

    let bin_dir = crate::config::get_agent_dir().join("bin");
    let path_key = env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("path"))
        .map(|(k, _)| k.clone())
        .unwrap_or_else(|| "PATH".to_string());

    let current = env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("path"))
        .map(|(_, v)| v.clone());

    let updated = match current {
        Some(cur) => {
            let mut entries: Vec<std::path::PathBuf> = std::env::split_paths(&cur).collect();
            if !entries.contains(&bin_dir) {
                entries.insert(0, bin_dir.clone());
            }
            std::env::join_paths(entries)
                .map(|os| os.to_string_lossy().to_string())
                .unwrap_or(cur)
        }
        None => bin_dir.to_string_lossy().to_string(),
    };

    if let Some(entry) = env.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case("path")) {
        entry.1 = updated;
    } else {
        env.push((path_key, updated));
    }
    env
}

/// PI_* variables that are never inherited from the parent process; they are
/// re-injected from the current session when available (match TS #6967).
const PI_SESSION_ENV_VARS: &[&str] = &[
    "PI_SESSION_ID",
    "PI_SESSION_FILE",
    "PI_PROVIDER",
    "PI_MODEL",
    "PI_REASONING_LEVEL",
];

/// Resolve the spawn context, applying the spawn hook if provided.
/// Session metadata is injected as `PI_*` env vars (match TS #6967).
fn resolve_spawn_context(
    command: &str,
    cwd: &str,
    spawn_hook: Option<&BashSpawnHook>,
    session_env: Option<&BashSessionEnv>,
    expose_session_env: bool,
) -> BashSpawnContext {
    let mut env = get_shell_env();
    // Never inherit PI_* from the parent process.
    env.retain(|(k, _)| !PI_SESSION_ENV_VARS.contains(&k.as_str()));
    // Re-inject the current session's PI_* vars only when exposure is enabled
    // (matching TS `resolveSpawnContext` `if (exposeSessionEnvironment && ctx)`).
    if expose_session_env {
        if let Some(session) = session_env {
            if let Some(id) = &session.session_id {
                env.push(("PI_SESSION_ID".to_string(), id.clone()));
            }
            if let Some(file) = &session.session_file {
                env.push(("PI_SESSION_FILE".to_string(), file.clone()));
            }
            if let Some(provider) = &session.provider {
                env.push(("PI_PROVIDER".to_string(), provider.clone()));
            }
            if let Some(model) = &session.model {
                env.push(("PI_MODEL".to_string(), model.clone()));
            }
            if let Some(level) = &session.thinking_level {
                env.push(("PI_REASONING_LEVEL".to_string(), level.clone()));
            }
        }
    }
    let base = BashSpawnContext {
        command: command.to_string(),
        cwd: cwd.to_string(),
        env,
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
    ) -> crate::core::tools::AsyncOpResult<BashExecResult> {
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
        let signal = options.signal;

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

            let shell_config = match get_shell_config(options.shell_path.as_deref()) {
                Ok(config) => config,
                Err(e) => {
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        e,
                    )) as Box<dyn std::error::Error + Send + Sync>);
                }
            };
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
            // (matching TS `killProcessTree` which sends SIGKILL to -pid).
            #[cfg(unix)]
            {
                cmd.process_group(0);
            }

            let mut child = cmd
                .spawn()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

            // Track the pid for cleanup on shutdown, matching TS
            // `trackDetachedChildPid(child.pid)` right after spawn. The guard
            // untracks on every exit path (matching TS `finally { untrack... }`).
            struct UntrackGuard(u32);
            impl Drop for UntrackGuard {
                fn drop(&mut self) {
                    untrack_detached_child_pid(self.0);
                }
            }
            if let Some(pid) = child.id() {
                track_detached_child_pid(pid);
            }
            let _untrack_guard = child.id().map(UntrackGuard);
            // Recorded pid used to kill the whole process group on abort/timeout
            // (child is moved into the wait task below, so we keep it here).
            let pid = child.id();

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

            // Take ownership of stdout/stderr. They are read directly in the
            // main loop below (matching TS `waitForChildProcess`, which watches
            // exit + pipe activity) instead of spawning detached reader tasks
            // that can never finish while a descendant holds the pipe open.
            let mut stdout = child.stdout.take();
            let mut stderr = child.stderr.take();
            let on_data = options.on_data.clone();

            // Wait for the process to exit on a background task so stdout and
            // stderr can be read concurrently in the main loop.
            let mut wait_task = tokio::spawn(async move { child.wait().await });

            let mut stdout_buf = vec![0u8; 4096];
            let mut stderr_buf = vec![0u8; 4096];
            let mut stdout_eof = stdout.is_none();
            let mut stderr_eof = stderr.is_none();
            // None = shell has not exited yet; Some(code) once child.wait() returns.
            let mut exited: Option<Option<i32>> = None;
            // Last moment output arrived / shell exited; used for the post-exit
            // stdio idle grace (matching TS `EXIT_STDIO_GRACE_MS = 100`).
            let mut last_activity = std::time::Instant::now();
            let mut exited_at: Option<std::time::Instant> = None;
            let mut aborted = false;
            let mut timed_out = false;
            let start = std::time::Instant::now();

            use tokio::io::AsyncReadExt;

            // ── Main loop: wait for exit, stream output, handle abort/timeout ──
            // Matching TS `createLocalBashOperations` + `waitForChildProcess`:
            // - abort (polled here; TS is event-driven) and timeout both call
            //   `killProcessTree` and then keep waiting for the process to exit
            //   and the pipes to fall idle.
            // - after the shell exits we wait for the stdio pipes: if both end
            //   we finish immediately; if a detached descendant keeps a pipe
            //   open we release once it has been idle for EXIT_STDIO_GRACE_MS
            //   (re-armed on every chunk), so we never hang on inherited handles
            //   and never drop output that is still being written.
            const EXIT_STDIO_GRACE_MS: u64 = 100;
            loop {
                // Cancellation (matching TS onAbort; kill happens once).
                if let Some(ref rx) = signal {
                    if !aborted && *rx.borrow() {
                        aborted = true;
                        if let Some(pid) = pid {
                            kill_process_tree(pid);
                        }
                    }
                }
                // Timeout (matching TS setTimeout → killProcessTree).
                if !timed_out {
                    if let Some(ms) = timeout_ms {
                        if start.elapsed().as_millis() as u64 >= ms {
                            timed_out = true;
                            if let Some(pid) = pid {
                                kill_process_tree(pid);
                            }
                        }
                    }
                }

                tokio::select! {
                    // Shell exit (matching TS 'exit' event).
                    res = &mut wait_task, if exited.is_none() => {
                        let code: Option<i32> = match res {
                            Ok(Ok(status)) => status.code(),
                            _ => None,
                        };
                        exited = Some(code);
                        exited_at = Some(std::time::Instant::now());
                    }
                    // stdout data / EOF.
                    r = async {
                        match stdout.as_mut() {
                            Some(s) => match s.read(&mut stdout_buf).await {
                                Ok(0) => Ok(None),
                                Ok(n) => Ok(Some(n)),
                                Err(e) => Err(e),
                            },
                            None => Ok(None),
                        }
                    }, if !stdout_eof => {
                        match r {
                            Ok(None) => stdout_eof = true,
                            Ok(Some(n)) => {
                                if let Some(ref cb) = on_data {
                                    cb(&stdout_buf[..n]);
                                }
                                last_activity = std::time::Instant::now();
                            }
                            Err(_) => stdout_eof = true,
                        }
                    }
                    // stderr data / EOF.
                    r = async {
                        match stderr.as_mut() {
                            Some(s) => match s.read(&mut stderr_buf).await {
                                Ok(0) => Ok(None),
                                Ok(n) => Ok(Some(n)),
                                Err(e) => Err(e),
                            },
                            None => Ok(None),
                        }
                    }, if !stderr_eof => {
                        match r {
                            Ok(None) => stderr_eof = true,
                            Ok(Some(n)) => {
                                if let Some(ref cb) = on_data {
                                    cb(&stderr_buf[..n]);
                                }
                                last_activity = std::time::Instant::now();
                            }
                            Err(_) => stderr_eof = true,
                        }
                    }
                    // Poll loop heartbeat: re-check abort/timeout and idle grace.
                    _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
                }

                // Termination check (matching TS waitForChildProcess):
                // - shell exited and both pipes ended → done;
                // - shell exited and pipes idle for EXIT_STDIO_GRACE_MS → done
                //   (a quiet inherited handle held by a detached descendant
                //    still releases us after the grace elapses).
                if exited.is_some() {
                    let pipes_done = stdout_eof && stderr_eof;
                    let grace_elapsed = exited_at
                        .map(|exit_time| {
                            let last = std::cmp::max(exit_time, last_activity);
                            last.elapsed().as_millis() as u64 >= EXIT_STDIO_GRACE_MS
                        })
                        .unwrap_or(false);
                    if pipes_done || grace_elapsed {
                        break;
                    }
                }
            }

            // Matching TS: after waitForChildProcess resolves, an abort signal
            // (even one arriving right at exit) surfaces as "aborted".
            if let Some(ref rx) = signal {
                if *rx.borrow() {
                    aborted = true;
                }
            }

            // stdout/stderr readers above are dropped here (pipe ends closed),
            // so no further output can reach the accumulator — matching TS
            // `waitForChildProcess` finalize which destroys the streams.
            if aborted {
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "aborted",
                )) as Box<dyn std::error::Error + Send + Sync>)
            } else if timed_out {
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("timeout:{}", options.timeout.unwrap_or(0.0)),
                )) as Box<dyn std::error::Error + Send + Sync>)
            } else {
                Ok(BashExecResult {
                    exit_code: exited.flatten(),
                })
            }
        })
    }
}

// ============================================================================
// BashToolOptions
// ============================================================================

/// Current session metadata exposed to bash commands as `PI_*` environment
/// variables (match TS #6967).
#[derive(Debug, Clone, Default)]
pub struct BashSessionEnv {
    pub session_id: Option<String>,
    pub session_file: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
}

/// Async provider for [`BashSessionEnv`], resolved at each command start so
/// model/thinking-level changes are picked up (match TS #6967).
pub type BashSessionEnvProvider = Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = BashSessionEnv> + Send>,
        > + Send
        + Sync,
>;

#[derive(Clone)]
pub struct BashToolOptions {
    pub operations: Arc<dyn BashOperations>,
    pub command_prefix: Option<String>,
    pub shell_path: Option<String>,
    pub spawn_hook: Option<BashSpawnHook>,
    /// Current session metadata injected as `PI_*` env vars, resolved at each
    /// command start (match TS #6967).
    pub session_env_provider: Option<BashSessionEnvProvider>,
    /// Expose the current session to the command as `PI_*` environment
    /// variables, and include the session-environment prompt guideline
    /// (matching TS `createBashToolDefinition` `exposeSessionEnvironment`,
    /// default `true`).
    pub expose_session_environment: bool,
    /// Timeout in seconds applied when the model does not pass an explicit
    /// `timeout` argument. `None` keeps the TS-original behavior (no default
    /// timeout). ACP mode sets this so a hung command (e.g. a stalled
    /// `git clone`) is killed and the turn settles instead of blocking the
    /// session forever.
    pub default_timeout: Option<f64>,
}

impl fmt::Debug for BashToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BashToolOptions")
            .field("command_prefix", &self.command_prefix)
            .field("shell_path", &self.shell_path)
            .field("spawn_hook", &self.spawn_hook.as_ref().map(|_| "BashSpawnHook"))
            .field(
                "session_env_provider",
                &self.session_env_provider.as_ref().map(|_| "BashSessionEnvProvider"),
            )
            .field("expose_session_environment", &self.expose_session_environment)
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
            session_env_provider: None,
            expose_session_environment: true,
            default_timeout: None,
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
    let session_env_provider = opts.session_env_provider.clone();
    let expose_session_environment = opts.expose_session_environment;
    let shell_path = opts.shell_path.clone();

    AgentTool {
        name: "bash".to_string(),
        description: format!(
            "Execute a bash command in the current working directory. Returns stdout and stderr. \
             Output is truncated to last {} lines or {}KB (whichever is hit first). \
             If truncated, full output is saved to a temp file. \
             Optionally provide a timeout in seconds.",
            DEFAULT_MAX_LINES,
            DEFAULT_MAX_BYTES / 1024
        ),
        label: "bash".to_string(),
        prompt_snippet: Some(
            "Execute bash commands (ls, grep, find, etc.)".to_string(),
        ),
        prompt_guidelines: if expose_session_environment {
            Some(vec![
                "You can inspect PI_* environment variables for current model and session details."
                    .to_string(),
            ])
        } else {
            None
        },
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
                let session_env_provider = session_env_provider.clone();
                let shell_path = shell_path.clone();
                Box::pin(async move {
                    let command = params
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    // Model-provided timeout wins; otherwise fall back to the
                    // configured default (ACP mode injects one so hung
                    // commands cannot block the session forever).
                    let timeout = params
                        .get("timeout")
                        .and_then(|v| v.as_f64())
                        .or(opts.default_timeout);

                    let resolved_command = if let Some(ref prefix) = command_prefix {
                        format!("{}\n{}", prefix, command)
                    } else {
                        command.clone()
                    };

                    // Resolve spawn context (applies spawn hook). Session metadata
                    // is resolved at command start (match TS #6967).
                    let session_env = match &session_env_provider {
                        Some(provider) => provider().await,
                        None => BashSessionEnv::default(),
                    };
                    let spawn_ctx = resolve_spawn_context(
                        &resolved_command,
                        &cwd,
                        spawn_hook.as_ref(),
                        Some(&session_env),
                        expose_session_environment,
                    );

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
                            usage: None,
                            added_tool_names: None,

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
                                let mut acc = on_data_output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
                                        let acc = on_data_output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
                                            usage: None,
                                            added_tool_names: None,

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
                                shell_path,
                            },
                        )
                        .await;

                    // Finish output accumulation
                    {
                        let mut acc = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        acc.finish();
                    }

                    // Final snapshot (matching TS `finishOutput`).
                    let snapshot = {
                        let acc = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        acc.snapshot(true)
                    };
                    let last_line_bytes = {
                        let acc = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        acc.get_last_line_bytes()
                    };
                    let final_details: Option<BashToolDetails> = if snapshot.truncation.truncated {
                        Some(BashToolDetails {
                            truncation: Some(snapshot.truncation.clone()),
                            full_output_path: snapshot.full_output_path.clone(),
                        })
                    } else {
                        None
                    };

                    // Matching TS `finishOutput` → `emitOutputUpdate`: flush any
                    // output that arrived since the last throttled update so the
                    // client streams the complete text (the ACP translator's
                    // delta logic filters empty updates downstream). Without this,
                    // a fast command that never hit the 100ms throttle would
                    // deliver its output only at `tool_execution_end` — and a
                    // successful command never emits a final update at all.
                    if update_dirty.load(Ordering::SeqCst) {
                        if let Some(ref cb) = on_update {
                            cb(AgentToolResult {
                                content: vec![ContentBlock::text(snapshot.content.clone())],
                                details: serde_json::to_value(&final_details)
                                    .unwrap_or(serde_json::Value::Null),
                                usage: None,
                                added_tool_names: None,

                                terminate: None,
                            });
                        }
                        update_dirty.store(false, Ordering::SeqCst);
                    }

                    match result {
                        Ok(exec_result) => {
                            let (output_text, details) = format_output(&snapshot, last_line_bytes, "(no output)");

                            // If exit code is non-zero, treat as error. Matching
                            // TS: the thrown error carries the FULL output + status
                            // (`appendStatus(outputText, "Command exited with code
                            // N")`) so the output lands in the tool result message
                            // and the ACP terminal — not a bare status line.
                            if let Some(code) = exec_result.exit_code {
                                if code != 0 {
                                    return Err(Box::new(std::io::Error::other(
                                        append_status(&output_text, &format!("Command exited with code {}", code)),
                                    )) as Box<dyn std::error::Error + Send + Sync>);
                                }
                            }

                            Ok(AgentToolResult {
                                content: vec![ContentBlock::text(output_text)],
                                details: serde_json::to_value(details)
                                    .unwrap_or(serde_json::Value::Null),
                                usage: None,
                                added_tool_names: None,

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

                            Err(Box::new(std::io::Error::other(
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_resolve_timeout_ms_none() {
        assert_eq!(resolve_timeout_ms(None).unwrap(), None);
    }

    #[test]
    fn test_resolve_timeout_ms_valid() {
        assert_eq!(resolve_timeout_ms(Some(5.0)).unwrap(), Some(5000));
    }

    #[test]
    fn test_resolve_timeout_ms_fractional() {
        // Fractional seconds are supported, matching TS (number timeout).
        assert_eq!(resolve_timeout_ms(Some(0.5)).unwrap(), Some(500));
        assert_eq!(resolve_timeout_ms(Some(1.25)).unwrap(), Some(1250));
    }

    #[test]
    fn test_resolve_timeout_ms_zero() {
        assert!(resolve_timeout_ms(Some(0.0)).is_err());
    }

    #[test]
    fn test_resolve_timeout_ms_too_large() {
        assert!(resolve_timeout_ms(Some((MAX_TIMEOUT_SECONDS + 1) as f64)).is_err());
    }

    #[test]
    fn test_resolve_timeout_ms_max() {
        let result = resolve_timeout_ms(Some(MAX_TIMEOUT_SECONDS as f64)).unwrap();
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
        // On non-Windows, default should be /bin/bash with `-c` (args
        // transport), matching TS getShellConfig resolution order.
        if !cfg!(target_os = "windows") {
            let config = get_shell_config(None).unwrap();
            assert_eq!(config.shell, "/bin/bash");
            assert_eq!(config.args, vec!["-c".to_string()]);
            assert!(matches!(config.command_transport, CommandTransport::Args));
        }
    }

    #[test]
    fn test_get_shell_config_with_path() {
        // An existing bash path resolves to `-c` (args transport), matching
        // TS getBashShellConfig for non-WSL shells.
        let config = get_shell_config(Some("/bin/zsh")).unwrap();
        assert_eq!(config.shell, "/bin/zsh");
        assert_eq!(config.args, vec!["-c".to_string()]);
        assert!(matches!(config.command_transport, CommandTransport::Args));
    }

    #[test]
    fn test_get_shell_config_missing_path_errors() {
        // A non-existent custom shell path errors with the TS wording.
        let missing = std::env::temp_dir().join("pi-definitely-missing-shell");
        match get_shell_config(Some(missing.to_str().unwrap())) {
            Err(err) => assert!(
                err.starts_with("Custom shell path not found:"),
                "unexpected error: {err}"
            ),
            Ok(_) => panic!("missing shell path must error"),
        }
    }

    #[test]
    fn test_get_shell_env_prepends_bin_dir_to_path() {
        // Matching TS getShellEnv: the pi bin dir is prepended to PATH.
        let env = get_shell_env();
        let path_val = env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("path"))
            .expect("PATH present")
            .1
            .clone();
        let bin_dir = crate::config::get_agent_dir().join("bin");
        let entries: Vec<std::path::PathBuf> = std::env::split_paths(&path_val).collect();
        assert!(
            !entries.is_empty(),
            "PATH must not be empty"
        );
        // The bin dir must be present at the front (or the only entry).
        assert_eq!(
            entries[0], bin_dir,
            "pi bin dir must be prepended to PATH (got {path_val})"
        );
    }

    #[test]
    fn test_resolve_spawn_context_no_hook() {
        let ctx = resolve_spawn_context("echo hello", "/tmp", None, None, true);
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
        let ctx = resolve_spawn_context("hello", "/tmp", Some(&hook), None, true);
        assert_eq!(ctx.command, "echo 'wrapped: hello'");
    }

    #[test]
    fn test_resolve_spawn_context_injects_session_env() {
        let session = BashSessionEnv {
            session_id: Some("sess-1".to_string()),
            session_file: Some("/tmp/sess.jsonl".to_string()),
            provider: Some("anthropic".to_string()),
            model: Some("claude-sonnet-4-6".to_string()),
            thinking_level: Some("high".to_string()),
        };
        let ctx = resolve_spawn_context("echo hi", "/tmp", None, Some(&session), true);
        let env: std::collections::HashMap<&str, &str> = ctx
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(env.get("PI_SESSION_ID"), Some(&"sess-1"));
        assert_eq!(env.get("PI_SESSION_FILE"), Some(&"/tmp/sess.jsonl"));
        assert_eq!(env.get("PI_PROVIDER"), Some(&"anthropic"));
        assert_eq!(env.get("PI_MODEL"), Some(&"claude-sonnet-4-6"));
        assert_eq!(env.get("PI_REASONING_LEVEL"), Some(&"high"));
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

    /// Tool definition must match TS `createBashToolDefinition`: name/label,
    /// description wording ("in the current working directory"), and the
    /// prompt snippet + guidelines (bashToolSystemPromptContribution).
    #[test]
    fn bash_tool_definition_matches_ts() {
        let tool = create_bash_tool("/tmp", None);
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.label, "bash");
        assert!(
            tool.description.contains("in the current working directory"),
            "description must match TS wording: {}",
            tool.description
        );
        assert_eq!(
            tool.prompt_snippet.as_deref(),
            Some("Execute bash commands (ls, grep, find, etc.)")
        );
        let guidelines = tool.prompt_guidelines.as_deref().expect("guidelines");
        assert_eq!(
            guidelines,
            &["You can inspect PI_* environment variables for current model and session details."]
        );
        // Parameters schema matches TS bashSchema.
        let params = tool.parameters_schema.as_object().expect("object");
        assert_eq!(params["required"], serde_json::json!(["command"]));
        assert!(params["properties"].get("command").is_some());
        assert!(params["properties"].get("timeout").is_some());
    }

    /// Aborting a running command must kill the whole process tree, not just
    /// the shell (matching TS `killProcessTree` which sends SIGKILL to the
    /// process group). Regression guard: `child.kill()` alone only kills the
    /// shell, leaving spawned children running as orphans.
    #[cfg(unix)]
#[tokio::test]
    async fn abort_kills_process_tree() {
        if cfg!(target_os = "windows") {
            return; // process groups behave differently on Windows
        }
        let ops = LocalBashOperations;
        // Serialize against other tests sharing the global TRACKED_PIDS set.
        let _guard = crate::utils::shell::TEST_TRACK_LOCK.lock().await;

        let (tx, rx) = tokio::sync::watch::channel(false);

        // Spawn a background child (`sleep`) that writes its PID to a file,
        // then wait on it — the child must die together with the shell.
        let pid_file = std::env::temp_dir().join(format!(
            "pi-bash-tree-test-{}.pid",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&pid_file);
        let cmd = format!(
            "sleep 61.5 & echo $! > {}; wait",
            pid_file.to_string_lossy()
        );

        let exec_fut = ops.exec(&cmd, "/tmp", BashExecOptions {
            on_data: None,
            signal: Some(rx),
            timeout: None,
            env: None,
            shell_path: None,
        });
        tokio::pin!(exec_fut);

        // Poll the exec future while the shell starts the child and writes
        // the pid file (a bare future is lazy — it only runs once polled).
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {}
            _ = &mut exec_fut => panic!("exec finished before child started"),
        }
        let pid_text = std::fs::read_to_string(&pid_file).expect("pid file");
        let child_pid: i32 = pid_text.trim().parse().expect("pid");
        assert!(
            process_alive(child_pid),
            "background child {child_pid} must be running before abort"
        );

        // Abort — the shell must be killed together with its child.
        tx.send(true).expect("send abort");
        let result = exec_fut.await;
        let err = result
            .expect_err("abort must fail the exec")
            .to_string();
        assert!(err.contains("aborted"), "unexpected error: {err}");

        // The background child must no longer be alive (killed with the group).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !process_alive(child_pid),
            "background child {child_pid} must be killed with the shell"
        );
        let _ = std::fs::remove_file(&pid_file);
    }

    /// Timeout must also kill the whole process tree and surface a
    /// `timeout:N` error (matching TS `createLocalBashOperations` timeout
    /// handling).
    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_process_tree() {
        if cfg!(target_os = "windows") {
            return;
        }
        let ops = LocalBashOperations;
        // Serialize against other tests sharing the global TRACKED_PIDS set.
        let _guard = crate::utils::shell::TEST_TRACK_LOCK.lock().await;

        let pid_file = std::env::temp_dir().join(format!(
            "pi-bash-timeout-test-{}.pid",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&pid_file);
        let cmd = format!(
            "sleep 61.5 & echo $! > {}; wait",
            pid_file.to_string_lossy()
        );

        let exec_fut = ops.exec(&cmd, "/tmp", BashExecOptions {
            on_data: None,
            signal: None,
            timeout: Some(1.0),
            env: None,
            shell_path: None,
        });
        tokio::pin!(exec_fut);

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => {}
            _ = &mut exec_fut => panic!("exec finished before child started"),
        }
        let pid_text = std::fs::read_to_string(&pid_file).expect("pid file");
        let child_pid: i32 = pid_text.trim().parse().expect("pid");
        assert!(process_alive(child_pid), "child must be running before timeout");

        let result = exec_fut.await;
        let err = result
            .expect_err("timeout must fail the exec")
            .to_string();
        assert!(err.contains("timeout:"), "unexpected error: {err}");

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !process_alive(child_pid),
            "background child {child_pid} must be killed by timeout"
        );
        let _ = std::fs::remove_file(&pid_file);
    }

    /// A detached descendant that keeps the inherited stdout pipe open (but
    /// quiet) must not hang the tool: after the shell exits, `exec` releases
    /// once the pipe has been idle for EXIT_STDIO_GRACE_MS (matching TS
    /// `waitForChildProcess` — see utils/child-process.js). This was a real
    /// hang: the old code awaited the stdout/stderr reader tasks to EOF, which
    /// never happens while a descendant holds the pipe open.
    #[cfg(unix)]
    #[tokio::test]
    async fn detached_descendant_holding_pipe_does_not_hang() {
        if cfg!(target_os = "windows") {
            return;
        }
        let ops = LocalBashOperations;
        // Serialize against other tests sharing the global TRACKED_PIDS set.
        let _guard = crate::utils::shell::TEST_TRACK_LOCK.lock().await;

        let pid_file = std::env::temp_dir().join(format!(
            "pi-bash-pipe-test-{}.pid",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&pid_file);
        // The shell exits immediately but the background `sleep` inherits the
        // stdout pipe and keeps it open without writing — the exact case TS
        // waitForChildProcess handles via its idle grace timer.
        let cmd = format!(
            "sleep 61.5 & echo $! > {}",
            pid_file.to_string_lossy()
        );

        let start = std::time::Instant::now();
        let result = ops
            .exec(
                &cmd,
                "/tmp",
                BashExecOptions {
                    on_data: None,
                    signal: None,
                    timeout: None,
                    env: None,
                    shell_path: None,
                },
            )
            .await;
        let elapsed = start.elapsed();

        let exec_result = result.expect("exec must complete (not hang)");
        assert_eq!(exec_result.exit_code, Some(0), "shell exited 0");

        // The idle grace (~100ms + poll slack) must release us well under a
        // second even though the pipe is still held open by the descendant.
        assert!(
            elapsed.as_millis() < 2000,
            "exec hung for {elapsed:?} with a descendant holding the pipe"
        );

        // Clean up the background sleep.
        if let Ok(pid_text) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid_text.trim().parse::<i32>() {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    /// The spawned shell PID must be tracked while the command runs and
    /// untracked once it finishes (matching TS trackDetachedChildPid /
    /// untrackDetachedChildPid in createLocalBashOperations). The cleanup on
    /// shutdown (killTrackedDetachedChildren) is only effective if exec
    /// actually registers the pid.
    #[tokio::test]
    async fn exec_tracks_and_untracks_pid() {
        let ops = LocalBashOperations;
        // Serialize against other tests sharing the global TRACKED_PIDS set.
        let _guard = crate::utils::shell::TEST_TRACK_LOCK.lock().await;

        // Do NOT call kill_tracked_detached_children() here: that would kill
        // pids tracked by other concurrently running tests. Instead assert on
        // the set delta caused by this exec.
        let before = crate::utils::shell::tracked_pids_snapshot();

        let exec_fut = ops.exec(
            "sleep 0.6",
            "/tmp",
            BashExecOptions {
                on_data: None,
                signal: None,
                timeout: None,
                env: None,
                shell_path: None,
            },
        );
        tokio::pin!(exec_fut);

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
            _ = &mut exec_fut => panic!("exec finished before tracking could be observed"),
        }
        let during = crate::utils::shell::tracked_pids_snapshot();
        let new_pids: Vec<u32> = during
            .iter()
            .copied()
            .filter(|p| !before.contains(p))
            .collect();
        assert!(
            !new_pids.is_empty(),
            "shell pid must be tracked while the command runs (before={before:?}, during={during:?})"
        );

        let result = exec_fut.await;
        assert!(result.is_ok(), "exec should succeed");
        // Give the untrack guard a moment to run; the pid(s) this exec added
        // must be gone from the set.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let after = crate::utils::shell::tracked_pids_snapshot();
        for p in &new_pids {
            assert!(
                !after.contains(p),
                "shell pid {p} must be untracked after the command finishes"
            );
        }
    }

    #[cfg(unix)]
    fn process_alive(pid: i32) -> bool {
        if pid <= 0 {
            return false;
        }
        // kill(pid, 0) returns 0 if the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}
