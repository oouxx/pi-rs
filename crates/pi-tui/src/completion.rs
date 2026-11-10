//! File/path completion — port of TS `packages/tui/src/autocomplete.ts`
//! (CombinedAutocompleteProvider's path + fuzzy-file branches).
//!
//! The TS original shells out to `fd` (fast, gitignore-aware). Rust uses the
//! `ignore` crate's gitignore-aware walker instead — same semantics for the
//! cases the TUI needs (files + dirs, hidden, follow links, `.git` excluded).

use std::path::{Path, PathBuf};

use crate::components::CompletionItem;

const PATH_DELIMITERS: [char; 5] = [' ', '\t', '"', '\'', '='];

/// Whether a character separates prose from a completion token.
///
/// Mirrors TS `autocompleteSeparatorRegex` exactly: `\s`, CJK-script
/// punctuation (Han/Hiragana/Katakana/Hangul/Bopomofo `Script_Extensions`
/// punctuation), plus the explicitly listed CJK marks. CJK **letters** and
/// fullwidth alphanumerics stay inside words and paths, so typing `查看，@REA`
/// triggers a completion while `Ａ` (U+FF21) does not break the token.
pub fn is_separator(c: char) -> bool {
    if c.is_whitespace() || c == '\u{FEFF}' {
        return true;
    }
    matches!(
        c,
        '\u{B7}'
            | '\u{2014}'
            | '\u{2018}'
            | '\u{2019}'
            | '\u{201C}'
            | '\u{201D}'
            | '\u{2026}'
            | '\u{3001}'..='\u{3003}'
            | '\u{3008}'..='\u{3011}'
            | '\u{3014}'..='\u{301F}'
            | '\u{3030}'
            | '\u{303D}'
            | '\u{30A0}'
            | '\u{30FB}'
            | '\u{FE45}'
            | '\u{FE46}'
            | '\u{FF01}'
            | '\u{FF08}'
            | '\u{FF09}'
            | '\u{FF0C}'
            | '\u{FF0E}'
            | '\u{FF1A}'
            | '\u{FF1B}'
            | '\u{FF1F}'
            | '\u{FF3B}'
            | '\u{FF3D}'
            | '\u{FF5B}'
            | '\u{FF5D}'
            | '\u{FF61}'..='\u{FF65}'
            | '\u{16FE2}'
    )
}

/// TS `toDisplayPath`: normalize separators to `/`.
pub fn to_display_path(value: &str) -> String {
    value.replace('\\', "/")
}

/// TS `findLastDelimiter`: byte offset **after** the last delimiter (an ASCII
/// path delimiter or a [`is_separator`] character).
///
/// TS returns `index - 1` where `index` is the UTF-16 position after the
/// delimiter, so callers slice `text.slice(index)`. Returning the byte offset
/// after the delimiter gives Rust callers the same "slice from here" contract
/// and is always a valid char boundary, even for multi-byte separators.
pub fn find_last_delimiter(text: &str) -> Option<usize> {
    let mut last = None;
    for (index, c) in text.char_indices() {
        if PATH_DELIMITERS.contains(&c) || is_separator(c) {
            last = Some(index + c.len_utf8());
        }
    }
    last
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
    if in_quotes {
        quote_start
    } else {
        None
    }
}

fn is_token_start(text: &str, index: usize) -> bool {
    index == 0
        || text[..index].ends_with(PATH_DELIMITERS)
        || text[..index].chars().next_back().is_some_and(is_separator)
}

/// TS `extractQuotedPrefix`.
pub fn extract_quoted_prefix(text: &str) -> Option<String> {
    let quote_start = find_unclosed_quote_start(text)?;
    if quote_start > 0 && text.as_bytes()[quote_start - 1] == b'@' {
        if !is_token_start(text, quote_start - 1) {
            return None;
        }
        return Some(text[quote_start - 1..].to_string());
    }
    if !is_token_start(text, quote_start) {
        return None;
    }
    Some(text[quote_start..].to_string())
}

