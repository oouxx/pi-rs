//! Find tool — port of `packages/coding-agent/src/core/tools/find.ts`.
//!
//! Searches with the `fd` binary, resolved through
//! [`crate::utils::tools_manager::ensure_tool`]: managed install in the pi bin
//! directory first, then `fd`/`fdfind` on PATH, then a GitHub release download.
//! This matches TS, which shells out to `fd` and reports an error when the
//! binary is unavailable.
//!
//! Callers that pass their own [`FindOperations`] (for example to search over
//! SSH) bypass `fd` entirely, exactly like TS `customOps?.glob`. The pure-Rust
//! [`LocalFindOperations`] implementation is kept as an opt-in backend (it walks
//! with ripgrep's `ignore` crate); the default tool path never uses it.

use std::process::Stdio;
use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

use super::path_utils;
use super::truncate::{self, truncate_head_bytes_only, TruncationResult, DEFAULT_MAX_BYTES};
use super::wait_for_abort;
use crate::utils::tools_manager;

const DEFAULT_LIMIT: usize = 1000;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindToolInput {
    pub pattern: String,
    pub path: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FindToolDetails {
    pub truncation: Option<TruncationResult>,
    pub result_limit_reached: Option<usize>,
}

// ============================================================================
// FindOperations trait
// ============================================================================

/// Pluggable operations for the find tool.
/// Override these to delegate file search to remote systems (for example SSH).
///
/// Matching TS `FindOperations`: providing operations replaces the `fd` search.
pub trait FindOperations: Send + Sync {
    /// Check if path exists.
    fn exists(&self, path: &str) -> crate::core::tools::AsyncOpResult<bool>;

    /// Find files matching glob pattern. Returns paths.
    fn glob(
        &self,
        pattern: &str,
        cwd: &str,
        ignore: &[String],
        limit: usize,
    ) -> crate::core::tools::AsyncOpResult<Vec<String>>;
}

// ============================================================================
// LocalFindOperations (opt-in, no external binary)
// ============================================================================

/// Check whether `cwd` is inside a git repository by walking up to the root,
/// matching the TS find tool's `insideGitRepo` detection.
fn is_inside_git_repo(cwd: &std::path::Path) -> bool {
    let mut current = Some(cwd);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return true;
        }
        current = dir.parent();
    }
    false
}

pub struct LocalFindOperations;

impl FindOperations for LocalFindOperations {
    fn exists(&self, path: &str) -> crate::core::tools::AsyncOpResult<bool> {
        let path = path.to_string();
        Box::pin(async move { Ok(std::path::Path::new(&path).exists()) })
    }

    fn glob(
        &self,
        pattern: &str,
        cwd: &str,
        ignore: &[String],
        limit: usize,
    ) -> crate::core::tools::AsyncOpResult<Vec<String>> {
        let pattern = pattern.to_string();
        let cwd = cwd.to_string();
        let ignore = ignore.to_vec();
        Box::pin(async move {
            let full_pattern = if std::path::Path::new(&pattern).is_absolute() {
                pattern.clone()
            } else {
                format!("{}/{}", cwd, pattern)
            };
            let matcher = glob::Pattern::new(&full_pattern)
                .map_err(|e| format!("Invalid glob pattern: {e}"))?;
            let match_options = glob::MatchOptions {
                case_sensitive: true,
                require_literal_separator: false,
                require_literal_leading_dot: false,
            };

            let mut results = Vec::new();
            // Gitignore-aware walk (ripgrep `ignore`), matching the TS fd behavior:
            // hidden files included; .gitignore/.ignore/.git/info/exclude respected
            // even outside a git repo; parent rules stop at nested repo boundaries.
            let inside_git = is_inside_git_repo(std::path::Path::new(&cwd));
            let walker = ignore::WalkBuilder::new(&cwd)
                // `hidden(false)` = do NOT ignore hidden files (include them,
                // matching fd --hidden). The ignore crate's `hidden(true)` is
                // the default and means "ignore hidden files".
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .require_git(inside_git)
                .filter_entry(|e| e.file_name() != ".git")
                .build();
            for entry in walker {
                if results.len() >= limit {
                    break;
                }
                let Ok(entry) = entry else { continue };
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                let path = entry.path();
                let path_str = path.to_string_lossy().to_string();
                let skip = ignore.iter().any(|ign| path_str.contains(ign));
                if skip {
                    continue;
                }
                if matcher.matches_path_with(path, match_options) {
                    results.push(path_str);
                }
            }
            Ok(results)
        })
    }
}

