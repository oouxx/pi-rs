use crate::harness::types::PromptTemplate;
use std::fs;
use std::path::Path;

pub fn format_prompt_template_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

/// Load prompt templates from a directory.
///
/// Scans for `.md` files, parses YAML frontmatter (delimited by `---`) for
/// `name` and `description` fields, and uses the remaining content as the
/// template body. Files without frontmatter are skipped.
///
/// Template bodies support substitution variables: `$1`, `$2`, `$@`, `$ARGUMENTS`, `${@:N}`.
pub fn load_prompt_templates(templates_dir: &Path) -> Vec<PromptTemplate> {
    let mut templates = Vec::new();

    let entries = match fs::read_dir(templates_dir) {
        Ok(entries) => entries,
        Err(_) => return templates,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let (name, description, body) = parse_frontmatter(&content);

        if let Some(name) = name {
            templates.push(PromptTemplate {
                name,
                description: description.unwrap_or_default(),
                content: body,
            });
        }
    }

    templates
}

/// A sourced template input: a directory path and its source identifier.
#[derive(Debug, Clone)]
pub struct SourcedTemplateInput<S: Clone = String> {
    pub path: std::path::PathBuf,
    pub source: S,
}

/// Load prompt templates from multiple source directories, tracking origin.
pub fn load_sourced_prompt_templates<S: Clone>(
    inputs: &[SourcedTemplateInput<S>],
) -> Vec<(PromptTemplate, S)> {
    let mut templates = Vec::new();

    for input in inputs {
        let loaded = load_prompt_templates(&input.path);
        for template in loaded {
            templates.push((template, input.source.clone()));
        }
    }

    templates
}

/// Parse YAML-like frontmatter from markdown text.
///
/// Frontmatter is delimited by `---` at the start of the file.
/// Returns `(name, description, body)` extracted from the frontmatter and remaining text.
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, None, content.to_string());
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let end_marker = after_first.find("\n---");
    if end_marker.is_none() {
        return (None, None, content.to_string());
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

    (name, description, body)
}

pub fn substitute_args(content: &str, args: &[String]) -> String {
    let mut result = content.to_string();

    let re_num = regex::Regex::new(r"\$(\d+)").unwrap();
    let args_owned: Vec<String> = args.to_vec();
    result = re_num
        .replace_all(&result, |caps: &regex::Captures| {
            let num: usize = caps[1].parse().unwrap_or(0);
            args_owned
                .get(num.saturating_sub(1))
                .cloned()
                .unwrap_or_default()
        })
        .to_string();

    let re_range = regex::Regex::new(r"\$\{@:(\d+)(?::(\d+))?\}").unwrap();
    let args_owned2: Vec<String> = args.to_vec();
    result = re_range
        .replace_all(&result, |caps: &regex::Captures| {
            let start: usize = caps[1].parse::<usize>().unwrap_or(1).saturating_sub(1);
            let end: Option<usize> = caps.get(2).and_then(|m| m.as_str().parse::<usize>().ok());
            match end {
                Some(e) => args_owned2
                    .get(start..e.min(args_owned2.len()))
                    .map(|slice: &[String]| slice.join(" "))
                    .unwrap_or_default(),
                None => args_owned2
                    .get(start..)
                    .map(|slice: &[String]| slice.join(" "))
                    .unwrap_or_default(),
            }
        })
        .to_string();

    let all_args = args.join(" ");
    result = result.replace("$ARGUMENTS", &all_args);
    result = result.replace("$@", &all_args);

    result
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
    use crate::harness::types::PromptTemplate;

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
    // Tests for frontmatter parsing and template loading
    // ============================================================

    #[test]
    fn test_parse_frontmatter_basic() {
        let content = "---\nname: review\ndescription: Review code\n---\nReview $1 with $ARGUMENTS";
        let (name, description, body) = parse_frontmatter(content);
        assert_eq!(name, Some("review".to_string()));
        assert_eq!(description, Some("Review code".to_string()));
        assert_eq!(body, "Review $1 with $ARGUMENTS");
    }

    #[test]
    fn test_parse_frontmatter_no_name() {
        let content = "---\ndescription: Just a description\n---\nBody text";
        let (name, description, body) = parse_frontmatter(content);
        assert_eq!(name, None);
        assert_eq!(description, Some("Just a description".to_string()));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Just plain text without frontmatter";
        let (name, description, body) = parse_frontmatter(content);
        assert_eq!(name, None);
        assert_eq!(description, None);
        assert_eq!(body, "Just plain text without frontmatter");
    }

    #[test]
    fn test_parse_frontmatter_unclosed() {
        let content = "---\nname: test\nBody text without closing marker";
        let (name, description, body) = parse_frontmatter(content);
        assert_eq!(name, None);
        assert_eq!(body, content);
    }

    #[test]
    fn test_parse_frontmatter_with_extra_fields() {
        let content = "---\nname: template1\ndescription: A template\ntools: [bash, read]\n---\nTemplate body";
        let (name, description, body) = parse_frontmatter(content);
        assert_eq!(name, Some("template1".to_string()));
        assert_eq!(description, Some("A template".to_string()));
        assert_eq!(body, "Template body");
    }

    #[test]
    fn test_parse_frontmatter_multiline_body() {
        let content = "---\nname: multi\n---\nLine 1\nLine 2\nLine 3";
        let (name, description, body) = parse_frontmatter(content);
        assert_eq!(name, Some("multi".to_string()));
        assert!(body.contains("Line 1"));
        assert!(body.contains("Line 3"));
    }

    #[test]
    fn test_load_prompt_templates_empty_dir() {
        let dir = std::env::temp_dir().join("pi_test_templates_empty");
        let _ = fs::create_dir_all(&dir);
        let templates = load_prompt_templates(&dir);
        assert!(templates.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_prompt_templates_with_file() {
        let dir = std::env::temp_dir().join("pi_test_templates_load");
        let _ = fs::create_dir_all(&dir);
        let content = "---\nname: hello\ndescription: Say hello\n---\nHello $1!";
        fs::write(dir.join("hello.md"), content).unwrap();
        let templates = load_prompt_templates(&dir);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "hello");
        assert_eq!(templates[0].description, "Say hello");
        assert_eq!(templates[0].content, "Hello $1!");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_prompt_templates_skips_no_frontmatter() {
        let dir = std::env::temp_dir().join("pi_test_templates_skip");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("no_fm.md"), "Just text without frontmatter").unwrap();
        fs::write(dir.join("has_fm.md"), "---\nname: ok\n---\nTemplate body").unwrap();
        let templates = load_prompt_templates(&dir);
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "ok");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_prompt_templates_skips_non_md() {
        let dir = std::env::temp_dir().join("pi_test_templates_non_md");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("template.txt"), "---\nname: test\n---\nBody").unwrap();
        let templates = load_prompt_templates(&dir);
        assert!(templates.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}