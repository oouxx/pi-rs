//! JSON-RPC client for the Bun extension sidecar process.
//!
//! Communicates with `rpc-host/src/index.ts` via line-delimited JSON over stdin/stdout.
//! The sidecar loads TypeScript extensions and executes their tool handlers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin};
use tokio::sync::Mutex;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("Sidecar not started")]
    NotStarted,
    #[error("Sidecar already running")]
    AlreadyRunning,
    #[error("Sidecar process error: {0}")]
    ProcessError(#[from] std::io::Error),
    #[error("JSON-RPC error (code={code}): {message}")]
    RpcError { code: i64, message: String },
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Response timeout")]
    Timeout,
    #[error("Bun binary not found; install bun to enable extension execution")]
    BunNotFound,
    #[error("No response for request {id}")]
    NoResponse { id: u64 },
}

// ============================================================================
// JSON-RPC Protocol
// ============================================================================

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RpcResponse {
    Success {
        id: u64,
        result: serde_json::Value,
    },
    Error {
        id: u64,
        error: RpcErrorDetail,
    },
}

#[derive(Deserialize)]
struct RpcErrorDetail {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct RpcNotification {
    method: String,
}

// ============================================================================
// Response types from sidecar
// ============================================================================

#[derive(Deserialize, Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    #[serde(default)]
    pub prompt_guidelines: Option<Vec<String>>,
    #[serde(default)]
    pub execution_mode: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct CommandInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct LoadError {
    pub path: String,
    pub error: String,
}

#[derive(Deserialize, Debug)]
pub struct LoadResult {
    pub tools: Vec<ToolInfo>,
    pub commands: Vec<CommandInfo>,
    pub errors: Vec<LoadError>,
}

#[derive(Deserialize, Debug)]
pub struct CallToolResponse {
    pub result: serde_json::Value,
    #[serde(default)]
    pub notifications: Vec<String>,
}

// ============================================================================
// ExtensionsRpcClient
// ============================================================================

/// Manages the Bun subprocess lifecycle and JSON-RPC communication.
///
/// Uses separate locks for stdin and the response channel so that writing
/// a request and reading the matching response don't conflict.
pub struct ExtensionsRpcClient {
    bun_path: String,
    rpc_host_path: String,
    next_id: AtomicU64,

    /// Guarded by its own lock — one writer at a time.
    stdin: Arc<Mutex<Option<BufWriter<ChildStdin>>>>,
    /// Guarded by its own lock — one reader at a time.
    response_rx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>>,
    /// Child process handle (kill on drop).
    child: Arc<Mutex<Option<Child>>>,

    /// Tool name cache (name → name).
    tool_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl Clone for ExtensionsRpcClient {
    fn clone(&self) -> Self {
        ExtensionsRpcClient {
            bun_path: self.bun_path.clone(),
            rpc_host_path: self.rpc_host_path.clone(),
            next_id: AtomicU64::new(self.next_id.load(Ordering::SeqCst)),
            stdin: Arc::clone(&self.stdin),
            response_rx: Arc::clone(&self.response_rx),
            child: Arc::clone(&self.child),
            tool_cache: Arc::clone(&self.tool_cache),
        }
    }
}

impl ExtensionsRpcClient {
    /// Create a new RPC client. Call `start()` to launch the sidecar.
    pub fn new() -> Self {
        let rpc_host_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("rpc-host").join("src").join("index.ts"))
            .unwrap_or_else(|| std::path::PathBuf::from("rpc-host/src/index.ts"));