/// TS `parsePathPrefix`.
pub fn parse_path_prefix(prefix: &str) -> (String, bool, bool) {
    if let Some(rest) = prefix.strip_prefix("@\"") {
        (rest.to_string(), true, true)
    } else if let Some(rest) = prefix.strip_prefix('"') {
        (rest.to_string(), false, true)
    } else if let Some(rest) = prefix.strip_prefix('@') {
        (rest.to_string(), true, false)
    } else {
        (prefix.to_string(), false, false)
    }
}

/// TS `buildCompletionValue`.
pub fn build_completion_value(
    path: &str,
    is_directory: bool,
    is_at_prefix: bool,
    is_quoted_prefix: bool,
) -> String {
    let _ = is_directory; // 路径本身已带尾随 `/`，无需再拼
    let needs_quotes = is_quoted_prefix || path.chars().any(is_separator);
    let prefix = if is_at_prefix { "@" } else { "" };
    if !needs_quotes {
        format!("{prefix}{path}")
    } else {
        format!("{prefix}\"{path}\"")
    }
}

/// TS `expandHomePath` (`~` / `~/...`).
fn expand_home_path(path: &str) -> String {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    if let Some(rest) = path.strip_prefix("~/") {
        let expanded = home.join(rest);
        if path.ends_with('/') && !expanded.to_string_lossy().ends_with('/') {
            return format!("{}/", expanded.to_string_lossy());
        }
        expanded.to_string_lossy().to_string()
    } else if path == "~" {
        home.to_string_lossy().to_string()
    } else {
        path.to_string()
    }
}

fn dirname_display(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => ".",
    }
}

/// Number of non-empty path segments (TS `toDisplayPath(path).split("/")`
/// `.filter(Boolean).length`).
fn path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

fn basename_display(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// TS `getFileSuggestions`（readdir 版）：按本地目录列出前缀匹配项，
/// 目录优先 + 字母序；保留 `~`/`./`/绝对路径/引号语义。
pub fn file_suggestions(cwd: &str, prefix: &str) -> Vec<CompletionItem> {
    let (raw_prefix, is_at_prefix, is_quoted_prefix) = parse_path_prefix(prefix);
    let expanded_prefix = if raw_prefix.starts_with('~') {
        expand_home_path(&raw_prefix)
    } else {
        raw_prefix.to_string()
    };

    let is_root_prefix = raw_prefix.is_empty()
        || raw_prefix == "./"
        || raw_prefix == "../"
        || raw_prefix == "~"
        || raw_prefix == "~/"
        || raw_prefix == "/"
        || (is_at_prefix && raw_prefix.is_empty());

    // 根前缀（"" / "./" / "~" 等）与目录前缀（以 "/" 结尾）都是"列出目录
    // 内容"，处理相同（TS 两分支代码一致）。
    let (search_dir, search_prefix) = if is_root_prefix || raw_prefix.ends_with('/') {
        if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
            (PathBuf::from(expanded_prefix), String::new())
        } else {
            (Path::new(cwd).join(&expanded_prefix), String::new())
        }
    } else {
        let dir = dirname_display(&expanded_prefix);
        let file = basename_display(&expanded_prefix);
        if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
            (PathBuf::from(dir), file.to_string())
        } else {
            (Path::new(cwd).join(dir), file.to_string())
        }
    };

    let Ok(entries) = std::fs::read_dir(&search_dir) else {
        return Vec::new();
    };
    let lower_prefix = search_prefix.to_lowercase();
    let mut suggestions: Vec<CompletionItem> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.to_lowercase().starts_with(&lower_prefix) {
            continue;
        }
        let mut is_directory = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_directory && entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            if let Ok(md) = entry.path().metadata() {
                is_directory = md.is_dir();
            }
        }
        let display_prefix = &raw_prefix;
        let relative_path = if display_prefix.ends_with('/') {
            format!("{display_prefix}{name}")
        } else if display_prefix.contains('/') || display_prefix.contains('\\') {
            if let Some(rest) = display_prefix.strip_prefix("~/") {
                let dir = dirname_display(rest);
                let joined = if dir == "." {
                    name.clone()
                } else {
                    format!("{dir}/{name}")
                };
                format!("~/{joined}")
            } else if let Some(abs) = display_prefix.strip_prefix('/') {
                let dir = dirname_display(abs);
                if dir == "/" {
                    format!("/{name}")
                } else {
                    format!("{dir}/{name}")
                }
            } else {
                let joined = {
                    let dir = dirname_display(display_prefix);
                    if dir == "." {
                        name.clone()
                    } else {
                        format!("{dir}/{name}")
                    }
                };
                if display_prefix.starts_with("./") && !joined.starts_with("./") {
                    format!("./{joined}")
                } else {
                    joined
                }
            }
        } else if display_prefix.starts_with('~') {
            format!("~/{name}")
        } else {
            name.clone()
        };

        let relative_path = to_display_path(&relative_path);
        let path_value = if is_directory {
            format!("{relative_path}/")
        } else {
            relative_path
        };
        let value =
            build_completion_value(&path_value, is_directory, is_at_prefix, is_quoted_prefix);
        suggestions.push(CompletionItem {
            value,
            label: if is_directory {
                format!("{name}/")
            } else {
                name
            },
            description: String::new(),
        });
    }

    // 目录优先，再按 label 字母序（TS getFileSuggestions 排序）。
    suggestions.sort_by(|a, b| {
        let a_dir = a.value.ends_with('/');
        let b_dir = b.value.ends_with('/');
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.label.cmp(&b.label),
        }
    });
    suggestions
}

