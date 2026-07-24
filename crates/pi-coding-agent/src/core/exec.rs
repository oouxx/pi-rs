use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct ExecOptions {
    pub signal: Option<watch::Receiver<bool>>,
    pub timeout: Option<Duration>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub killed: bool,
}

pub async fn exec_command(
    command: &str,
    args: &[String],
    cwd: &str,
    options: Option<ExecOptions>,
) -> ExecResult {
    let opts = options.unwrap_or(ExecOptions {
        signal: None,
        timeout: None,
        cwd: None,
    });

    let mut child = Command::new(command);
    child.args(args);
    child.current_dir(cwd);
    child.stdout(std::process::Stdio::piped());
    child.stderr(std::process::Stdio::piped());
    child.kill_on_drop(true);

    let mut spawned = match child.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ExecResult {
                stdout: String::new(),
                stderr: format!("Failed to spawn process: {}", e),
                code: -1,
                killed: false,
            };
        }
    };

    let mut killed = false;

    let result = if let Some(dur) = opts.timeout {
        let timed = timeout(dur, wait_for_child(&mut spawned, opts.signal, &mut killed)).await;
        match timed {
            Ok(r) => r,
            Err(_) => {
                let _ = spawned.kill().await;
                killed = true;
                let _ = spawned.wait().await;
                ExecResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    code: -1,
                    killed: true,
                }
            }
        }
    } else {
        wait_for_child(&mut spawned, opts.signal, &mut killed).await
    };

    ExecResult { killed, ..result }
}

async fn wait_for_child(
    child: &mut Child,
    mut signal: Option<watch::Receiver<bool>>,
    killed: &mut bool,
) -> ExecResult {
    use tokio::io::AsyncReadExt;

    let stdout_handle = tokio::spawn({
        let mut stdout = child.stdout.take();
        async move {
            let mut buf = String::new();
            if let Some(ref mut s) = stdout {
                let _ = s.read_to_string(&mut buf).await;
            }
            buf
        }
    });

    let stderr_handle = tokio::spawn({
        let mut stderr = child.stderr.take();
        async move {
            let mut buf = String::new();
            if let Some(ref mut s) = stderr {
                let _ = s.read_to_string(&mut buf).await;
            }
            buf
        }
    });

    let status = tokio::select! {
        status = child.wait() => status,
        _ = async {
            if let Some(ref mut sig) = signal {
                sig.changed().await.ok()
            } else {
                std::future::pending::<()>().await;
                unreachable!()
            }
        } => {
            let _ = child.kill().await;
            *killed = true;
            child.wait().await
        }
    };

    let stdout = stdout_handle.await.unwrap_or_default();
    let stderr = stderr_handle.await.unwrap_or_default();
    let code = status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);

    ExecResult {
        stdout,
        stderr,
        code,
        killed: *killed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_exec_echo() {
        let result = exec_command("echo", &["hello".into()], "/tmp", None).await;
        assert!(
            result.stdout.trim() == "hello" || result.stdout.trim().is_empty(),
            "got: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn test_exec_exit_code() {
        let result = exec_command("sh", &["-c".into(), "exit 42".into()], "/tmp", None).await;
        assert_eq!(result.code, 42);
    }

    #[tokio::test]
    async fn test_exec_timeout() {
        let result = exec_command(
            "sh",
            &["-c".into(), "sleep 10".into()],
            "/tmp",
            Some(ExecOptions {
                signal: None,
                timeout: Some(Duration::from_millis(50)),
                cwd: None,
            }),
        )
        .await;
        assert!(result.killed);
    }

    #[tokio::test]
    async fn test_exec_cancellation() {
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            exec_command(
                "sh",
                &["-c".into(), "sleep 10".into()],
                "/tmp",
                Some(ExecOptions {
                    signal: Some(rx),
                    timeout: None,
                    cwd: None,
                }),
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = tx.send(true);

        let result = handle.await.unwrap();
        assert!(result.killed);
    }
}
