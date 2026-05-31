use std::fmt;
use std::process::Stdio;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::truncate::{self, TruncationResult};

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

pub trait BashOperations: Send + Sync {
    fn execute(
        &self,
        command: &str,
        cwd: &str,
        env: Option<Vec<(String, String)>>,
        signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<BashResult, Box<dyn std::error::Error + Send + Sync>>> + Send>>;
}

#[derive(Debug, Clone)]
pub struct BashResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
}

pub struct LocalBashOperations;

impl BashOperations for LocalBashOperations {
    fn execute(
        &self,
        command: &str,
        cwd: &str,
        _env: Option<Vec<(String, String)>>,
        mut signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<BashResult, Box<dyn std::error::Error + Send + Sync>>> + Send>> {
        let command = command.to_string();
        let cwd = cwd.to_string();
        Box::pin(async move {
            let shell = if cfg!(target_os = "windows") { "cmd" } else { "bash" };
            let shell_arg = if cfg!(target_os = "windows") { "/C" } else { "-c" };

            let mut cmd = tokio::process::Command::new(shell);
            cmd.arg(shell_arg)
                .arg(&command)
                .current_dir(&cwd)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());

            if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
                cmd.process_group(0);
            }

            let mut child = cmd.spawn().map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let stdout_task = tokio::spawn(async move {
                if let Some(mut out) = stdout {
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut out, &mut buf).await;
                    String::from_utf8_lossy(&buf).to_string()
                } else {
                    String::new()
                }
            });

            let stderr_task = tokio::spawn(async move {
                if let Some(mut err) = stderr {
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut err, &mut buf).await;
                    String::from_utf8_lossy(&buf).to_string()
                } else {
                    String::new()
                }
            });

            let timed_out = false;
            let mut cancelled = false;

            let exit_code = loop {
                if let Some(ref mut rx) = signal {
                    if rx.has_changed().unwrap_or(false) {
                        let _ = child.kill().await;
                        cancelled = true;
                        break None;
                    }
                }
                match tokio::time::timeout(std::time::Duration::from_millis(100), child.wait()).await {
                    Ok(Ok(status)) => break status.code(),
                    Ok(Err(_)) => break None,
                    Err(_) => continue,
                }
            };

            let stdout_output = stdout_task.await.unwrap_or_default();
            let stderr_output = stderr_task.await.unwrap_or_default();

            Ok(BashResult {
                exit_code,
                stdout: stdout_output,
                stderr: stderr_output,
                timed_out,
                cancelled,
            })
        })
    }
}

#[derive(Clone)]
pub struct BashToolOptions {
    pub operations: Arc<dyn BashOperations>,
    pub command_prefix: Option<String>,
    pub shell_path: Option<String>,
}

impl fmt::Debug for BashToolOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BashToolOptions")
            .field("command_prefix", &self.command_prefix)
            .field("shell_path", &self.shell_path)
            .finish()
    }
}

impl Default for BashToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalBashOperations),
            command_prefix: None,
            shell_path: None,
        }
    }
}

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

pub fn create_bash_tool(cwd: &str, options: Option<BashToolOptions>) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();
    let command_prefix = opts.command_prefix.clone();

    AgentTool {
        name: "bash".to_string(),
        description: "Execute a bash command on the local machine.".to_string(),
        label: "Bash".to_string(),
        parameters_schema: bash_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id: String, params: serde_json::Value, signal: Option<tokio::sync::watch::Receiver<bool>>, _on_update: Option<Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>>| {
            let cwd = cwd.clone();
            let operations = operations.clone();
            let command_prefix = command_prefix.clone();
            Box::pin(async move {
                let command = params.get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let full_command = if let Some(ref prefix) = command_prefix {
                    format!("{}; {}", prefix, command)
                } else {
                    command.clone()
                };

                let result = operations.execute(&full_command, &cwd, None, signal).await;

                match result {
                    Ok(bash_result) => {
                        let mut output = String::new();
                        if !bash_result.stdout.is_empty() {
                            output.push_str(&bash_result.stdout);
                        }
                        if !bash_result.stderr.is_empty() {
                            if !output.is_empty() {
                                output.push('\n');
                            }
                            output.push_str(&bash_result.stderr);
                        }

                        let truncation = truncate::truncate_tail(&output, None);

                        let exit_code = bash_result.exit_code;
                        let timed_out = bash_result.timed_out;
                        let cancelled = bash_result.cancelled;

                        let is_truncated = truncation.truncated;
                        let truncation_clone = truncation.clone();
                        let mut output_text = truncation.content;
                        if timed_out {
                            output_text = format!("Command timed out\n{}", output_text);
                        }
                        if cancelled {
                            output_text = format!("Command cancelled\n{}", output_text);
                        }
                        if let Some(code) = exit_code {
                            if code != 0 {
                                output_text = format!("Exit code: {}\n{}", code, output_text);
                            }
                        }

                        let truncation_for_details = if is_truncated {
                            Some(truncation_clone)
                        } else {
                            None
                        };

                        let details = serde_json::to_value(BashToolDetails {
                            truncation: truncation_for_details,
                            ..Default::default()
                        }).unwrap_or(serde_json::Value::Null);

                        Ok(AgentToolResult {
                            content: vec![ContentBlock::text(output_text)],
                            details,
                            terminate: None,
                        })
                    }
                    Err(e) => Ok(AgentToolResult {
                        content: vec![ContentBlock::text(format!("Error: {}", e))],
                        details: serde_json::to_value(BashToolDetails::default())
                            .unwrap_or(serde_json::Value::Null),
                        terminate: None,
                    }),
                }
            })
        }),
    }
}