#[cfg(test)]
mod cjk_separator_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// CJK punctuation ends a token, so a completion triggers after it, while
    /// CJK letters stay inside the token (TS #9746).
    #[test]
    fn cjk_punctuation_separates_tokens() {
        let text = "查看，@REA";
        let last = find_last_delimiter(text).expect("the ，is a delimiter");
        assert_eq!(&text[last..], "@REA");
        assert!(is_token_start(text, last));

        // The returned offset is after the separator and always a char
        // boundary, so `&text[offset..]` cannot panic on a multi-byte
        // separator (the old code added 1 to the separator's first byte).
        assert!(text.is_char_boundary(last));
        assert_eq!(&text[last..], "@REA");

        // A CJK letter is not a separator.
        assert!(!is_separator('查'));
        assert!(!is_separator('あ'));
        assert!(!is_separator('/'));
        assert!(is_separator('，'));
        assert!(is_separator('。'));
        assert!(is_separator('（'));
        assert!(is_separator('…'));
        assert!(is_separator(' '));
        // Fullwidth alphanumerics are letters, not punctuation (TS
        // `autocompleteSeparatorRegex` only matches CJK punctuation).
        assert!(!is_separator('Ａ'));
        assert!(!is_separator('１'));
        assert!(!is_separator('〆'));
        assert!(!is_separator('々'));
        // Halfwidth CJK punctuation does separate.
        assert!(is_separator('｡'));
        assert!(is_separator('･'));
        assert!(is_separator('、'));
    }

    /// A path containing a separator is quoted so the inserted text stays one
    /// token.
    #[test]
    fn separators_inside_a_path_force_quoting() {
        assert_eq!(
            build_completion_value("我的，文档/", true, true, false),
            "@\"我的，文档/\""
        );
        assert_eq!(
            build_completion_value("src/main.rs", false, true, false),
            "@src/main.rs"
        );
        assert_eq!(
            build_completion_value("my dir/", true, true, false),
            "@\"my dir/\""
        );
    }
}

