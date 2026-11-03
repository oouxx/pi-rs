//! MCP server log messages (`notifications/message`) appended to `mcp.log`.
//!
//! Port of `packages/coding-agent/src/extensions/mcp/log.ts`. Messages are
//! written one synchronous append at a time so several pi processes can share
//! the file; it rotates to `mcp.log.1` above 5 MiB.

use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

fn format_data(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => "null".to_string(),
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(value) => serde_json::to_string(value).unwrap_or_else(|_| value.to_string()),
    }
}

/// Format one `notifications/message` from `server` as a log line.
pub fn format_mcp_log_message(server: &str, params: Option<&serde_json::Value>) -> String {
    let message = match params {
        Some(serde_json::Value::Object(object)) => object.clone(),
        Some(value) => {
            let mut object = serde_json::Map::new();
            object.insert("data".to_string(), value.clone());
            object
        }
        None => serde_json::Map::new(),
    };
    let level = message
        .get("level")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("info");
    let logger = message
        .get("logger")
        .and_then(serde_json::Value::as_str)
        .filter(|logger| !logger.is_empty())
        .map(|logger| format!(" {logger}:"))
        .unwrap_or_default();
    let text = format_data(message.get("data")).replace("\r\n", "\n").replace('\n', "\n    ");
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    format!("{timestamp} [{server}] {level}{logger} {text}\n")
}

/// Appends server log messages to one file. Write errors are ignored.
pub struct McpServerLog {
    path: PathBuf,
}

impl std::fmt::Debug for McpServerLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerLog")
            .field("path", &self.path)
            .finish()
    }
}

impl McpServerLog {
    /// Create a log at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Log file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one message. Best effort: errors are ignored.
    pub fn write(&self, server: &str, params: Option<&serde_json::Value>) {
        let line = format_mcp_log_message(server, params);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let size = std::fs::metadata(&self.path).map(|meta| meta.len()).unwrap_or(0);
        if size > MAX_LOG_BYTES {
            let rotated = self.path.with_extension("log.1");
            let _ = std::fs::rename(&self.path, rotated);
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        if let Ok(mut file) = options.open(&self.path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_level_logger_and_multiline_data() {
        let line = format_mcp_log_message(
            "docs",
            Some(&json!({"level": "warn", "logger": "fetch", "data": "a\nb"})),
        );
        assert!(line.contains("[docs]"));
        assert!(line.contains("warn fetch:"));
        assert!(line.contains("a\n    b"));
    }

    #[test]
    fn appends_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = McpServerLog::new(dir.path().join("mcp.log"));
        log.write("s", Some(&json!({"data": "one"})));
        log.write("s", Some(&json!({"data": "two"})));
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(text.lines().count(), 2);
    }
}