// ============================================================================
// FindToolOptions
// ============================================================================

/// `operations: None` = default local search through the `fd` binary
/// (matching TS `customOps === undefined`).
#[derive(Clone, Default)]
pub struct FindToolOptions {
    pub operations: Option<Arc<dyn FindOperations>>,
}

impl std::fmt::Debug for FindToolOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FindToolOptions")
            .field("operations", &self.operations.as_ref().map(|_| "custom"))
            .finish()
    }
}

// ============================================================================
// Parameters schema
// ============================================================================

fn find_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'" },
            "path": { "type": "string", "description": "Directory to search in (default: current directory)" },
            "limit": { "type": "number", "description": "Maximum number of results (default: 1000)" }
        },
        "required": ["pattern"]
    })
}

// ============================================================================
// Output formatting
// ============================================================================

/// Relativize a find result against the search root and normalize it to posix
/// separators (TS `relativizeFindResultPath`).
fn relativize_find_result_path(result_path: &str, search_path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let had_trailing_separator =
        result_path.ends_with(sep) || (sep == '\\' && result_path.ends_with('/'));
    let relative = if std::path::Path::new(result_path).is_absolute() {
        match std::path::Path::new(result_path).strip_prefix(search_path) {
            Ok(relative) => relative.to_string_lossy().to_string(),
            Err(_) => result_path.to_string(),
        }
    } else {
        result_path.to_string()
    };
    let posix_path = relative.replace(sep, "/");
    if had_trailing_separator && !posix_path.ends_with('/') {
        format!("{posix_path}/")
    } else {
        posix_path
    }
}

/// Build the `fd` argument vector, matching TS `createFindToolDefinition`.
fn build_fd_args(
    pattern: &str,
    search_path: &str,
    limit: usize,
    inside_git_repo: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--glob".to_string(),
        "--color=never".to_string(),
        "--hidden".to_string(),
    ];
    // fd normally ignores .gitignore outside git repos, so keep --no-require-git
    // there. Inside repos, use fd's default git-aware behavior so parent
    // .gitignore rules stop at nested repo boundaries:
    // https://github.com/earendil-works/pi/issues/5960
    if !inside_git_repo {
        args.push("--no-require-git".to_string());
    }
    args.push("--max-results".to_string());
    args.push(limit.to_string());

    // fd --glob matches against the basename unless --full-path is set; in
    // --full-path mode it matches against the absolute candidate path, so a
    // path-containing pattern like 'src/**/*.spec.ts' needs a leading '**/' to
    // match anything.
    let mut effective_pattern = pattern.to_string();
    if pattern.contains('/') {
        args.push("--full-path".to_string());
        if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
            effective_pattern = format!("**/{pattern}");
        }
        // fd matches full paths using native separators on Windows.
        if cfg!(target_os = "windows") {
            effective_pattern = effective_pattern.replace('/', "[\\/\\\\]");
        }
    }
    args.push("--".to_string());
    args.push(effective_pattern);
    args.push(search_path.to_string());
    args
}

