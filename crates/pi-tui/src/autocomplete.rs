use std::path::Path;

use crate::fuzzy::fuzzy_filter;

#[derive(Debug, Clone)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

pub struct AutocompleteSuggestions {
    pub items: Vec<AutocompleteItem>,
    pub prefix: String,
}

pub trait AutocompleteProvider: Send + Sync {
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions>;

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> ApplyResult;

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool;
}

#[derive(Debug, Clone)]
pub struct ApplyResult {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

impl Default for ApplyResult {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            cursor_line: 0,
            cursor_col: 0,
        }
    }
}

pub struct SlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub get_argument_completions: Option<Box<dyn Fn(&str) -> Option<Vec<AutocompleteItem>> + Send + Sync>>,
}

fn to_display_path(value: &str) -> String {
    value.replace('\\', "/")
}

fn escape_regex(value: &str) -> String {
    let special = ['.', '*', '+', '?', '^', '$', '{', '}', '(', ')', '|', '[', ']', '\\'];
    let mut result = String::with_capacity(value.len());
    for c in value.chars() {
        if special.contains(&c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

fn find_last_delimiter(text: &str) -> Option<usize> {
    let delimiters = [' ', '\t', '"', '\'', '='];
    text.rfind(|c| delimiters.contains(&c))
}

fn find_unclosed_quote_start(text: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut quote_start = None;

    for (i, c) in text.char_indices() {
        if c == '"' {
            in_quotes = !in_quotes;
            if in_quotes {
                quote_start = Some(i);
            }
        }
    }

    if in_quotes { quote_start } else { None }
}

fn is_token_start(text: &str, index: usize) -> bool {
    index == 0 || text[..index].chars().last().map_or(true, |c| matches!(c, ' ' | '\t' | '"' | '\'' | '='))
}

fn extract_quoted_prefix(text: &str) -> Option<String> {
    let quote_start = find_unclosed_quote_start(text)?;

    if quote_start > 0 {
        let prev = text[..quote_start].chars().last().unwrap();
        if prev == '@' {
            if !is_token_start(text, quote_start - 1) {
                return None;
            }
            return Some(text[quote_start - 1..].to_string());
        }
    }

    if !is_token_start(text, quote_start) {
        return None;
    }

    Some(text[quote_start..].to_string())
}

#[derive(Debug, Clone)]
struct PathPrefixInfo {
    raw_prefix: String,
    is_at_prefix: bool,
    is_quoted_prefix: bool,
}

fn parse_path_prefix(prefix: &str) -> PathPrefixInfo {
    if prefix.starts_with("@\"") {
        PathPrefixInfo {
            raw_prefix: prefix[2..].to_string(),
            is_at_prefix: true,
            is_quoted_prefix: true,
        }
    } else if prefix.starts_with('"') {
        PathPrefixInfo {
            raw_prefix: prefix[1..].to_string(),
            is_at_prefix: false,
            is_quoted_prefix: true,
        }
    } else if prefix.starts_with('@') {
        PathPrefixInfo {
            raw_prefix: prefix[1..].to_string(),
            is_at_prefix: true,
            is_quoted_prefix: false,
        }
    } else {
        PathPrefixInfo {
            raw_prefix: prefix.to_string(),
            is_at_prefix: false,
            is_quoted_prefix: false,
        }
    }
}

fn build_completion_value(path: &str, _is_directory: bool, is_at_prefix: bool, is_quoted_prefix: bool) -> String {
    let needs_quotes = is_quoted_prefix || path.contains(' ');
    let prefix = if is_at_prefix { "@/"} else { "" };

    if !needs_quotes {
        return format!("{}{}", prefix, path);
    }

    format!("{}\"{}\"", prefix, path)
}

pub struct CombinedAutocompleteProvider {
    commands: Vec<Box<dyn SlashCommandLike + Send + Sync>>,
    base_path: String,
}

trait SlashCommandLike {
    fn name(&self) -> &str;
    fn description(&self) -> Option<&str>;
    fn argument_hint(&self) -> Option<&str>;
    fn get_argument_completions(&self, argument_prefix: &str) -> Option<Vec<AutocompleteItem>>;
}

impl SlashCommandLike for SlashCommand {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> Option<&str> { self.description.as_deref() }
    fn argument_hint(&self) -> Option<&str> { self.argument_hint.as_deref() }
    fn get_argument_completions(&self, argument_prefix: &str) -> Option<Vec<AutocompleteItem>> {
        self.get_argument_completions.as_ref().and_then(|f| f(argument_prefix))
    }
}

impl CombinedAutocompleteProvider {
    pub fn new(commands: Vec<Box<dyn SlashCommandLike + Send + Sync>>, base_path: String) -> Self {
        Self { commands, base_path }
    }

    fn extract_at_prefix(&self, text: &str) -> Option<String> {
        if let Some(quoted) = extract_quoted_prefix(text) {
            return Some(quoted);
        }

        let last_delim = find_last_delimiter(text);
        let token_start = match last_delim {
            Some(i) => i + 1,
            None => 0,
        };

        if text[token_start..].starts_with('@') {
            Some(text[token_start..].to_string())
        } else {
            None
        }
    }

    fn extract_path_prefix(&self, text: &str, force_extract: bool) -> Option<String> {
        if let Some(quoted) = extract_quoted_prefix(text) {
            return Some(quoted);
        }

        let last_delim = find_last_delimiter(text);
        let path_prefix = match last_delim {
            Some(i) => text[i + 1..].to_string(),
            None => text.to_string(),
        };

        if force_extract {
            return Some(path_prefix);
        }

        if path_prefix.contains('/') || path_prefix.starts_with('.') || path_prefix.starts_with("~/") {
            return Some(path_prefix);
        }

        if path_prefix.is_empty() && text.ends_with(' ') {
            return Some(path_prefix);
        }

        None
    }

    fn expand_home_path(&self, path: &str) -> String {
        if path.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
            let expanded = format!("{}/{}", home.trim_end_matches('/'), &path[2..]);
            if path.ends_with('/') && !expanded.ends_with('/') {
                format!("{}/", expanded)
            } else {
                expanded
            }
        } else if path == "~" {
            std::env::var("HOME").unwrap_or_else(|_| "/root".to_string())
        } else {
            path.to_string()
        }
    }

    fn get_file_suggestions(&self, prefix: &str) -> Vec<AutocompleteItem> {
        let info = parse_path_prefix(prefix);
        let expanded_prefix = self.expand_home_path(&info.raw_prefix);

        let (search_dir, search_prefix, display_base) = if expanded_prefix.ends_with('/') || expanded_prefix.is_empty() || info.raw_prefix.is_empty() {
            let dir = if expanded_prefix.starts_with('/') {
                expanded_prefix.clone()
            } else if expanded_prefix.starts_with("~") {
                expanded_prefix.clone()
            } else {
                format!("{}/{}", self.base_path, expanded_prefix)
            };
            (dir, String::new(), info.raw_prefix.clone())
        } else {
            let path = Path::new(&expanded_prefix);
            let dir = path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let file = path.file_name().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();

            let full_dir = if dir.starts_with('/') || dir.starts_with("~") {
                dir.clone()
            } else {
                format!("{}/{}", self.base_path, dir)
            };
            (full_dir, file, info.raw_prefix.clone())
        };

        let read_dir_result = match std::fs::read_dir(&search_dir) {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        let mut suggestions: Vec<AutocompleteItem> = Vec::new();

        for entry in read_dir_result.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_lowercase().starts_with(&search_prefix.to_lowercase()) {
                continue;
            }

            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);

            let relative_path = self.build_relative_path(&info.raw_prefix, &name, is_dir, &display_base);
            let complete_path = if is_dir {
                format!("{}/", relative_path)
            } else {
                relative_path
            };
            let value = build_completion_value(&complete_path, is_dir, info.is_at_prefix, info.is_quoted_prefix);

            suggestions.push(AutocompleteItem {
                value,
                label: if is_dir { format!("{}/", name) } else { name },
                description: None,
            });
        }

        suggestions.sort_by(|a, b| {
            let a_dir = a.value.ends_with('/');
            let b_dir = b.value.ends_with('/');
            if a_dir && !b_dir { return std::cmp::Ordering::Less; }
            if !a_dir && b_dir { return std::cmp::Ordering::Greater; }
            a.label.cmp(&b.label)
        });

        suggestions
    }

    fn build_relative_path(&self, raw_prefix: &str, name: &str, is_dir: bool, display_base: &str) -> String {
        let normalized = to_display_path(raw_prefix);

        if normalized.ends_with('/') {
            return format!("{}{}", normalized, name);
        }

        if normalized.contains('/') || normalized.contains('\\') {
            if normalized.starts_with("~/") {
                let home_relative = &normalized[2..];
                let parent = Path::new(home_relative).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                let dir = if parent == "." || parent.is_empty() {
                    format!("~/{}", name)
                } else {
                    format!("~/{}/{}", parent, name)
                };
                return to_display_path(&dir);
            }
            if normalized.starts_with('/') {
                let parent = Path::new(&normalized).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                return to_display_path(&format!("{}/{}", parent, name));
            }
            let parent = Path::new(&normalized).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let result = if normalized.starts_with("./") && !parent.starts_with("./") {
                format!("./{}/{}", parent, name)
            } else {
                format!("{}/{}", parent, name)
            };
            return to_display_path(&result);
        }

        if normalized.starts_with('~') {
            return to_display_path(&format!("~/{}", name));
        }

        name.to_string()
    }
}

impl AutocompleteProvider for CombinedAutocompleteProvider {
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions> {
        let current_line = lines.get(cursor_line)?;
        let text_before_cursor = &current_line[..cursor_col];

        if let Some(at_prefix) = self.extract_at_prefix(text_before_cursor) {
            let info = parse_path_prefix(&at_prefix);
            let suggestions = self.get_file_suggestions(&at_prefix);
            if suggestions.is_empty() {
                return None;
            }
            return Some(AutocompleteSuggestions {
                items: suggestions,
                prefix: at_prefix,
            });
        }

        if !force && text_before_cursor.starts_with('/') {
            let space_idx = text_before_cursor.find(' ');

            match space_idx {
                None => {
                    let prefix = text_before_cursor[1..].to_string();
                    let command_items: Vec<AutocompleteItem> = self.commands.iter().map(|cmd| {
                        let name = cmd.name();
                        let hint = cmd.argument_hint();
                        let desc = cmd.description();
                        let full_desc = match (hint, desc) {
                            (Some(h), Some(d)) => format!("{} — {}", h, d),
                            (Some(h), None) => h.to_string(),
                            (None, Some(d)) => d.to_string(),
                            (None, None) => String::new(),
                        };
                        AutocompleteItem {
                            value: name.to_string(),
                            label: name.to_string(),
                            description: if full_desc.is_empty() { None } else { Some(full_desc) },
                        }
                    }).collect();

                    let filtered = fuzzy_filter(&command_items, &prefix, |item| &item.label);
                    if filtered.is_empty() {
                        return None;
                    }
                    return Some(AutocompleteSuggestions {
                        items: filtered.into_iter().cloned().collect(),
                        prefix: text_before_cursor.to_string(),
                    });
                }
                Some(sp) => {
                    let command_name = &text_before_cursor[1..sp];
                    let argument_text = &text_before_cursor[sp + 1..];

                    let command = self.commands.iter().find(|cmd| cmd.name() == command_name)?;
                    let argument_suggestions = command.get_argument_completions(argument_text)?;
                    if argument_suggestions.is_empty() {
                        return None;
                    }
                    return Some(AutocompleteSuggestions {
                        items: argument_suggestions,
                        prefix: argument_text.to_string(),
                    });
                }
            }
        }

        let path_match = self.extract_path_prefix(text_before_cursor, force);
        if let Some(prefix) = path_match {
            let suggestions = self.get_file_suggestions(&prefix);
            if suggestions.is_empty() {
                return None;
            }
            return Some(AutocompleteSuggestions {
                items: suggestions,
                prefix,
            });
        }

        None
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> ApplyResult {
        let current_line = lines.get(cursor_line).cloned().unwrap_or_default();
        let before_prefix = &current_line[..cursor_col.saturating_sub(prefix.len())];
        let after_cursor = &current_line[cursor_col..];

        let is_quoted_prefix = prefix.starts_with('"') || prefix.starts_with("@\"");
        let has_leading_quote_after = after_cursor.starts_with('"');
        let has_trailing_quote_in_item = item.value.ends_with('"');

        let adjusted_after = if is_quoted_prefix && has_trailing_quote_in_item && has_leading_quote_after {
            &after_cursor[1..]
        } else {
            after_cursor
        };

        let is_slash_command = prefix.starts_with('/') && before_prefix.trim().is_empty() && !prefix[1..].contains('/');

        if is_slash_command {
            let trailing = if adjusted_after.is_empty() { " " } else { "" };
            let new_line = format!("{}/{}{}{}", before_prefix, item.value, trailing, adjusted_after);
            let mut new_lines = lines.to_vec();
            new_lines[cursor_line] = new_line;
            return ApplyResult {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.len() + item.value.len() + 2,
            };
        }

        if prefix.starts_with('@') {
            let is_directory = item.label.ends_with('/');
            let suffix = if is_directory { "" } else { " " };
            let new_line = format!("{}{}{}{}", before_prefix, item.value, suffix, adjusted_after);
            let mut new_lines = lines.to_vec();
            new_lines[cursor_line] = new_line;

            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.len() - 1
            } else {
                item.value.len()
            };

            return ApplyResult {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.len() + cursor_offset + suffix.len(),
            };
        }

        let text_before_cursor = &current_line[..cursor_col];
        if text_before_cursor.contains('/') && text_before_cursor.contains(' ') {
            let new_line = format!("{}{}{}", before_prefix, item.value, adjusted_after);
            let mut new_lines = lines.to_vec();
            new_lines[cursor_line] = new_line;

            let is_directory = item.label.ends_with('/');
            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.len() - 1
            } else {
                item.value.len()
            };

            return ApplyResult {
                lines: new_lines,
                cursor_line,
                cursor_col: before_prefix.len() + cursor_offset,
            };
        }

        let new_line = format!("{}{}{}", before_prefix, item.value, adjusted_after);
        let mut new_lines = lines.to_vec();
        new_lines[cursor_line] = new_line;

        let is_directory = item.label.ends_with('/');
        let has_trailing_quote = item.value.ends_with('"');
        let cursor_offset = if is_directory && has_trailing_quote {
            item.value.len() - 1
        } else {
            item.value.len()
        };

        ApplyResult {
            lines: new_lines,
            cursor_line,
            cursor_col: before_prefix.len() + cursor_offset,
        }
    }

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let current_line = match lines.get(cursor_line) {
            Some(l) => l,
            None => return false,
        };
        let text_before_cursor = &current_line[..cursor_col];
        let trimmed = text_before_cursor.trim_start();
        !(trimmed.starts_with('/') && !trimmed.contains(' '))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCommand {
        name: String,
        desc: Option<String>,
        hint: Option<String>,
    }

    impl SlashCommandLike for TestCommand {
        fn name(&self) -> &str { &self.name }
        fn description(&self) -> Option<&str> { self.desc.as_deref() }
        fn argument_hint(&self) -> Option<&str> { self.hint.as_deref() }
        fn get_argument_completions(&self, _argument_prefix: &str) -> Option<Vec<AutocompleteItem>> { None }
    }

    fn test_provider() -> CombinedAutocompleteProvider {
        CombinedAutocompleteProvider::new(Vec::new(), "/tmp".to_string())
    }

    #[test]
    fn test_parse_path_prefix() {
        let info = parse_path_prefix("@/path/to/file");
        assert!(info.is_at_prefix);
        assert!(!info.is_quoted_prefix);
    }

    #[test]
    fn test_find_last_delimiter() {
        assert_eq!(find_last_delimiter("hello world"), Some(5));
        assert_eq!(find_last_delimiter("hello"), None);
        assert_eq!(find_last_delimiter(""), None);
    }

    #[test]
    fn test_find_unclosed_quote_start() {
        assert_eq!(find_unclosed_quote_start("hello \""), Some(6));
        assert_eq!(find_unclosed_quote_start("\"hello\""), None);
        assert_eq!(find_unclosed_quote_start("no quotes"), None);
    }

    #[test]
    fn test_extract_quoted_prefix() {
        assert_eq!(extract_quoted_prefix("hello \""), Some("\"".to_string()));
        assert_eq!(extract_quoted_prefix("\"closed\""), None);
    }

    #[test]
    fn test_build_completion_value() {
        assert_eq!(build_completion_value("file.txt", false, false, false), "file.txt");
        assert_eq!(build_completion_value("dir/", true, true, false), "@/dir/");
        assert_eq!(build_completion_value("my file.txt", false, false, true), "\"my file.txt\"");
    }

    #[test]
    fn test_expand_home_path() {
        let provider = test_provider();
        let home = std::env::var("HOME").unwrap_or_default();
        let expanded = provider.expand_home_path("~/test");
        assert!(expanded.starts_with(&home));
        assert!(expanded.ends_with("/test"));
    }

    #[test]
    fn test_extract_at_prefix() {
        let provider = test_provider();
        assert!(provider.extract_at_prefix("no prefix").is_none());
        let result = provider.extract_at_prefix("hello @file");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "@file");
    }

    #[test]
    fn test_extract_path_prefix() {
        let provider = test_provider();
        let result = provider.extract_path_prefix("open src/file.txt", false);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.contains("src/file.txt") || r.contains("file.txt"));
    }

    #[test]
    fn test_apply_completion_slash_command() {
        let provider = test_provider();
        let lines = vec!["/hel".to_string()];
        let item = AutocompleteItem {
            value: "help".to_string(),
            label: "help".to_string(),
            description: None,
        };

        let result = provider.apply_completion(&lines, 0, 4, &item, "/hel");
        assert_eq!(result.lines[0], "/help ");
        assert!(result.cursor_col > 4);
    }

    #[test]
    fn test_is_token_start() {
        assert!(is_token_start("hello", 0));
        assert!(is_token_start("hello world", 6));
        assert!(!is_token_start("hello", 3));
    }
}
