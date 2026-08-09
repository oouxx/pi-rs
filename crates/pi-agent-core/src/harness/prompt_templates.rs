#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::harness::types::{ExecutionEnv, PromptTemplate};

#[derive(Debug, Clone)]
pub struct PromptTemplateDiagnostic {
    pub diagnostic_type: String,
    pub code: String,
    pub message: String,
    pub path: String,
}

pub fn format_prompt_template_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

/// Load prompt templates from a directory using the provided ExecutionEnv.
///
/// Scans for `.md` files, parses YAML frontmatter (delimited by `---`) for
/// `name` and `description` fields, and uses the remaining content as the
/// template body. Files without frontmatter are skipped.
///
/// Returns both loaded templates and any diagnostics (warnings/errors).
pub async fn load_prompt_templates(
    env: &dyn ExecutionEnv,
    path: &str,
) -> (Vec<PromptTemplate>, Vec<PromptTemplateDiagnostic>) {
    let mut templates = Vec::new();
    let mut diagnostics = Vec::new();

    let entries = match env.list_dir(path).await {
        Ok(entries) => entries,
        Err(e) => {
            diagnostics.push(PromptTemplateDiagnostic {
                diagnostic_type: "warning".to_string(),
                code: "list_dir_failed".to_string(),
                message: e.message,
                path: path.to_string(),
            });
            return (templates, diagnostics);
        }
    };

    for entry in entries {
        if entry.kind != "file" {
            continue;
        }
        if !entry.name.ends_with(".md") {
            continue;
        }

        let (tmpl, mut diags) = load_prompt_template_from_file(env, &entry.path).await;
        diagnostics.append(&mut diags);
        if let Some(t) = tmpl {
            templates.push(t);
        }
    }

    (templates, diagnostics)
}

async fn load_prompt_template_from_file(
    env: &dyn ExecutionEnv,
    file_path: &str,
) -> (Option<PromptTemplate>, Vec<PromptTemplateDiagnostic>) {
    let mut diagnostics = Vec::new();

    let content = match env.read_text_file(file_path, None).await {
        Ok(c) => c,
        Err(e) => {
            diagnostics.push(PromptTemplateDiagnostic {
                diagnostic_type: "warning".to_string(),
                code: "read_failed".to_string(),
                message: e.message,
                path: file_path.to_string(),
            });
            return (None, diagnostics);
        }
    };

    let (name, description, body, has_frontmatter) = parse_frontmatter(&content);

    let name = match name {
        Some(n) => n,
        None => {
            if has_frontmatter {
                diagnostics.push(PromptTemplateDiagnostic {
                    diagnostic_type: "warning".to_string(),
                    code: "missing_name".to_string(),
                    message: "No name found in frontmatter, skipping".to_string(),
                    path: file_path.to_string(),
                });
            }
            return (None, diagnostics);
        }
    };

    (
        Some(PromptTemplate {
            name,
            description: description.unwrap_or_default(),
            content: body,
        }),
        diagnostics,
    )
}

/// A sourced template input: a directory path and its source identifier.
#[derive(Debug, Clone)]
pub struct SourcedTemplateInput<S: Clone = String> {
    pub path: String,
    pub source: S,
}

/// A sourced prompt template diagnostic.
#[derive(Debug, Clone)]
pub struct SourcedPromptTemplateDiagnostic<S: Clone = String> {
    pub diagnostic: PromptTemplateDiagnostic,
    pub source: S,
}

/// Load prompt templates from multiple source directories, tracking origin.
pub async fn load_sourced_prompt_templates<S: Clone>(
    env: &dyn ExecutionEnv,
    inputs: &[SourcedTemplateInput<S>],
) -> (Vec<(PromptTemplate, S)>, Vec<SourcedPromptTemplateDiagnostic<S>>) {
    let mut templates = Vec::new();
    let mut diagnostics = Vec::new();

    for input in inputs {
        let (mut t, mut d) = load_prompt_templates(env, &input.path).await;
        for template in t.drain(..) {
            templates.push((template, input.source.clone()));
        }
        for diag in d.drain(..) {
            diagnostics.push(SourcedPromptTemplateDiagnostic {
                diagnostic: diag,
                source: input.source.clone(),
            });
        }
    }

    (templates, diagnostics)
}

