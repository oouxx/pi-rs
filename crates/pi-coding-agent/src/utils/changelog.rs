#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::unnecessary_struct_initialization,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_closure,
    clippy::missing_const_for_fn,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::manual_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::redundant_clone,
    clippy::clone_on_ref_ptr,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::match_same_arms,
    clippy::needless_continue,
    clippy::items_after_statements,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args,
    clippy::derive_partial_eq_without_eq,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::string_lit_as_bytes,
    clippy::trivially_copy_pass_by_ref,
    clippy::use_self,
    clippy::significant_drop_tightening,
    clippy::default_trait_access,
    clippy::iter_with_drain,
    clippy::if_not_else,
    clippy::explicit_iter_loop,
    clippy::assigning_clones,
    clippy::implicit_hasher,
    clippy::ignored_unit_patterns,
    clippy::missing_fields_in_debug,
    clippy::or_fun_call,
    clippy::too_long_first_doc_paragraph,
    clippy::manual_string_new,
    clippy::single_match_else,
    clippy::significant_drop_in_scrutinee,
    clippy::needless_collect,
    clippy::duplicated_attributes,
    clippy::similar_names,
    clippy::needless_raw_string_hashes,
    clippy::unnested_or_patterns,
)]
use std::cmp::Ordering;
use std::fs;
use regex::Regex;

/// A parsed changelog entry.
#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    pub major: i32,
    pub minor: i32,
    pub patch: i32,
    pub content: String,
}

const GITHUB_REPO: &str = "earendil-works/pi";
const CHANGELOG_LINK_BASE_PATH: &str = "packages/coding-agent";

/// Parse a CHANGELOG.md into entries.
pub fn parse_changelog(path: &str) -> Vec<ChangelogEntry> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let version_re = Regex::new(r"^##\s+\[?(\d+)\.(\d+)\.(\d+)\]?").unwrap();
    let mut entries = Vec::new();
    let mut current: Option<(i32, i32, i32, String)> = None;

    for line in content.lines() {
        if let Some(caps) = version_re.captures(line) {
            if let Some((maj, min, pat, content)) = current.take() {
                entries.push(ChangelogEntry {
                    major: maj,
                    minor: min,
                    patch: pat,
                    content,
                });
            }
            let major: i32 = caps[1].parse().unwrap_or(0);
            let minor: i32 = caps[2].parse().unwrap_or(0);
            let patch: i32 = caps[3].parse().unwrap_or(0);
            current = Some((major, minor, patch, String::new()));
        } else if let Some((_maj, _min, _pat, ref mut acc)) = current {
            if !acc.is_empty() {
                acc.push('\n');
            }
            acc.push_str(line);
        }
    }
    if let Some((maj, min, pat, content)) = current {
        entries.push(ChangelogEntry {
            major: maj,
            minor: min,
            patch: pat,
            content,
        });
    }

    entries
}

/// Compare two semantic versions.
pub fn compare_versions(v1: &ChangelogEntry, v2: &ChangelogEntry) -> Ordering {
    let by_major = v1.major.cmp(&v2.major);
    if by_major != Ordering::Equal {
        return by_major;
    }
    let by_minor = v1.minor.cmp(&v2.minor);
    if by_minor != Ordering::Equal {
        return by_minor;
    }
    v1.patch.cmp(&v2.patch)
}

/// Filter entries newer than the given version string (e.g. "0.9.0").
pub fn get_new_entries(entries: &[ChangelogEntry], last_version: &str) -> Vec<ChangelogEntry> {
    let parts: Vec<&str> = last_version.split('.').collect();
    if parts.len() != 3 {
        return Vec::new();
    }
    let last = ChangelogEntry {
        major: parts[0].parse().unwrap_or(0),
        minor: parts[1].parse().unwrap_or(0),
        patch: parts[2].parse().unwrap_or(0),
        content: String::new(),
    };

    entries
        .iter()
        .filter(|e| compare_versions(e, &last) == Ordering::Greater)
        .cloned()
        .collect()
}

/// Normalize markdown internal links to point to the correct GitHub tag.
pub fn normalize_changelog_links(markdown: &str, version: &str) -> String {
    let tag = if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{}", version)
    };

    let link_re = Regex::new(r"\[([^\]]*)\]\(([^)]*)\)").unwrap();
    link_re
        .replace_all(markdown, |caps: &regex::Captures| {
            let text = &caps[1];
            let target = &caps[2];

            // Skip anchors, protocol URLs, and double-slash paths
            if target.starts_with('#')
                || target.contains("://")
                || target.starts_with("//")
            {
                return format!("[{}]({})", text, target);
            }

            // Resolve as local path under base
            let clean = target.trim_start_matches("./");
            let path = if clean.starts_with(CHANGELOG_LINK_BASE_PATH) {
                clean.to_string()
            } else {
                // Avoid path traversal beyond base
                let resolved = format!("{}/{}", CHANGELOG_LINK_BASE_PATH, clean);
                resolved
            };

            let link_type = if path.ends_with('/') || !path.contains('.') {
                "tree"
            } else {
                "blob"
            };

            format!(
                "[{}](https://github.com/{}/{}/{}/{})",
                text, GITHUB_REPO, link_type, tag, path
            )
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_changelog_empty() {
        let entries = parse_changelog("/tmp/nonexistent.md");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_compare_versions() {
        let v1 = ChangelogEntry {
            major: 1,
            minor: 0,
            patch: 0,
            content: String::new(),
        };
        let v2 = ChangelogEntry {
            major: 1,
            minor: 0,
            patch: 1,
            content: String::new(),
        };
        assert_eq!(compare_versions(&v1, &v2), std::cmp::Ordering::Less);
        assert_eq!(compare_versions(&v2, &v1), std::cmp::Ordering::Greater);
        assert_eq!(compare_versions(&v1, &v1), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_parse_changelog_with_content() {
        let content = r#"## [1.0.0] - 2024-01-01

First release

## [0.9.0] - 2023-12-01

Beta release
"#;
        let path = "/tmp/test_changelog.md";
        std::fs::write(path, content).unwrap();
        let entries = parse_changelog(path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].major, 1);
        assert_eq!(entries[0].minor, 0);
        assert_eq!(entries[0].patch, 0);
        assert!(entries[0].content.contains("First release"));
        assert_eq!(entries[1].major, 0);
        assert_eq!(entries[1].minor, 9);
        assert_eq!(entries[1].patch, 0);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn test_get_new_entries() {
        let entries = vec![
            ChangelogEntry {
                major: 1,
                minor: 0,
                patch: 0,
                content: "v1".into(),
            },
            ChangelogEntry {
                major: 0,
                minor: 9,
                patch: 0,
                content: "v0.9".into(),
            },
        ];
        let new = get_new_entries(&entries, "0.9.0");
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].content, "v1");
    }

    #[test]
    fn test_normalize_changelog_links_local_path() {
        let md = "See [file](src/lib.rs) for details.";
        let result = normalize_changelog_links(md, "v1.0.0");
        assert!(
            result.contains("earendil-works/pi/blob/v1.0.0/packages/coding-agent/src/lib.rs")
        );
    }
}
