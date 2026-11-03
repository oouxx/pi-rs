//! Stdio transport: spawn a server and speak newline-delimited JSON-RPC.
//!
//! Port of `packages/mcp/src/transports/stdio.ts`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;

use crate::error::{McpError, Result};
use crate::protocol::jsonrpc::{parse_json_rpc_message, JsonRpcMessage};
use crate::transport::{
    CloseListener, ErrorListener, McpTransport, McpTransportSendOptions, MessageListener,
    Subscription, TransportEvents, DEFAULT_MAX_MESSAGE_BYTES,
};

const DEFAULT_MAX_STDERR_BYTES: usize = 64 * 1024;
const DEFAULT_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a server gets to exit after stdin closes before SIGTERM.
const STDIN_CLOSE_GRACE: Duration = Duration::from_millis(500);

/// Where the child's stderr goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioStderr {
    /// Capture the tail in the transport.
    Pipe,
    /// Let the process inherit the parent's stderr.
    Inherit,
}

/// Receives stderr chunks.
pub type StderrListener = Arc<dyn Fn(&str) + Send + Sync>;

/// Options for [`StdioTransport`].
#[derive(Clone)]
pub struct StdioTransportOptions {
    /// Executable name or path.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables.
    pub env: std::collections::BTreeMap<String, String>,
    /// Whether to inherit the parent environment. Default: true.
    pub inherit_env: bool,
    /// Stderr handling. Default: pipe.
    pub stderr: StdioStderr,
    /// Called with every stderr chunk.
    pub on_stderr: Option<StderrListener>,
    /// Maximum size of one JSON-RPC message.
    pub max_message_bytes: usize,
    /// Maximum stderr tail kept in memory.
    pub max_stderr_bytes: usize,
    /// Time to wait after SIGTERM before SIGKILL.
    pub close_timeout: Duration,
}

impl StdioTransportOptions {
    /// Options for a command with no arguments.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: std::collections::BTreeMap::new(),
            inherit_env: true,
            stderr: StdioStderr::Pipe,
            on_stderr: None,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
            close_timeout: DEFAULT_CLOSE_TIMEOUT,
        }
    }
}

impl std::fmt::Debug for StdioTransportOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransportOptions")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("inherit_env", &self.inherit_env)
            .field("stderr", &self.stderr)
            .finish()
    }
}

struct Inner {
    options: StdioTransportOptions,
    events: TransportEvents,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    stdout_buffer: Mutex<Vec<u8>>,
    stderr_buffer: StdMutex<VecDeque<u8>>,
    pid: StdMutex<Option<u32>>,
    started: AtomicBool,
    closed: AtomicBool,
}

/// A child-process MCP transport.
#[derive(Clone)]
pub struct StdioTransport {
    inner: Arc<Inner>,
}