        ExtensionsRpcClient {
            bun_path: "bun".to_string(),
            rpc_host_path: rpc_host_path.to_string_lossy().to_string(),
            next_id: AtomicU64::new(1),
            stdin: Arc::new(Mutex::new(None)),
            response_rx: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
            tool_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if bun is available on the system.
    pub fn is_available() -> bool {
        std::process::Command::new("bun")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Start the Bun sidecar subprocess.
    pub async fn start(&self) -> Result<(), RpcError> {
        {
            let child_lock = self.child.lock().await;
            if child_lock.is_some() {
                return Err(RpcError::AlreadyRunning);
            }
        }

        let mut child = tokio::process::Command::new(&self.bun_path)
            .arg("run")
            .arg(&self.rpc_host_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(RpcError::ProcessError)?;

        let child_stdin = child
            .stdin
            .take()
            .ok_or_else(|| {
                RpcError::ProcessError(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Failed to capture stdin",
                ))
            })?;

        let child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| {
                RpcError::ProcessError(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Failed to capture stdout",
                ))
            })?;

        // Spawn a background task that reads stdout lines into a channel
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let reader = BufReader::new(child_stdout);
        tokio::spawn(async move {
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        // Store handles in their respective locks
        *self.stdin.lock().await = Some(BufWriter::new(child_stdin));
        *self.response_rx.lock().await = Some(rx);
        *self.child.lock().await = Some(child);

        // Wait for the "ready" notification
        self.wait_for_ready().await?;

        Ok(())
    }

    /// Wait for the initial `ready` notification from the sidecar.
    async fn wait_for_ready(&self) -> Result<(), RpcError> {
        for _ in 0..50 {
            let line = {
                let mut rx_lock = self.response_rx.lock().await;
                let rx = rx_lock.as_mut().ok_or(RpcError::NotStarted)?;

                use tokio::time::timeout;
                match timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                    Ok(Some(l)) => l,
                    _ => continue,
                }
            };

            if let Ok(notif) = serde_json::from_str::<RpcNotification>(&line) {
                if notif.method == "ready" {
                    return Ok(());
                }
            }
        }
        Err(RpcError::Timeout)
    }

    /// Load extensions via the sidecar.
    pub async fn load_extensions(
        &self,
        cwd: &str,
        agent_dir: &str,
        extension_paths: &[String],
    ) -> Result<LoadResult, RpcError> {
        let params = serde_json::json!({
            "cwd": cwd,
            "agentDir": agent_dir,
            "extensionPaths": extension_paths,
        });

        let response: serde_json::Value = self.call("load", Some(params)).await?;
        let result: LoadResult = serde_json::from_value(response)?;

        // Update tool cache
        let mut cache = self.tool_cache.lock().await;
        cache.clear();
        for tool in &result.tools {
            cache.insert(tool.name.clone(), tool.name.clone());
        }

        Ok(result)
    }

    /// Call an extension tool handler.
    pub async fn call_tool(
        &self,
        tool_name: &str,
        args: serde_json::Value,
        cwd: &str,
    ) -> Result<CallToolResponse, RpcError> {
        let params = serde_json::json!({
            "toolName": tool_name,
            "toolArgs": args,
            "cwd": cwd,
        });

        let response: serde_json::Value = self.call("call_tool", Some(params)).await?;
        let result: CallToolResponse = serde_json::from_value(response)?;
        Ok(result)
    }

    /// Reload all extensions.
    pub async fn reload(
        &self,
        cwd: &str,
        agent_dir: &str,
        extension_paths: &[String],
    ) -> Result<LoadResult, RpcError> {
        let params = serde_json::json!({
            "cwd": cwd,
            "agentDir": agent_dir,
            "extensionPaths": extension_paths,
        });
        let response: serde_json::Value = self.call("reload", Some(params)).await?;
        let result: LoadResult = serde_json::from_value(response)?;

        let mut cache = self.tool_cache.lock().await;
        cache.clear();
        for tool in &result.tools {
            cache.insert(tool.name.clone(), tool.name.clone());
        }

        Ok(result)
    }

    /// Gracefully stop the sidecar.
    pub async fn stop(&self) -> Result<(), RpcError> {
        let _ = self.call("shutdown", None::<serde_json::Value>).await;

        // Clean up child process
        let mut child_lock = self.child.lock().await;
        if let Some(mut child) = child_lock.take() {
            let _ = child.wait().await;
        }

        // Release handles
        *self.stdin.lock().await = None;
        *self.response_rx.lock().await = None;

        Ok(())
    }

    /// Check if the sidecar is running.
    pub async fn is_running(&self) -> bool {
        self.child.lock().await.is_some()
    }

    /// Get a list of loaded tool names.
    pub async fn loaded_tool_names(&self) -> Vec<String> {
        let cache = self.tool_cache.lock().await;
        cache.keys().cloned().collect()
    }

    // ========================================================================
    // Internal: JSON-RPC call
    // ========================================================================

    async fn call(
        &self,
        method: &'static str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        // Write request (lock stdin only for the write)
        {
            let mut stdin_lock = self.stdin.lock().await;
            let stdin = stdin_lock
                .as_mut()
                .ok_or(RpcError::NotStarted)?;

            let request = RpcRequest {
                jsonrpc: "2.0",
                id,
                method,
                params,
            };
            let line = serde_json::to_string(&request)?;
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        } // stdin lock released

        // Read response (lock response_rx only for each recv)
        use tokio::time::timeout;
        let deadline = std::time::Duration::from_secs(30);

        loop {
            let line = {
                let mut rx_lock = self.response_rx.lock().await;
                let rx = rx_lock
                    .as_mut()
                    .ok_or(RpcError::NotStarted)?;

                match timeout(deadline, rx.recv()).await {
                    Ok(Some(l)) => l,
                    Ok(None) => return Err(RpcError::NoResponse { id }),
                    Err(_) => return Err(RpcError::Timeout),
                }
            }; // rx lock released

            // Try parsing as notification (skip if no valid id)
            let Ok(response) = serde_json::from_str::<RpcResponse>(&line) else {
                continue;
            };

            match response {
                RpcResponse::Success { id: resp_id, result } => {
                    if resp_id == id {
                        return Ok(result);
                    }
                }
                RpcResponse::Error { id: resp_id, error } => {
                    if resp_id == id {
                        return Err(RpcError::RpcError {
                            code: error.code,
                            message: error.message,
                        });
                    }
                }
            }
        }
    }
}

impl Drop for ExtensionsRpcClient {
    fn drop(&mut self) {
        // The child process is dropped via kill_on_drop.
        // The stdin/stdout handles are closed when dropped.
    }
}

// ============================================================================
// AgentTool Wrapper — creates executable AgentTools from extension ToolInfo
// ============================================================================

use std::sync::Arc as StdArc;
use pi_agent_core::types::{AgentTool, AgentToolResult};

/// Wrap extension tools into AgentTools that call the RPC sidecar.
///
/// Each tool's `execute` sends a JSON-RPC `call_tool` request to the Bun
/// sidecar, which invokes the TypeScript handler and returns the result.
pub fn create_extension_agent_tools(
    tools: &[ToolInfo],
    client: StdArc<ExtensionsRpcClient>,
    cwd: String,
) -> Vec<AgentTool<serde_json::Value, serde_json::Value>> {
    tools
        .iter()
        .map(|info| {
            let client = StdArc::clone(&client);
            let tool_cwd = cwd.clone();
            let tool_name = info.name.clone();

            AgentTool {
                name: info.name.clone(),
                description: info.description.clone(),
                label: String::new(),
                parameters_schema: info
                    .parameters
                    .clone()
                    .unwrap_or(serde_json::Value::Null),
                execution_mode: info.execution_mode.as_ref().and_then(|m| match m.as_str() {
                    "sequential" => {
                        Some(pi_agent_core::pi_ai_types::ToolExecutionMode::Sequential)
                    }
                    _ => None,
                }),
                prepare_arguments: None,
                execute: StdArc::new(move |_call_id, args, _signal, _on_update| {
                    let client = StdArc::clone(&client);
                    let cwd = tool_cwd.clone();
                    let name = tool_name.clone();
                    Box::pin(async move {
                        let response = client
                            .call_tool(&name, args, &cwd)
                            .await
                            .map_err(|e| {
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    format!("Extension tool '{name}' failed: {e}"),
                                )) as Box<dyn std::error::Error + Send + Sync>
                            })?;

                        Ok(AgentToolResult {
                            content: Vec::new(),
                            details: response.result,
                            terminate: None,
                        })
                    })
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tool_wrapper_tests {
    use super::*;

    #[test]
    fn test_create_extension_agent_tools_empty() {
        let tools: Vec<ToolInfo> = vec![];
        let client = ExtensionsRpcClient::new();
        let result = create_extension_agent_tools(&tools, StdArc::new(client), "/tmp".into());
        assert!(result.is_empty());
    }

    #[test]
    fn test_create_extension_agent_tools_basic() {
        let tools = vec![ToolInfo {
            name: "my-tool".into(),
            description: "A test tool".into(),
            parameters: Some(serde_json::json!({"type": "object"})),
            prompt_guidelines: None,
            execution_mode: None,
        }];
        let client = ExtensionsRpcClient::new();
        let result = create_extension_agent_tools(&tools, StdArc::new(client), "/tmp".into());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "my-tool");
        assert_eq!(result[0].description, "A test tool");
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_extension_path() -> PathBuf {
        PathBuf::from("/tmp/test-extensions/test-hello/index.ts")
    }

    fn make_client() -> ExtensionsRpcClient {
        ExtensionsRpcClient::new()
    }

    #[tokio::test]
    async fn test_sidecar_ping() {
        if !ExtensionsRpcClient::is_available() {
            eprintln!("Skipping test: bun not available");
            return;
        }

        let client = make_client();
        client.start().await.expect("should start sidecar");
        assert!(client.is_running().await);

        client.stop().await.expect("should stop sidecar");
        assert!(!client.is_running().await);
    }

    #[tokio::test]
    async fn test_load_extensions() {
        if !ExtensionsRpcClient::is_available() {
            eprintln!("Skipping test: bun not available");
            return;
        }

        let ext_path = test_extension_path();
        if !ext_path.exists() {
            eprintln!("Skipping test: test extension not found at {:?}", ext_path);
            return;
        }

        let client = make_client();
        client.start().await.expect("should start sidecar");

        let result = client
            .load_extensions(
                "/tmp",
                "/tmp",
                &[ext_path.to_string_lossy().to_string()],
            )
            .await
            .expect("should load extensions");

        // Verify the test extension's tools are registered
        assert_eq!(result.tools.len(), 2, "expected 2 tools from test extension");
        assert_eq!(result.tools[0].name, "hello");
        assert_eq!(result.tools[1].name, "exec_test");

        // Verify commands
        assert_eq!(result.commands.len(), 1);
        assert_eq!(result.commands[0].name, "greet");

        // Verify no errors
        assert!(result.errors.is_empty(), "load errors: {:?}", result.errors);

        // Verify tool cache is populated
        let names = client.loaded_tool_names().await;
        assert!(names.contains(&"hello".to_string()));
        assert!(names.contains(&"exec_test".to_string()));

        client.stop().await.expect("should stop sidecar");
    }

    #[tokio::test]
    async fn test_call_tool() {
        if !ExtensionsRpcClient::is_available() {
            eprintln!("Skipping test: bun not available");
            return;
        }

        let ext_path = test_extension_path();
        if !ext_path.exists() {
            eprintln!("Skipping test: test extension not found at {:?}", ext_path);
            return;
        }

        let client = make_client();
        client.start().await.expect("should start sidecar");
        client
            .load_extensions("/tmp", "/tmp", &[ext_path.to_string_lossy().to_string()])
            .await
            .expect("should load extensions");

        // Call the hello tool
        let args = serde_json::json!({ "name": "Rust" });
        let response = client
            .call_tool("hello", args, "/tmp")
            .await
            .expect("should call tool");

        // Verify result
        assert_eq!(
            response.result,
            serde_json::json!({"greeting": "Hello, Rust! from pi extension"})
        );

        // Verify notification was captured
        assert!(response.notifications.len() >= 1);

        client.stop().await.expect("should stop sidecar");
    }

    #[tokio::test]
    async fn test_call_tool_with_exec() {
        if !ExtensionsRpcClient::is_available() {
            eprintln!("Skipping test: bun not available");
            return;
        }

        let ext_path = test_extension_path();
        if !ext_path.exists() {
            eprintln!("Skipping test: test extension not found at {:?}", ext_path);
            return;
        }

        let client = make_client();
        client.start().await.expect("should start sidecar");
        client
            .load_extensions("/tmp", "/tmp", &[ext_path.to_string_lossy().to_string()])
            .await
            .expect("should load extensions");

        // Test ctx.exec() from extension handler
        let args = serde_json::json!({ "command": "world" });
        let response = client
            .call_tool("exec_test", args, "/tmp")
            .await
            .expect("should call exec_test tool");

        // Verify exec output
        let stdout = response.result["stdout"].as_str().unwrap_or("");
        assert!(stdout.contains("hello world"), "expected 'hello world' in stdout, got: '{}'", stdout);

        client.stop().await.expect("should stop sidecar");
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        if !ExtensionsRpcClient::is_available() {
            eprintln!("Skipping test: bun not available");
            return;
        }

        let client = make_client();
        client.start().await.expect("should start sidecar");

        let result = client.call_tool("nonexistent_tool", serde_json::json!({}), "/tmp").await;
        assert!(result.is_err(), "expected error for nonexistent tool");

        match result {
            Err(RpcError::RpcError { code: _, message }) => {
                assert!(message.contains("not found"));
            }
            _ => panic!("expected RpcError with 'not found'"),
        }

        client.stop().await.expect("should stop sidecar");
    }
}