/// Assemble the tool result from result lines (shared by both search paths).
fn emit_results(
    result_lines: &[String],
    effective_limit: usize,
    limit_notice: &str,
) -> AgentToolResult<serde_json::Value> {
    let result_limit_reached = result_lines.len() >= effective_limit;
    let raw_output = result_lines.join("\n");
    let truncation = truncate_head_bytes_only(&raw_output);
    let mut details = FindToolDetails::default();
    let mut notices: Vec<String> = Vec::new();

    if result_limit_reached {
        notices.push(limit_notice.to_string());
        details.result_limit_reached = Some(effective_limit);
    }
    if truncation.truncated {
        notices.push(format!(
            "{} limit reached",
            truncate::format_size(DEFAULT_MAX_BYTES)
        ));
        details.truncation = Some(truncation.clone());
    }

    let mut result_text = if truncation.truncated {
        truncation.content.clone()
    } else {
        raw_output
    };
    if !notices.is_empty() {
        result_text.push_str(&format!("\n\n[{}]", notices.join(". ")));
    }

    AgentToolResult {
        content: vec![ContentBlock::text(result_text)],
        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
        usage: None,
        terminate: None,
    }
}

fn text_result(text: &str) -> AgentToolResult<serde_json::Value> {
    AgentToolResult {
        content: vec![ContentBlock::text(text)],
        details: serde_json::Value::Null,
        usage: None,
        terminate: None,
    }
}

/// Run `fd` and collect its output lines.
async fn run_fd(
    search_path: &str,
    pattern: &str,
    effective_limit: usize,
    signal: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let fd_path = match tools_manager::ensure_tool_async("fd").await {
        Some(path) => path,
        None => {
            return Err("fd is not available and could not be downloaded"
                .to_string()
                .into());
        }
    };

    let inside_git_repo = is_inside_git_repo(std::path::Path::new(search_path));
    let args = build_fd_args(pattern, search_path, effective_limit, inside_git_repo);

    let mut child = tokio::process::Command::new(&fd_path)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run fd: {}", e))?;

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return Err("Failed to run fd: stdout unavailable".to_string().into()),
    };
    let stderr_pipe = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut buffer = String::new();
        if let Some(mut stderr) = stderr_pipe {
            let _ = stderr.read_to_string(&mut buffer).await;
        }
        buffer
    });

    let mut lines_iter = tokio::io::BufReader::new(stdout).lines();
    let mut lines: Vec<String> = Vec::new();
    let mut aborted = false;
    let mut signal_rx = signal.clone();

    loop {
        let next_line = match signal_rx.as_mut() {
            Some(rx) => {
                tokio::select! {
                    line = lines_iter.next_line() => line,
                    () = wait_for_abort(rx) => {
                        aborted = true;
                        let _ = child.kill().await;
                        break;
                    }
                }
            }
            None => lines_iter.next_line().await,
        };
        match next_line {
            Ok(Some(line)) => lines.push(line),
            Ok(None) => break,
            Err(_) => break,
        }
    }

    let status = child.wait().await.ok();
    let stderr = stderr_task.await.unwrap_or_default();

    if aborted {
        return Err("Operation aborted".to_string().into());
    }
    if status.and_then(|s| s.code()) != Some(0) && lines.is_empty() {
        let message = stderr.trim();
        if message.is_empty() {
            let code = status
                .and_then(|s| s.code())
                .map_or_else(|| "unknown".to_string(), |c| c.to_string());
            return Err(format!("fd exited with code {}", code).into());
        }
        return Err(message.to_string().into());
    }

    Ok(lines)
}

// ============================================================================
// create_find_tool
// ============================================================================

