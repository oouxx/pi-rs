use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;

use crate::harness::types::{
    CreateDirOptions, ExecResult, ExecutionEnv, ExecutionEnvExecOptions, ExecutionError, FileError,
    FileInfoType, ReadTextFileOptions, RemoveOptions, TempFileOptions,
};

pub struct NodeExecutionEnv {
    cwd: PathBuf,
}

impl NodeExecutionEnv {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }
}

fn to_file_error(error: std::io::Error, path: &str) -> FileError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => "not_found".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission_denied".to_string(),
        std::io::ErrorKind::AlreadyExists => "already_exists".to_string(),
        _ => "unknown".to_string(),
    };
    FileError {
        code,
        message: format!("{}: {}", path, error),
    }
}

#[async_trait]
impl ExecutionEnv for NodeExecutionEnv {
    fn cwd(&self) -> &str {
        self.cwd.to_str().unwrap_or(".")
    }

    async fn read_text_file(
        &self,
        path: &str,
        options: Option<ReadTextFileOptions>,
    ) -> std::result::Result<String, FileError> {
        let resolved = self.resolve_path(path);
        let content = fs::read_to_string(&resolved)
            .await
            .map_err(|e| to_file_error(e, path))?;

        if let Some(opts) = options {
            if let Some(max_lines) = opts.max_lines {
                let lines: Vec<&str> = content.lines().take(max_lines).collect();
                return Ok(lines.join("\n"));
            }
        }

        Ok(content)
    }

    async fn write_file(
        &self,
        path: &str,
        content: &str,
        _abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> std::result::Result<(), FileError> {
        let resolved = self.resolve_path(path);
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| to_file_error(e, path))?;
        }
        fs::write(&resolved, content)
            .await
            .map_err(|e| to_file_error(e, path))?;
        Ok(())
    }

    async fn append_file(&self, path: &str, content: &str) -> std::result::Result<(), FileError> {
        let resolved = self.resolve_path(path);
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| to_file_error(e, path))?;
        }
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved)
            .await
            .map_err(|e| to_file_error(e, path))?;
        file.write_all(content.as_bytes())
            .await
            .map_err(|e| to_file_error(e, path))?;
        Ok(())
    }

    async fn file_info(&self, path: &str) -> std::result::Result<FileInfoType, FileError> {
        let resolved = self.resolve_path(path);
        let metadata = fs::symlink_metadata(&resolved)
            .await
            .map_err(|e| to_file_error(e, path))?;
        let kind = if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };
        let name = resolved
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        Ok(FileInfoType {
            kind: kind.to_string(),
            name,
            path: resolved.to_str().unwrap_or("").to_string(),
        })
    }

    async fn list_dir(&self, path: &str) -> std::result::Result<Vec<FileInfoType>, FileError> {
        let resolved = self.resolve_path(path);
        let mut entries = fs::read_dir(&resolved)
            .await
            .map_err(|e| to_file_error(e, path))?;
        let mut infos = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| to_file_error(e, path))?
        {
            let path_str = entry.path().to_str().unwrap_or("").to_string();
            let name = entry.file_name().to_str().unwrap_or("").to_string();
            let metadata = entry
                .metadata()
                .await
                .map_err(|e| to_file_error(e, &path_str))?;
            let kind = if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else {
                "other"
            };
            infos.push(FileInfoType {
                kind: kind.to_string(),
                name,
                path: path_str,
            });
        }
        Ok(infos)
    }

    async fn canonical_path(&self, path: &str) -> std::result::Result<String, FileError> {
        let resolved = self.resolve_path(path);
        let canonical = fs::canonicalize(&resolved)
            .await
            .map_err(|e| to_file_error(e, path))?;
        Ok(canonical.to_str().unwrap_or("").to_string())
    }

    async fn exists(&self, path: &str) -> std::result::Result<bool, FileError> {
        let resolved = self.resolve_path(path);
        match fs::symlink_metadata(&resolved).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(to_file_error(e, path)),
        }
    }

    async fn create_dir(
        &self,
        path: &str,
        options: Option<CreateDirOptions>,
    ) -> std::result::Result<(), FileError> {
        let resolved = self.resolve_path(path);
        let recursive = options.as_ref().map_or(true, |o| o.recursive);
        if recursive {
            fs::create_dir_all(&resolved)
                .await
                .map_err(|e| to_file_error(e, path))?;
        } else {
            fs::create_dir(&resolved)
                .await
                .map_err(|e| to_file_error(e, path))?;
        }
        Ok(())
    }

    async fn remove(
        &self,
        path: &str,
        options: Option<RemoveOptions>,
    ) -> std::result::Result<(), FileError> {
        let resolved = self.resolve_path(path);
        let recursive = options.as_ref().map_or(false, |o| o.recursive);
        let force = options.as_ref().map_or(false, |o| o.force);
        if recursive {
            match fs::remove_dir_all(&resolved).await {
                Ok(_) => Ok(()),
                Err(e) if force && e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(to_file_error(e, path)),
            }
        } else {
            match fs::remove_file(&resolved).await {
                Ok(_) => Ok(()),
                Err(e) if force && e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(to_file_error(e, path)),
            }
        }
    }

    async fn create_temp_dir(&self, prefix: &str) -> std::result::Result<String, FileError> {
        let temp_dir = std::env::temp_dir();
        let dir = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(&temp_dir)
            .map_err(|e| FileError {
                code: "temp_dir_failed".to_string(),
                message: e.to_string(),
            })?;
        Ok(dir.path().to_str().unwrap_or("").to_string())
    }

    async fn create_temp_file(
        &self,
        options: Option<TempFileOptions>,
    ) -> std::result::Result<String, FileError> {
        let prefix = options
            .as_ref()
            .and_then(|o| o.prefix.clone())
            .unwrap_or_default();
        let suffix = options
            .as_ref()
            .and_then(|o| o.suffix.clone())
            .unwrap_or_default();
        let temp_dir = std::env::temp_dir();
        let file = tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(&suffix)
            .tempfile_in(&temp_dir)
            .map_err(|e| FileError {
                code: "temp_file_failed".to_string(),
                message: e.to_string(),
            })?;
        Ok(file.path().to_str().unwrap_or("").to_string())
    }

    async fn exec(
        &self,
        command: &str,
        options: ExecutionEnvExecOptions,
    ) -> std::result::Result<ExecResult, ExecutionError> {
        let cwd = options
            .cwd
            .as_deref()
            .unwrap_or_else(|| self.cwd.to_str().unwrap_or("."));

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .output()
            .await
            .map_err(|e| ExecutionError::Unknown(e.to_string()))?;

        Ok(ExecResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    async fn cleanup(&self) {}
}