impl StdioTransport {
    /// Create a transport. Call [`McpTransport::start`] to spawn the process.
    pub fn new(options: StdioTransportOptions) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Inner {
                options,
                events: TransportEvents::new(),
                child: Mutex::new(None),
                stdin: Mutex::new(None),
                stdout_buffer: Mutex::new(Vec::new()),
                stderr_buffer: StdMutex::new(VecDeque::new()),
                pid: StdMutex::new(None),
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Process id, once spawned.
    pub fn pid(&self) -> Option<u32> {
        *self.inner.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Captured stderr tail.
    pub fn stderr(&self) -> String {
        let buffer = self
            .inner
            .stderr_buffer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        String::from_utf8_lossy(&buffer.iter().copied().collect::<Vec<_>>()).to_string()
    }

    fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    async fn emit_stdout_chunk(&self, chunk: &[u8]) {
        // Buffer is bounded by the message limit: parsing happens while
        // holding the lock, so no task can interleave.
        let mut buffer = self.inner.stdout_buffer.lock().await;
        buffer.extend_from_slice(chunk);
        let max = self.inner.options.max_message_bytes;
        loop {
            let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') else {
                if buffer.len() > max {
                    buffer.clear();
                    self.inner.events.emit_error(&McpError::Transport(format!(
                        "MCP stdio message exceeds {max} bytes"
                    )));
                }
                return;
            };
            let line = buffer.drain(..=newline).collect::<Vec<_>>();
            let line = &line[..line.len().saturating_sub(1)];
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.len() > max {
                self.inner.events.emit_error(&McpError::Transport(format!(
                    "MCP stdio message exceeds {max} bytes"
                )));
                continue;
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(line) else {
                self.inner
                    .events
                    .emit_error(&McpError::Transport("MCP stdio message is not UTF-8".to_string()));
                continue;
            };
            match serde_json::from_str::<serde_json::Value>(text)
                .map_err(McpError::from)
                .and_then(parse_json_rpc_message)
            {
                Ok(message) => self.inner.events.emit_message(message),
                Err(error) => self.inner.events.emit_error(&error),
            }
        }
    }

    fn emit_stderr_chunk(&self, chunk: &[u8]) {
        let max = self.inner.options.max_stderr_bytes;
        {
            let mut buffer = self
                .inner
                .stderr_buffer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            buffer.extend(chunk.iter().copied());
            while buffer.len() > max {
                buffer.pop_front();
            }
        }
        if let Some(on_stderr) = &self.inner.options.on_stderr {
            on_stderr(&String::from_utf8_lossy(chunk));
        }
    }

    async fn finish_stdout(&self) {
        let mut buffer = self.inner.stdout_buffer.lock().await;
        if buffer.iter().any(|byte| !byte.is_ascii_whitespace()) {
            self.inner.events.emit_error(&McpError::Transport(
                "MCP stdio server closed with an incomplete JSON-RPC message".to_string(),
            ));
        }
        buffer.clear();
    }
}

#[cfg(unix)]
fn kill_process_tree(pid: u32, signal: i32) {
    // Negative pid kills the process group created below, so wrappers like
    // npx/uvx do not leave the server running.
    let result = unsafe { libc::kill(-(pid as i32), signal) };
    if result != 0 {
        let _ = unsafe { libc::kill(pid as i32, signal) };
    }
}

#[cfg(not(unix))]
fn kill_process_tree(_pid: u32, _signal: i32) {}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn start(&self) -> Result<()> {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return Err(McpError::Transport(
                "MCP stdio transport already started".to_string(),
            ));
        }
        if self.is_closed() {
            self.inner.started.store(false, Ordering::SeqCst);
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        let options = &self.inner.options;
        let mut command = Command::new(&options.command);
        command.args(&options.args);
        if !options.inherit_env {
            command.env_clear();
        }
        command.envs(&options.env);
        if let Some(cwd) = &options.cwd {
            command.current_dir(cwd);
        }
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(match options.stderr {
            StdioStderr::Pipe => std::process::Stdio::piped(),
            StdioStderr::Inherit => std::process::Stdio::inherit(),
        });
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            CommandExt::process_group(command.as_std_mut(), 0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.inner.started.store(false, Ordering::SeqCst);
                return Err(McpError::Transport(format!(
                    "MCP server spawn failed: {error}"
                )));
            }
        };
        *self.inner.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = child.id();
        let stdin = child.stdin.take();
        *self.inner.stdin.lock().await = stdin;
        let Some(stdout) = child.stdout.take() else {
            return Err(McpError::Transport("MCP server has no stdout".to_string()));
        };
        *self.inner.child.lock().await = Some(child);

        {
            let transport = self.clone();
            let mut stdout = stdout;
            tokio::spawn(async move {
                let mut chunk = [0u8; 8192];
                loop {
                    match stdout.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => transport.emit_stdout_chunk(&chunk[..read]).await,
                    }
                }
                transport.finish_stdout().await;
                transport.inner.events.emit_close();
            });
        }
        if options.stderr == StdioStderr::Pipe {
            if let Some(mut stderr) = self
                .inner
                .child
                .lock()
                .await
                .as_mut()
                .and_then(|child| child.stderr.take())
            {
                let transport = self.clone();
                tokio::spawn(async move {
                    let mut chunk = [0u8; 8192];
                    loop {
                        match stderr.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => transport.emit_stderr_chunk(&chunk[..read]),
                        }
                    }
                });
            }
        }
        Ok(())
    }

    async fn send(&self, message: JsonRpcMessage, options: McpTransportSendOptions) -> Result<()> {
        if !self.inner.started.load(Ordering::SeqCst) || self.is_closed() {
            return Err(McpError::ConnectionClosed("MCP connection closed".to_string()));
        }
        if options.is_cancelled() {
            return Err(McpError::Aborted);
        }
        let payload = message.to_json_string()?;
        let mut stdin = self.inner.stdin.lock().await;
        let Some(stdin) = stdin.as_mut() else {
            return Err(McpError::ConnectionClosed("MCP stdio stdin closed".to_string()));
        };
        stdin.write_all(payload.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let stdin = self.inner.stdin.lock().await.take();
        drop(stdin);
        let mut child = self.inner.child.lock().await.take();
        if let Some(child) = child.as_mut() {
            let pid = child.id();
            let closed = tokio::time::timeout(STDIN_CLOSE_GRACE, child.wait()).await;
            if closed.is_err() {
                if let Some(pid) = pid {
                    #[cfg(unix)]
                    kill_process_tree(pid, libc::SIGTERM);
                    #[cfg(not(unix))]
                    let _ = child.start_kill();
                }
                let closed = tokio::time::timeout(self.inner.options.close_timeout, child.wait()).await;
                if closed.is_err() {
                    if let Some(pid) = pid {
                        #[cfg(unix)]
                        kill_process_tree(pid, libc::SIGKILL);
                        #[cfg(not(unix))]
                        let _ = child.start_kill();
                    }
                    let _ = child.wait().await;
                }
            }
        }
        self.inner.events.emit_close();
        Ok(())
    }

    fn on_message(&self, listener: MessageListener) -> Subscription {
        self.inner.events.on_message(listener)
    }

    fn on_error(&self, listener: ErrorListener) -> Subscription {
        self.inner.events.on_error(listener)
    }

    fn on_close(&self, listener: CloseListener) -> Subscription {
        self.inner.events.on_close(listener)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[tokio::test]
    async fn missing_command_fails_to_spawn() {
        let transport = StdioTransport::new(StdioTransportOptions::new(
            "/definitely/not/a/command-pi-mcp-test",
        ));
        assert!(transport.start().await.is_err());
    }

    #[tokio::test]
    async fn reads_newline_delimited_messages() {
        let transport = StdioTransport::new(StdioTransportOptions {
            command: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}'".to_string(),
            ],
            ..StdioTransportOptions::new("")
        });
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let _subscription = transport.on_message(Arc::new(move |message| {
            let _ = sender.send(message);
        }));
        transport.start().await.unwrap();
        let message = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.method(), Some("notifications/initialized"));
        transport.close().await.unwrap();
    }
}