/// Parse YAML-like frontmatter from markdown text.
///
/// Frontmatter is delimited by `---` at the start of the file.
/// Returns `(name, description, body)` extracted from the frontmatter and remaining text.
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>, String, bool) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, None, content.to_string(), false);
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let end_marker = after_first.find("\n---");
    if end_marker.is_none() {
        return (None, None, content.to_string(), true);
    }

    let end_idx = end_marker.unwrap();
    let frontmatter = &after_first[..end_idx].trim();
    let body = after_first[end_idx + 4..].trim().to_string();

    // Simple YAML-like key: value parsing
    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "name" => name = Some(value.to_string()),
                "description" => description = Some(value.to_string()),
                _ => {}
            }
        }
    }

    (name, description, body, true)
}

pub fn substitute_args(content: &str, args: &[String]) -> String {
    // Match TS `substituteArgs` (packages/coding-agent/src/core/prompt-templates.ts):
    //   ${N:-default} | ${ARGUMENTS:-default} | ${@:-default}  (default when missing/empty)
    //   ${@:N} | ${@:N:L}  (bash-style slicing; L is a length, not an absolute end)
    //   $ARGUMENTS | $@ | $N
    // Replacement happens on the template string only; argument and default values
    // containing patterns like $1, $@, or $ARGUMENTS are NOT recursively substituted.
    let re = regex::Regex::new(
        r"\$\{(\d+|ARGUMENTS|@):-([^}]*)\}|\$\{@:(\d+)(?::(\d+))?\}|\$(ARGUMENTS|@|\d+)",
    )
    .unwrap();
    let all_args = args.join(" ");

    re.replace_all(content, |caps: &regex::Captures| {
        if let Some(default_target) = caps.get(1) {
            let value = if default_target.as_str() == "@" || default_target.as_str() == "ARGUMENTS" {
                all_args.clone()
            } else {
                let num: usize = default_target.as_str().parse().unwrap_or(0);
                args.get(num.saturating_sub(1)).cloned().unwrap_or_default()
            };
            let default_value = caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
            return if value.is_empty() { default_value } else { value };
        }

        if let Some(slice_start) = caps.get(3) {
            // Convert to 0-indexed (user provides 1-indexed); treat 0 as 1
            // (bash convention: args start at 1), so saturating_sub handles it.
            let start: usize = slice_start
                .as_str()
                .parse::<usize>()
                .unwrap_or(1)
                .saturating_sub(1);
            if let Some(slice_length) = caps.get(4) {
                let length: usize = slice_length.as_str().parse().unwrap_or(0);
                let end = start.saturating_add(length).min(args.len());
                return args
                    .get(start..end)
                    .map(|slice: &[String]| slice.join(" "))
                    .unwrap_or_default();
            }
            return args
                .get(start..)
                .map(|slice: &[String]| slice.join(" "))
                .unwrap_or_default();
        }

        if let Some(simple) = caps.get(5) {
            let s = simple.as_str();
            if s == "ARGUMENTS" || s == "@" {
                return all_args.clone();
            }
            let num: usize = s.parse().unwrap_or(0);
            return args.get(num.saturating_sub(1)).cloned().unwrap_or_default();
        }

        String::new()
    })
    .to_string()
}

pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for ch in args_string.chars() {
        if let Some(qc) = in_quote {
            if ch == qc {
                in_quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        } else if ch == ' ' || ch == '\t' {
            if !current.is_empty() {
                args.push(current.clone());
                current.clear();
            }
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::env::nodejs::NodeExecutionEnv;
    use crate::harness::types::PromptTemplate;
    use tokio::fs;

    /// Helper to create a temp dir with test template files using NodeExecutionEnv.
    async fn setup_test_env(dir_name: &str, files: &[(&str, &str)]) -> (NodeExecutionEnv, String) {
        let base = std::env::temp_dir().join("pi_test_prompt_templates").join(dir_name);
        let _ = fs::remove_dir_all(&base).await;
        fs::create_dir_all(&base).await.unwrap();
        for (name, content) in files {
            fs::write(base.join(name), content).await.unwrap();
        }
        let env = NodeExecutionEnv::new(&base);
        (env, base.to_str().unwrap().to_string())
    }

    // --- substitute_args tests (unchanged) ---

    #[test]
    fn test_substitute_args_positional() {
        let content = "Hello $1, welcome $2";
        let args = vec!["world".to_string(), "home".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Hello world, welcome home");
    }

    #[test]
    fn test_substitute_args_missing_positional() {
        let content = "Hello $1, $2";
        let args = vec!["world".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Hello world, ");
    }

    #[test]
    fn test_substitute_args_arguments_variable() {
        let content = "Review $1 with $ARGUMENTS";
        let args = vec!["a.ts".to_string(), "care".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Review a.ts with a.ts care");
    }

    #[test]
    fn test_substitute_args_at_variable() {
        let content = "All: $@";
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "All: a b c");
    }

    #[test]
    fn test_substitute_args_range_from() {
        let content = "${@:2}";
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "b c");
    }

    #[test]
    fn test_substitute_args_range_with_end() {
        let content = "${@:1:2}";
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "a b");
    }

    #[test]
    fn test_substitute_args_range_out_of_bounds() {
        let content = "${@:5}";
        let args = vec!["a".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "");
    }

    #[test]
    fn test_substitute_args_combined() {
        let content = "$1 ${@:2} $ARGUMENTS";
        let args = vec!["hello".to_string(), "world".to_string(), "test".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "hello world test hello world test");
    }

    #[test]
    fn test_substitute_args_no_placeholders() {
        let content = "No placeholders here";
        let args: Vec<String> = vec![];
        let result = substitute_args(content, &args);
        assert_eq!(result, "No placeholders here");
    }

    #[test]
    fn test_substitute_args_empty_args() {
        let content = "Hello $1";
        let args: Vec<String> = vec![];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Hello ");
    }

    #[test]
    fn test_substitute_args_positional_default() {
        // ${N:-default}: use default when arg missing or empty (TS #6695)
        let content = "Hello ${1:-world}";
        let args: Vec<String> = vec![];
        assert_eq!(substitute_args(content, &args), "Hello world");
        let args = vec!["pi".to_string()];
        assert_eq!(substitute_args(content, &args), "Hello pi");
        // empty string counts as missing
        let args = vec!["".to_string()];
        assert_eq!(substitute_args(content, &args), "Hello world");
    }

    #[test]
    fn test_substitute_args_all_args_default() {
        // ${@:-default} / ${ARGUMENTS:-default}: default when all args empty (TS #6695)
        let content = "All: ${@:-none}";
        let args: Vec<String> = vec![];
        assert_eq!(substitute_args(content, &args), "All: none");
        let args = vec!["a".to_string(), "b".to_string()];
        assert_eq!(substitute_args(content, &args), "All: a b");

        let content = "All: ${ARGUMENTS:-none}";
        let args: Vec<String> = vec![];
        assert_eq!(substitute_args(content, &args), "All: none");
        let args = vec!["a".to_string()];
        assert_eq!(substitute_args(content, &args), "All: a");
    }

    #[test]
    fn test_substitute_args_range_length_semantics() {
        // ${@:N:L}: L is a length, not an absolute end (TS slice semantics)
        let content = "${@:2:2}";
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        assert_eq!(substitute_args(content, &args), "b c");
        // length beyond bounds clamps
        let content = "${@:3:5}";
        assert_eq!(substitute_args(content, &args), "c d");
        // ${@:0} treated as ${@:1} (bash convention)
        let content = "${@:0}";
        assert_eq!(substitute_args(content, &args), "a b c d");
    }

    #[test]
    fn test_substitute_args_defaults_not_recursive() {
        // Default values containing $1/$@ are NOT recursively substituted (TS note)
        let content = "${1:-$2}";
        let args = vec!["a".to_string(), "b".to_string()];
        assert_eq!(substitute_args(content, &args), "a");
        let content = "${1:-$2}";
        let args: Vec<String> = vec![];
        assert_eq!(substitute_args(content, &args), "$2");
    }

    #[test]
    fn test_format_prompt_template_invocation() {
        let template = PromptTemplate {
            name: "review".to_string(),
            description: "Review code".to_string(),
            content: "Review $1 with $ARGUMENTS".to_string(),
        };
        let args = vec!["a.ts".to_string(), "care".to_string()];
        let result = format_prompt_template_invocation(&template, &args);
        assert_eq!(result, "Review a.ts with a.ts care");
    }

    #[test]
    fn test_parse_command_args_simple() {
        let result = parse_command_args("hello world");
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_quoted() {
        let result = parse_command_args("hello \"world test\"");
        assert_eq!(result, vec!["hello", "world test"]);
    }

    #[test]
    fn test_parse_command_args_single_quoted() {
        let result = parse_command_args("hello 'world test'");
        assert_eq!(result, vec!["hello", "world test"]);
    }

    #[test]
    fn test_parse_command_args_empty() {
        let result = parse_command_args("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_command_args_extra_spaces() {
        let result = parse_command_args("  hello   world  ");
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_tabs() {
        let result = parse_command_args("hello\tworld");
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_mixed_quotes() {
        let result = parse_command_args("hello \"world 'test'\"");
        assert_eq!(result, vec!["hello", "world 'test'"]);
    }

    #[test]
    fn test_parse_command_args_unclosed_quote() {
        let result = parse_command_args("hello \"world");
        assert_eq!(result, vec!["hello", "world"]);
    }

    // ============================================================
    // Tests for frontmatter parsing
    // ============================================================

    #[test]
    fn test_parse_frontmatter_basic() {
        let content = "---\nname: review\ndescription: Review code\n---\nReview $1 with $ARGUMENTS";
        let (name, description, body, has_fm) = parse_frontmatter(content);
        assert!(has_fm);
        assert_eq!(name, Some("review".to_string()));
        assert_eq!(description, Some("Review code".to_string()));
        assert_eq!(body, "Review $1 with $ARGUMENTS");
    }

    #[test]
    fn test_parse_frontmatter_no_name() {
        let content = "---\ndescription: Just a description\n---\nBody text";
        let (name, description, _body, has_fm) = parse_frontmatter(content);
        assert!(has_fm);
        assert_eq!(name, None);
        assert_eq!(description, Some("Just a description".to_string()));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Just plain text without frontmatter";
        let (name, description, body, has_fm) = parse_frontmatter(content);
        assert!(!has_fm);
        assert_eq!(name, None);
        assert_eq!(description, None);
        assert_eq!(body, "Just plain text without frontmatter");
    }

    #[test]
    fn test_parse_frontmatter_unclosed() {
        let content = "---\nname: test\nBody text without closing marker";
        let (name, _description, body, has_fm) = parse_frontmatter(content);
        assert!(has_fm);
        assert_eq!(name, None);
        assert_eq!(body, content);
    }

    #[test]
    fn test_parse_frontmatter_with_extra_fields() {
        let content = "---\nname: template1\ndescription: A template\ntools: [bash, read]\n---\nTemplate body";
        let (name, description, body, has_fm) = parse_frontmatter(content);
        assert!(has_fm);
        assert_eq!(name, Some("template1".to_string()));
        assert_eq!(description, Some("A template".to_string()));
        assert_eq!(body, "Template body");
    }

    #[test]
    fn test_parse_frontmatter_multiline_body() {
        let content = "---\nname: multi\n---\nLine 1\nLine 2\nLine 3";
        let (name, _description, body, has_fm) = parse_frontmatter(content);
        assert!(has_fm);
        assert_eq!(name, Some("multi".to_string()));
        assert!(body.contains("Line 1"));
        assert!(body.contains("Line 3"));
    }

    // ============================================================
    // Tests for async template loading with NodeExecutionEnv
    // ============================================================

    #[tokio::test]
    async fn test_load_prompt_templates_empty_dir() {
        let (env, path) = setup_test_env("empty_dir", &[]).await;
        let (templates, diagnostics) = load_prompt_templates(&env, &path).await;
        assert!(templates.is_empty());
        assert!(diagnostics.is_empty());
        let _ = fs::remove_dir_all(&path).await;
    }

    #[tokio::test]
    async fn test_load_prompt_templates_with_file() {
        let (env, path) = setup_test_env("with_file", &[
            ("hello.md", "---\nname: hello\ndescription: Say hello\n---\nHello $1!"),
        ]).await;
        let (templates, diagnostics) = load_prompt_templates(&env, &path).await;
        assert!(diagnostics.is_empty());
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "hello");
        assert_eq!(templates[0].description, "Say hello");
        assert_eq!(templates[0].content, "Hello $1!");
        let _ = fs::remove_dir_all(&path).await;
    }

    #[tokio::test]
    async fn test_load_prompt_templates_skips_no_frontmatter() {
        let (env, path) = setup_test_env("skip_no_fm", &[
            ("no_fm.md", "Just text without frontmatter"),
            ("has_fm.md", "---\nname: ok\n---\nTemplate body"),
        ]).await;
        let (templates, diagnostics) = load_prompt_templates(&env, &path).await;
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "ok");
        // no_fm.md should not produce a diagnostic — it's silently skipped (no `---`)
        assert!(diagnostics.is_empty());
        let _ = fs::remove_dir_all(&path).await;
    }

    #[tokio::test]
    async fn test_load_prompt_templates_skips_non_md() {
        let (env, path) = setup_test_env("skip_non_md", &[
            ("template.txt", "---\nname: test\n---\nBody"),
        ]).await;
        let (templates, diagnostics) = load_prompt_templates(&env, &path).await;
        assert!(templates.is_empty());
        assert!(diagnostics.is_empty());
        let _ = fs::remove_dir_all(&path).await;
    }

    #[tokio::test]
    async fn test_load_prompt_templates_diagnostic_on_missing_name() {
        let (env, path) = setup_test_env("missing_name", &[
            ("no_name.md", "---\ndescription: no name\n---\nBody"),
        ]).await;
        let (templates, diagnostics) = load_prompt_templates(&env, &path).await;
        assert!(templates.is_empty());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "missing_name");
        let _ = fs::remove_dir_all(&path).await;
    }

    #[tokio::test]
    async fn test_load_sourced_prompt_templates() {
        let (env, path_a) = setup_test_env("sourced_a", &[
            ("a.md", "---\nname: a\n---\nTemplate A"),
        ]).await;
        let (_, path_b) = setup_test_env("sourced_b", &[
            ("b.md", "---\nname: b\n---\nTemplate B"),
        ]).await;

        let inputs = vec![
            SourcedTemplateInput { path: path_a.clone(), source: "source_a".to_string() },
            SourcedTemplateInput { path: path_b.clone(), source: "source_b".to_string() },
        ];

        let (templates, diagnostics) = load_sourced_prompt_templates(&env, &inputs).await;
        assert!(diagnostics.is_empty());
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0].0.name, "a");
        assert_eq!(templates[0].1, "source_a");
        assert_eq!(templates[1].0.name, "b");
        assert_eq!(templates[1].1, "source_b");

        let _ = fs::remove_dir_all(&path_a).await;
        let _ = fs::remove_dir_all(&path_b).await;
    }

    #[tokio::test]
    async fn test_load_prompt_templates_nonexistent_dir() {
        let env = NodeExecutionEnv::new("/nonexistent/path");
        let (templates, diagnostics) = load_prompt_templates(&env, "/nonexistent/path").await;
        assert!(templates.is_empty());
        assert!(!diagnostics.is_empty());
        assert_eq!(diagnostics[0].code, "list_dir_failed");
    }
}
