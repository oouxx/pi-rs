use serde::{Deserialize, Serialize};

use crate::core::tools::truncate::{self, DEFAULT_MAX_BYTES};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashExecutorResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    pub full_output_path: Option<String>,
}

pub struct BashExecutorOptions {
    pub on_chunk: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pub signal: Option<tokio::sync::watch::Receiver<bool>>,
}

impl std::fmt::Debug for BashExecutorOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BashExecutorOptions")
            .field("on_chunk", &self.on_chunk.as_ref().map(|_| "..."))
            .field("signal", &self.signal.is_some())
            .finish()
    }
}

impl Default for BashExecutorOptions {
    fn default() -> Self {
        Self {
            on_chunk: None,
            signal: None,
        }
    }
}

pub struct BashExecutor {
    cwd: String,
    max_bytes: usize,
    save_full_output: bool,
}

impl BashExecutor {
    pub fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_string(),
            max_bytes: DEFAULT_MAX_BYTES,
            save_full_output: true,
        }
    }

    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    pub fn with_save_full_output(mut self, save: bool) -> Self {
        self.save_full_output = save;
        self
    }

    pub async fn execute(
        &self,
        command: &str,
        options: Option<BashExecutorOptions>,
    ) -> Result<BashExecutorResult, Box<dyn std::error::Error + Send + Sync>> {
        let opts = options.unwrap_or_default();
        let shell = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "bash"
        };
        let shell_arg = if cfg!(target_os = "windows") {
            "/C"
        } else {
            "-c"
        };

        let mut cmd = tokio::process::Command::new(shell);
        cmd.arg(shell_arg)
            .arg(command)
            .current_dir(&self.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::null());

        if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
            cmd.process_group(0);
        }

        let mut child = cmd.spawn()?;

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

        let mut cancelled = false;

        let exit_code = loop {
            if let Some(ref mut rx) = opts.signal.as_ref() {
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

        let mut output = String::new();
        if !stdout_output.is_empty() {
            output.push_str(&stdout_output);
        }
        if !stderr_output.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&stderr_output);
        }

        if let Some(ref on_chunk) = opts.on_chunk {
            on_chunk(&output);
        }

        let truncation = truncate::truncate_tail(&output, Some(truncate::TruncationOptions {
            max_bytes: Some(self.max_bytes),
            ..Default::default()
        }));

        let full_output_path = if self.save_full_output && truncation.truncated {
            let path = self.save_full_output_to_temp(&output)?;
            Some(path)
        } else {
            None
        };

        Ok(BashExecutorResult {
            output: truncation.content,
            exit_code,
            cancelled,
            truncated: truncation.truncated,
            full_output_path,
        })
    }

    fn save_full_output_to_temp(
        &self,
        output: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = std::env::temp_dir();
        let file_name = format!("pi-bash-output-{}.log", uuid::Uuid::new_v4());
        let path = temp_dir.join(&file_name);
        std::fs::write(&path, output)?;
        Ok(path.to_string_lossy().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bash_executor_echo() {
        let executor = BashExecutor::new("/tmp");
        let result = executor.execute("echo hello", None).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.exit_code, Some(0));
        assert!(r.output.contains("hello"));
        assert!(!r.cancelled);
        assert!(!r.truncated);
    }

    #[tokio::test]
    async fn test_bash_executor_error() {
        let executor = BashExecutor::new("/tmp");
        let result = executor.execute("exit 42", None).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.exit_code, Some(42));
    }
}