pub fn create_find_tool(
    cwd: &str,
    options: Option<FindToolOptions>,
) -> AgentTool<serde_json::Value, serde_json::Value> {
    let opts = options.unwrap_or_default();
    let cwd = cwd.to_string();
    let operations = opts.operations.clone();

    AgentTool {
        constrained_sampling: None,
        name: "find".to_string(),
        description: format!(
            "Search for files by glob pattern. Returns matching file paths relative to the search \
             directory. Respects .gitignore. Output is truncated to {} results or {}KB (whichever \
             is hit first).",
            DEFAULT_LIMIT,
            DEFAULT_MAX_BYTES / 1024
        ),
        label: "find".to_string(),
        prompt_snippet: Some("Find files by glob pattern (respects .gitignore)".to_string()),
        prompt_guidelines: None,
        parameters_schema: find_parameters_schema(),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(
            move |_tool_call_id: String,
                  params: serde_json::Value,
                  signal: Option<tokio::sync::watch::Receiver<bool>>,
                  _on_update: Option<
                Arc<dyn Fn(pi_agent_core::types::AgentToolResult<serde_json::Value>) + Send + Sync>,
            >| {
                let cwd = cwd.clone();
                let operations = operations.clone();
                Box::pin(async move {
                    let pattern = params.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                    let search_dir = params.get("path").and_then(|v| v.as_str());
                    let effective_limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(DEFAULT_LIMIT);

                    // Check for abort
                    if let Some(ref rx) = signal {
                        if *rx.borrow() {
                            return Err("Operation aborted".to_string().into());
                        }
                    }

                    let search_path = match search_dir {
                        Some(p) => path_utils::resolve_to_cwd(p, &cwd),
                        None => std::path::PathBuf::from(&cwd),
                    }
                    .to_string_lossy()
                    .to_string();

                    // Custom operations replace the `fd` search (TS
                    // `if (customOps?.glob)`).
                    if let Some(ops) = operations {
                        if !ops.exists(&search_path).await? {
                            return Err(format!("Path not found: {}", search_path).into());
                        }
                        if let Some(ref rx) = signal {
                            if *rx.borrow() {
                                return Err("Operation aborted".to_string().into());
                            }
                        }
                        let ignore_dirs =
                            ["**/node_modules/**".to_string(), "**/.git/**".to_string()];
                        let results = ops
                            .glob(pattern, &search_path, &ignore_dirs, effective_limit)
                            .await?;
                        if let Some(ref rx) = signal {
                            if *rx.borrow() {
                                return Err("Operation aborted".to_string().into());
                            }
                        }
                        if results.is_empty() {
                            return Ok(text_result("No files found matching pattern"));
                        }
                        let relativized: Vec<String> = results
                            .iter()
                            .map(|p| relativize_find_result_path(p, &search_path))
                            .collect();
                        return Ok(emit_results(
                            &relativized,
                            effective_limit,
                            &format!("{} results limit reached", effective_limit),
                        ));
                    }

                    let lines =
                        run_fd(&search_path, pattern, effective_limit, signal.clone()).await?;
                    if lines.is_empty() {
                        return Ok(text_result("No files found matching pattern"));
                    }

                    let relativized: Vec<String> = lines
                        .iter()
                        .map(|line| line.trim_end_matches('\r').trim())
                        .filter(|line| !line.is_empty())
                        .map(|line| relativize_find_result_path(line, &search_path))
                        .collect();

                    Ok(emit_results(
                        &relativized,
                        effective_limit,
                        &format!(
                            "{} results limit reached. Use limit={} for more, or refine pattern",
                            effective_limit,
                            effective_limit * 2
                        ),
                    ))
                })
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn test_find_tool_input_deserialization() {
        let json = serde_json::json!({
            "pattern": "**/*.rs",
            "path": "/tmp"
        });
        let input: FindToolInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.pattern, "**/*.rs");
        assert_eq!(input.path, Some("/tmp".to_string()));
    }

    #[test]
    fn test_find_tool_input_minimal() {
        let json = serde_json::json!({
            "pattern": "**/*.rs"
        });
        let input: FindToolInput = serde_json::from_value(json).unwrap();
        assert_eq!(input.pattern, "**/*.rs");
        assert_eq!(input.path, None);
    }

    #[test]
    fn test_find_parameters_schema() {
        let schema = find_parameters_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["pattern"].is_object());
        assert!(schema["properties"]["path"].is_object());
        assert_eq!(schema["required"][0], "pattern");
    }

    #[test]
    fn test_find_tool_creation() {
        let tool = create_find_tool("/tmp", None);
        assert_eq!(tool.name, "find");
        assert!(!tool.description.is_empty());
        assert!(tool.parameters_schema.is_object());
    }

    /// Arguments must match TS `createFindToolDefinition` exactly.
    #[test]
    fn test_build_fd_args() {
        let args = build_fd_args("*.txt", "/tmp/dir", 1000, true);
        assert_eq!(
            args,
            vec![
                "--glob",
                "--color=never",
                "--hidden",
                "--max-results",
                "1000",
                "--",
                "*.txt",
                "/tmp/dir",
            ]
        );

        // Outside a git repo: --no-require-git (fd's gitignore handling is
        // opt-in there).
        let outside = build_fd_args("*.txt", "/tmp/dir", 10, false);
        assert!(outside.iter().any(|a| a == "--no-require-git"));

        // Path-containing pattern: --full-path + leading '**/'.
        let nested = build_fd_args("src/**/*.spec.ts", "/tmp/dir", 10, true);
        assert!(nested.iter().any(|a| a == "--full-path"));
        assert!(nested.iter().any(|a| a == "**/src/**/*.spec.ts"));

        // Already-globbed / absolute patterns are not rewritten.
        let globbed = build_fd_args("**/*.ts", "/tmp/dir", 10, true);
        assert!(globbed.iter().any(|a| a == "--full-path"));
        assert!(globbed.iter().any(|a| a == "**/*.ts"));
    }

    #[test]
    fn test_relativize_find_result_path() {
        assert_eq!(
            relativize_find_result_path("/tmp/dir/sub/a.txt", "/tmp/dir"),
            "sub/a.txt"
        );
        assert_eq!(relativize_find_result_path("a.txt", "/tmp/dir"), "a.txt");
        // Paths outside the search root are left as-is.
        assert_eq!(
            relativize_find_result_path("/tmp/other/a.txt", "/tmp/dir"),
            "/tmp/other/a.txt"
        );
    }

    fn glob_in(dir: &std::path::Path, pattern: &str) -> Vec<String> {
        let ops = LocalFindOperations;
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            ops.glob(pattern, dir.to_str().unwrap(), &[], 1000)
                .await
                .unwrap()
        })
    }

    #[test]
    fn test_glob_basic_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "").unwrap();
        std::fs::write(dir.path().join("b.txt"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/c.rs"), "").unwrap();

        let results = glob_in(dir.path(), "**/*.rs");
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|p| p.ends_with("a.rs")));
        assert!(results.iter().any(|p| p.ends_with("sub/c.rs")));
    }

    #[test]
    fn test_glob_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "").unwrap();
        std::fs::write(dir.path().join("secret.log"), "").unwrap();

        let results = glob_in(dir.path(), "**/*");
        assert!(results.iter().any(|p| p.ends_with("keep.txt")));
        assert!(!results.iter().any(|p| p.ends_with("secret.log")));
    }

    #[test]
    fn test_glob_includes_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden.txt"), "").unwrap();
        std::fs::write(dir.path().join("visible.txt"), "").unwrap();

        let results = glob_in(dir.path(), "**/*.txt");
        assert!(results.iter().any(|p| p.ends_with(".hidden.txt")));
        assert!(results.iter().any(|p| p.ends_with("visible.txt")));
    }

    #[test]
    fn test_glob_nested_git_boundary() {
        // Parent .gitignore rules must stop at a nested git repo boundary
        // (https://github.com/earendil-works/pi/issues/5960).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(dir.path().join("parent.log"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("nested/.git")).unwrap();
        std::fs::write(dir.path().join("nested/child.log"), "").unwrap();

        let results = glob_in(dir.path(), "**/*.log");
        assert!(!results.iter().any(|p| p.ends_with("parent.log")));
        assert!(results.iter().any(|p| p.ends_with("nested/child.log")));
    }
}