/// TS `scoreEntry`：文件名精确 100 / 前缀 80 / 包含 50 / 全路径包含 30；
/// 目录 +10。
fn score_entry(file_path: &str, query: &str, is_directory: bool) -> i64 {
    let file_name = basename_display(file_path).to_lowercase();
    let lower_query = query.to_lowercase();
    let mut score = if file_name == lower_query {
        100
    } else if file_name.starts_with(&lower_query) {
        80
    } else if file_name.contains(&lower_query) {
        50
    } else if file_path.to_lowercase().contains(&lower_query) {
        30
    } else {
        0
    };
    if is_directory && score > 0 {
        score += 10;
    }
    score
}

/// TS `resolveScopedFuzzyQuery`：query 含 `/` 时把目录部分作为 fd base。
fn resolve_scoped_fuzzy_query(raw_query: &str, cwd: &str) -> Option<(PathBuf, String, String)> {
    let normalized = to_display_path(raw_query);
    let slash_index = normalized.rfind('/')?;
    let display_base = normalized[..=slash_index].to_string();
    let query = normalized[slash_index + 1..].to_string();

    let base_dir = if let Some(rest) = display_base.strip_prefix("~/") {
        PathBuf::from(expand_home_path(&format!("~/{rest}")))
    } else if display_base.starts_with('/') {
        PathBuf::from(&display_base)
    } else {
        Path::new(cwd).join(&display_base)
    };
    if !base_dir.is_dir() {
        return None;
    }
    Some((base_dir, query, display_base))
}

/// TS `scopedPathForDisplay`。
fn scoped_path_for_display(display_base: &str, relative_path: &str) -> String {
    let rel = to_display_path(relative_path);
    if display_base == "/" {
        format!("/{rel}")
    } else {
        format!("{}{rel}", to_display_path(display_base))
    }
}

