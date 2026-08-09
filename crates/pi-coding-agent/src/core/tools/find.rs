use std::sync::Arc;

use pi_agent_core::pi_ai_types::ContentBlock;
use pi_agent_core::types::{AgentTool, AgentToolResult};
use serde::{Deserialize, Serialize};

use super::path_utils;
use super::truncate::{self, TruncationResult};

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
pub trait FindOperations: Send + Sync {
    /// Check if path exists.
    fn exists(
        &self,
        path: &str,
    ) -> crate::core::tools::AsyncOpResult<bool>;

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
// LocalFindOperations
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
    fn exists(
        &self,
        path: &str,
    ) -> crate::core::tools::AsyncOpResult<bool> {
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
            // Walk with gitignore awareness (ripgrep's ignore crate), matching
            // the TS find tool's fd-based behavior:
            // - hidden files are included (fd --hidden);
            // - .gitignore / .ignore / .git/info/exclude are respected;
            // - inside a git repo, parent .gitignore rules stop at nested git
            //   repo boundaries (https://github.com/earendil-works/pi/issues/5960);
            // - outside a git repo, .gitignore is still respected (fd
            //   --no-require-git).
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

#[derive(Clone)]
pub struct FindToolOptions {
    pub operations: Arc<dyn FindOperations>,
}

impl std::fmt::Debug for FindToolOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FindToolOptions").finish()
    }
}

impl Default for FindToolOptions {
    fn default() -> Self {
        Self {
            operations: Arc::new(LocalFindOperations),
        }
    }
}

// ============================================================================
// Parameters schema
// ============================================================================

fn find_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Glob pattern to match files, e.g. '*.ts', '**/*.json'" },
            "path": { "type": "string", "description": "Directory to search in (default: current directory)" },
            "limit": { "type": "number", "description": "Maximum number of results (default: 1000)" }
        },
        "required": ["pattern"]
    })
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
        name: "find".to_string(),
        description: format!(
            "Find files matching a glob pattern. Returns matching file paths relative to the search directory. \
             Output is truncated to {} results or 256KB (whichever is hit first).",
            DEFAULT_LIMIT
        ),
        label: "Find".to_string(),
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
                    let search_path = params.get("path").and_then(|v| v.as_str());
                    let limit = params
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(DEFAULT_LIMIT);

                    // Check for abort
                    if let Some(ref rx) = signal {
                        if *rx.borrow() {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text("Operation aborted")],
                                details: serde_json::Value::Null,
                                usage: None,
                                added_tool_names: None,

                                terminate: None,
                            });
                        }
                    }

                    let search_cwd = match search_path {
                        Some(p) => path_utils::resolve_to_cwd(p, &cwd)
                            .to_string_lossy()
                            .to_string(),
                        None => cwd.clone(),
                    };

                    // Check if search path exists
                    if !std::path::Path::new(&search_cwd).exists() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text(format!(
                                "Path not found: {}",
                                search_path.unwrap_or(".")
                            ))],
                            details: serde_json::Value::Null,
                            usage: None,
                            added_tool_names: None,
                            terminate: None,
                        });
                    }

                    let ignore_dirs = [
                        "**/node_modules/**".to_string(),
                        "**/.git/**".to_string(),
                    ];

                    let results = match operations.glob(pattern, &search_cwd, &ignore_dirs, limit).await {
                        Ok(r) => r,
                        Err(e) => {
                            return Ok(AgentToolResult {
                                content: vec![ContentBlock::text(format!(
                                    "Error searching: {}",
                                    e
                                ))],
                                details: serde_json::Value::Null,
                                usage: None,
                                added_tool_names: None,
                                terminate: None,
                            });
                        }
                    };

                    if results.is_empty() {
                        return Ok(AgentToolResult {
                            content: vec![ContentBlock::text("No files found matching pattern")],
                            details: serde_json::Value::Null,
                            usage: None,
                            added_tool_names: None,

                            terminate: None,
                        });
                    }

                    // Relativize paths
                    let cwd_path = std::path::Path::new(&search_cwd);
                    let relativized: Vec<String> = results
                        .iter()
                        .map(|r| {
                            let path = std::path::Path::new(r);
                            path.strip_prefix(cwd_path)
                                .map(|p| format!("./{}", p.display()))
                                .unwrap_or_else(|_| r.clone())
                        })
                        .collect();

                    let result_limit_reached = relativized.len() >= limit;
                    let raw_output = relativized.join("\n");
                    let truncation = truncate::truncate_head(&raw_output, None);
                    let mut details = FindToolDetails::default();
                    let mut notices: Vec<String> = Vec::new();

                    if result_limit_reached {
                        notices.push(format!(
                            "{} results limit reached. Use limit={} for more, or refine pattern",
                            limit,
                            limit * 2
                        ));
                        details.result_limit_reached = Some(limit);
                    }
                    if truncation.truncated {
                        notices.push("256KB limit reached".to_string());
                        details.truncation = Some(truncation.clone());
                    }

                    let mut result_text = if truncation.truncated {
                        truncation.content
                    } else {
                        raw_output
                    };

                    if !notices.is_empty() {
                        result_text.push_str(&format!("\n\n[{}]", notices.join(". ")));
                    }

                    Ok(AgentToolResult {
                        content: vec![ContentBlock::text(result_text)],
                        details: serde_json::to_value(details).unwrap_or(serde_json::Value::Null),
                        usage: None,
                        added_tool_names: None,

                        terminate: None,
                    })
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