/// TS `walkDirectoryWithFd` 的 Rust 等价（`ignore` 遍历）：
/// 文件+目录、hidden、follow、排除 `.git`、最多 `max_results` 条命中。
fn walk_with_ignore(base_dir: &Path, query: &str, max_results: usize) -> Vec<(String, bool)> {
    let mut results: Vec<(String, bool)> = Vec::new();
    let mut scanned = 0usize;
    let walker = ignore::WalkBuilder::new(base_dir)
        .hidden(true)
        .follow_links(true)
        .filter_entry(|e| e.file_name().to_string_lossy() != ".git")
        .build();
    for entry in walker.flatten() {
        scanned += 1;
        if scanned > 100_000 {
            break;
        }
        if results.len() >= max_results {
            break;
        }
        let path = entry.path();
        if path == base_dir {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let rel = path.strip_prefix(base_dir).unwrap_or(path);
        let display = to_display_path(&rel.to_string_lossy());
        let path_for_score = display.trim_end_matches('/');
        if query.is_empty() || score_entry(path_for_score, query, is_dir) > 0 {
            let value = if is_dir {
                format!("{display}/")
            } else {
                display
            };
            results.push((value, is_dir));
        }
    }
    results
}

/// TS `getFuzzyFileSuggestions`（`@` 附件补全）：fuzzy 走查 + 打分 + top 20。
pub fn fuzzy_file_suggestions(
    cwd: &str,
    query: &str,
    is_quoted_prefix: bool,
    is_at_prefix: bool,
    max_results: usize,
) -> Vec<CompletionItem> {
    let scoped = resolve_scoped_fuzzy_query(query, cwd);
    let (base_dir, fd_query, display_base) = match &scoped {
        Some((base, q, display)) => (base.clone(), q.clone(), Some(display.clone())),
        None => (PathBuf::from(cwd), query.to_string(), None),
    };

    let entries = walk_with_ignore(&base_dir, &fd_query, 100);
    let mut scored: Vec<(i64, String, bool)> = entries
        .into_iter()
        .filter_map(|(path, is_dir)| {
            let score = if fd_query.is_empty() {
                1
            } else {
                let p = if is_dir {
                    path.trim_end_matches('/')
                } else {
                    &path
                };
                score_entry(p, &fd_query, is_dir)
            };
            if score <= 0 {
                None
            } else {
                Some((score, path, is_dir))
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        // v0.87.1 tie-breaks: score desc, path depth asc, path length asc,
        // then localeCompare (byte order for the ASCII paths fd returns).
        b.0.cmp(&a.0)
            .then_with(|| path_depth(&a.1).cmp(&path_depth(&b.1)))
            .then_with(|| {
                crate::text_utils::utf16_len(&a.1).cmp(&crate::text_utils::utf16_len(&b.1))
            })
            .then_with(|| a.1.cmp(&b.1))
    });

    let mut suggestions = Vec::new();
    for (_, path, is_dir) in scored.into_iter().take(max_results.max(1)) {
        let path_without_slash = if is_dir {
            path.trim_end_matches('/').to_string()
        } else {
            path.clone()
        };
        let display_path = match &display_base {
            Some(base) => scoped_path_for_display(base, &path_without_slash),
            None => path_without_slash.clone(),
        };
        let entry_name = basename_display(&path_without_slash).to_string();
        let completion_path = if is_dir {
            format!("{display_path}/")
        } else {
            display_path.clone()
        };
        let value =
            build_completion_value(&completion_path, is_dir, is_at_prefix, is_quoted_prefix);
        suggestions.push(CompletionItem {
            value,
            label: if is_dir {
                format!("{entry_name}/")
            } else {
                entry_name
            },
            description: display_path,
        });
    }
    suggestions
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use super::*;

    #[test]
    fn parse_prefix_forms() {
        assert_eq!(parse_path_prefix("src/ma"), ("src/ma".into(), false, false));
        assert_eq!(parse_path_prefix("@src/ma"), ("src/ma".into(), true, false));
        assert_eq!(parse_path_prefix("@\"a b"), ("a b".into(), true, true));
    }

    #[test]
    fn build_value_quotes_when_needed() {
        assert_eq!(
            build_completion_value("src/main.rs", false, true, false),
            "@src/main.rs"
        );
        assert_eq!(
            build_completion_value("a b.txt", false, true, false),
            "@\"a b.txt\""
        );
        assert_eq!(build_completion_value("src/", true, false, false), "src/");
    }

    #[test]
    fn file_suggestions_lists_local_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").expect("write");
        std::fs::write(dir.path().join("README.md"), "# readme").expect("write");
        let cwd = dir.path().to_string_lossy().to_string();

        // 列出根目录：目录优先（src/ 在 README.md 前）。
        let root_items = file_suggestions(&cwd, "");
        assert_eq!(
            root_items.first().map(|i| i.value.as_str()),
            Some("src/"),
            "目录优先: {root_items:?}"
        );
        // 带目录前缀：列出该目录内容。
        let items = file_suggestions(&cwd, "src/");
        assert!(
            items
                .iter()
                .any(|i| i.value == "src/main.rs" && i.label == "main.rs"),
            "got: {items:?}"
        );
    }

    #[test]
    fn fuzzy_file_suggestions_scores_and_prefixes_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/main.rs"), "").expect("write");
        std::fs::write(dir.path().join("src/mod.rs"), "").expect("write");
        let cwd = dir.path().to_string_lossy().to_string();

        let items = fuzzy_file_suggestions(&cwd, "main", false, true, 20);
        let main = items.iter().find(|i| i.value == "@src/main.rs");
        assert!(main.is_some(), "fuzzy @src/main.rs found: {items:?}");
    }

    #[test]
    fn fuzzy_file_suggestions_scopes_subdir_query() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
        std::fs::write(dir.path().join("src/app.rs"), "").expect("write");
        std::fs::write(dir.path().join("other.rs"), "").expect("write");
        let cwd = dir.path().to_string_lossy().to_string();

        let items = fuzzy_file_suggestions(&cwd, "src/app", false, true, 20);
        let app = items.iter().find(|i| i.value == "@src/app.rs");
        assert!(app.is_some(), "scoped src/app matched: {items:?}");
    }
}
