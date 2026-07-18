use crate::harness::types::{ExecutionEnv, FileInfoType, Skill};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: &[&str] = &[".gitignore", ".ignore", ".fdignore"];

#[derive(Debug, Clone)]
pub struct SkillDiagnostic {
    pub diagnostic_type: String,
    pub code: String,
    pub message: String,
    pub path: String,
}

/// A simple ignore matcher that checks if a path matches any ignore pattern.
/// Uses gitignore-style pattern matching.
#[derive(Debug, Clone, Default)]
struct IgnoreMatcher {
    patterns: Vec<String>,
}

impl IgnoreMatcher {
    fn new() -> Self {
        Self::default()
    }

    fn add_patterns(&mut self, patterns: &[String]) {
        self.patterns.extend(patterns.iter().cloned());
    }

    fn ignores(&self, path: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        for pattern in &self.patterns {
            if self.match_pattern(path, pattern) {
                return true;
            }
        }
        false
    }

    fn match_pattern(&self, path: &str, pattern: &str) -> bool {
        let trimmed = pattern.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return false;
        }
        let negated = trimmed.starts_with('!');
        let pat = if negated { &trimmed[1..] } else { trimmed };
        let pat = pat.strip_prefix('/').unwrap_or(pat);

        // Simple glob matching: check if path contains the pattern
        // or if the pattern matches the basename
        if pat.contains('*') {
            let prefix = pat.trim_end_matches('*');
            let suffix = pat.trim_start_matches('*');
            if pat.starts_with('*') && pat.ends_with('*') {
                path.contains(suffix)
            } else if pat.ends_with('*') {
                path.starts_with(prefix)
            } else if pat.starts_with('*') {
                path.ends_with(suffix)
            } else {
                path == pat
            }
        } else {
            path == pat || path.ends_with(&format!("/{}", pat))
        }
    }
}

pub async fn load_skills_from_directories(
    env: &dyn ExecutionEnv,
    directories: &[&str],
) -> (Vec<Skill>, Vec<SkillDiagnostic>) {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();

    for dir in directories {
        let (mut s, mut d) = load_skills_from_directory(env, dir).await;
        skills.append(&mut s);
        diagnostics.append(&mut d);
    }

    (skills, diagnostics)
}

async fn load_skills_from_directory(
    env: &dyn ExecutionEnv,
    directory: &str,
) -> (Vec<Skill>, Vec<SkillDiagnostic>) {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();

    let dir_info = env.file_info(directory).await;
    let info = match dir_info {
        Ok(info) => info,
        Err(_) => return (skills, diagnostics),
    };

    let resolved_kind = resolve_kind(env, &info, &mut diagnostics).await;
    if resolved_kind.as_deref() != Some("directory") {
        return (skills, diagnostics);
    }

    let mut ignore_matcher = IgnoreMatcher::new();
    add_ignore_rules(env, &mut ignore_matcher, directory, directory, &mut diagnostics).await;

    let entries = match env.list_dir(directory).await {
        Ok(entries) => entries,
        Err(e) => {
            diagnostics.push(SkillDiagnostic {
                diagnostic_type: "warning".to_string(),
                code: "list_failed".to_string(),
                message: e.message,
                path: directory.to_string(),
            });
            return (skills, diagnostics);
        }
    };

    // First pass: look for SKILL.md at the root level
    for entry in &entries {
        if entry.name != "SKILL.md" {
            continue;
        }
        let kind = resolve_kind(env, entry, &mut diagnostics).await;
        if kind.as_deref() != Some("file") {
            continue;
        }
        let rel_path = relative_path(directory, &entry.path);
        if ignore_matcher.ignores(&rel_path) {
            continue;
        }
        let result = load_skill_from_file(env, &entry.path, None).await;
        diagnostics.extend(result.diagnostics);
        if let Some(skill) = result.skill {
            skills.push(skill);
        }
        return (skills, diagnostics);
    }

    // Second pass: process all other entries
    let mut sorted_entries: Vec<_> = entries.iter().collect();
    sorted_entries.sort_by(|a, b| a.name.cmp(&b.name));

    for entry in sorted_entries {
        if entry.name.starts_with('.') || entry.name == "node_modules" {
            continue;
        }
        let full_path = &entry.path;
        let kind = resolve_kind(env, entry, &mut diagnostics).await;
        let kind = match kind {
            Some(k) => k,
            None => continue,
        };

        let rel_path = relative_path(directory, full_path);
        let ignore_path = if kind == "directory" {
            format!("{}/", rel_path)
        } else {
            rel_path.clone()
        };
        if ignore_matcher.ignores(&ignore_path) {
            continue;
        }

        if kind == "directory" {
            let result = load_skills_from_dir_internal(env, full_path, false, &mut ignore_matcher, directory, &mut diagnostics).await;
            skills.extend(result);
            continue;
        }

        if kind != "file" || !entry.name.ends_with(".md") {
            continue;
        }
        let result = load_skill_from_file(env, full_path, None).await;
        diagnostics.extend(result.diagnostics);
        if let Some(skill) = result.skill {
            skills.push(skill);
        }
    }

    (skills, diagnostics)
}

/// Load skills from a directory recursively, with ignore file support.
async fn load_skills_from_dir_internal(
    env: &dyn ExecutionEnv,
    dir: &str,
    include_root_files: bool,
    ignore_matcher: &mut IgnoreMatcher,
    root_dir: &str,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<Skill> {
    let mut skills = Vec::new();

    let dir_info = env.file_info(dir).await;
    let info = match dir_info {
        Ok(info) => info,
        Err(_) => return skills,
    };

    let resolved_kind = resolve_kind(env, &info, diagnostics).await;
    if resolved_kind.as_deref() != Some("directory") {
        return skills;
    }

    add_ignore_rules(env, ignore_matcher, dir, root_dir, diagnostics).await;

    let entries = match env.list_dir(dir).await {
        Ok(entries) => entries,
        Err(e) => {
            diagnostics.push(SkillDiagnostic {
                diagnostic_type: "warning".to_string(),
                code: "list_failed".to_string(),
                message: e.message,
                path: dir.to_string(),
            });
            return skills;
        }
    };

    for entry in &entries {
        if entry.name != "SKILL.md" {
            continue;
        }
        let kind = resolve_kind(env, entry, diagnostics).await;
        if kind.as_deref() != Some("file") {
            continue;
        }
        let rel_path = relative_path(root_dir, &entry.path);
        if ignore_matcher.ignores(&rel_path) {
            continue;
        }
        let result = load_skill_from_file(env, &entry.path, None).await;
        diagnostics.extend(result.diagnostics);
        if let Some(skill) = result.skill {
            skills.push(skill);
        }
        return skills;
    }

    let mut sorted_entries: Vec<_> = entries.iter().collect();
    sorted_entries.sort_by(|a, b| a.name.cmp(&b.name));

    for entry in sorted_entries {
        if entry.name.starts_with('.') || entry.name == "node_modules" {
            continue;
        }
        let full_path = &entry.path;
        let kind = resolve_kind(env, entry, diagnostics).await;
        let kind = match kind {
            Some(k) => k,
            None => continue,
        };

        let rel_path = relative_path(root_dir, full_path);
        let ignore_path = if kind == "directory" {
            format!("{}/", rel_path)
        } else {
            rel_path.clone()
        };
        if ignore_matcher.ignores(&ignore_path) {
            continue;
        }

        if kind == "directory" {
            let sub_skills = Box::pin(load_skills_from_dir_internal(env, full_path, false, ignore_matcher, root_dir, diagnostics)).await;
            skills.extend(sub_skills);
            continue;
        }

        if kind != "file" || !include_root_files || !entry.name.ends_with(".md") {
            continue;
        }
        let result = load_skill_from_file(env, full_path, None).await;
        diagnostics.extend(result.diagnostics);
        if let Some(skill) = result.skill {
            skills.push(skill);
        }
    }

    skills
}

/// Resolve the kind of a file system entry, following symlinks.
async fn resolve_kind(env: &dyn ExecutionEnv, info: &FileInfoType, diagnostics: &mut Vec<SkillDiagnostic>) -> Option<String> {
    if info.kind == "file" || info.kind == "directory" {
        return Some(info.kind.clone());
    }
    // Try to resolve symlink
    let canonical = env.canonical_path(&info.path).await;
    match canonical {
        Ok(path) => {
            let target = env.file_info(&path).await;
            match target {
                Ok(t) => {
                    if t.kind == "file" || t.kind == "directory" {
                        return Some(t.kind);
                    }
                    None
                }
                Err(e) => {
                    diagnostics.push(SkillDiagnostic {
                        diagnostic_type: "warning".into(),
                        code: "file_info_failed".into(),
                        message: format!("Failed to read file info: {}", e.message),
                        path: info.path.clone(),
                    });
                    None
                }
            }
        }
        Err(e) => {
            diagnostics.push(SkillDiagnostic {
                diagnostic_type: "warning".into(),
                code: "file_info_failed".into(),
                message: format!("Failed to resolve path: {}", e.message),
                path: info.path.clone(),
            });
            None
        }
    }
}

/// Add ignore rules from .gitignore, .ignore, and .fdignore files.
async fn add_ignore_rules(
    env: &dyn ExecutionEnv,
    ig: &mut IgnoreMatcher,
    dir: &str,
    root_dir: &str,
    diagnostics: &mut Vec<SkillDiagnostic>,
) {
    let relative_dir = relative_path(root_dir, dir);
    let prefix = if relative_dir.is_empty() {
        String::new()
    } else {
        format!("{}/", relative_dir)
    };

    for filename in IGNORE_FILE_NAMES {
        let ignore_path = format!("{}/{}", dir.trim_end_matches('/'), filename);
        let info = env.file_info(&ignore_path).await;
        match info {
            Ok(info) => {
                if info.kind != "file" {
                    continue;
                }
            }
            Err(e) => {
                if e.code.to_lowercase().contains("not_found") || e.code.to_lowercase().contains("not found") {
                    continue;
                }
                diagnostics.push(SkillDiagnostic {
                    diagnostic_type: "warning".into(),
                    code: "file_info_failed".into(),
                    message: format!("Failed to read ignore file info: {}", e.message),
                    path: ignore_path.clone(),
                });
                continue;
            }
        }
        let content = env.read_text_file(&ignore_path, None).await;
        match content {
            Ok(content) => {
                let patterns: Vec<String> = content
                    .lines()
                    .filter_map(|line| prefix_ignore_pattern(line, &prefix))
                    .collect();
                if !patterns.is_empty() {
                    ig.add_patterns(&patterns);
                }
            }
            Err(e) => {
                diagnostics.push(SkillDiagnostic {
                    diagnostic_type: "warning".into(),
                    code: "read_failed".into(),
                    message: format!("Failed to read ignore file: {}", e.message),
                    path: ignore_path.clone(),
                });
            }
        }
    }
}

/// Prefix an ignore pattern with the relative directory path.
fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }

    let mut pattern = line.to_string();
    let negated = if pattern.starts_with('!') {
        pattern = pattern[1..].to_string();
        true
    } else if pattern.starts_with("\\!") {
        pattern = pattern[1..].to_string();
        false
    } else {
        false
    };

    if pattern.starts_with('/') {
        pattern = pattern[1..].to_string();
    }

    let prefixed = if prefix.is_empty() {
        pattern
    } else {
        format!("{}{}", prefix, pattern)
    };

    if negated {
        Some(format!("!{}", prefixed))
    } else {
        Some(prefixed)
    }
}

/// Compute a relative path from root to target.
fn relative_path(root: &str, target: &str) -> String {
    let root = root.trim_end_matches('/');
    let target = target.trim_end_matches('/');
    if target == root {
        return String::new();
    }
    if target.starts_with(&format!("{}/", root)) {
        target[root.len() + 1..].to_string()
    } else {
        target.trim_start_matches('/').to_string()
    }
}

struct SkillLoadResult {
    skill: Option<Skill>,
    diagnostics: Vec<SkillDiagnostic>,
}

async fn load_skill_from_file(
    env: &dyn ExecutionEnv,
    file_path: &str,
    default_name: Option<&str>,
) -> SkillLoadResult {
    let mut diagnostics = Vec::new();

    let raw_content = match env.read_text_file(file_path, None).await {
        Ok(content) => content,
        Err(e) => {
            diagnostics.push(SkillDiagnostic {
                diagnostic_type: "warning".to_string(),
                code: "read_failed".to_string(),
                message: e.message,
                path: file_path.to_string(),
            });
            return SkillLoadResult {
                skill: None,
                diagnostics,
            };
        }
    };

    let (frontmatter, body) = parse_frontmatter(&raw_content);

    let skill_dir = dirname(file_path);
    let parent_dir_name = basename(&skill_dir);

    let description = frontmatter
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    for error in validate_description(description.as_deref()) {
        diagnostics.push(SkillDiagnostic {
            diagnostic_type: "warning".to_string(),
            code: "invalid_metadata".to_string(),
            message: error,
            path: file_path.to_string(),
        });
    }

    let frontmatter_name = frontmatter
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let name = frontmatter_name.unwrap_or_else(|| {
        default_name
            .map(|s| s.to_string())
            .unwrap_or(parent_dir_name.clone())
    });

    for error in validate_name(&name, &parent_dir_name) {
        diagnostics.push(SkillDiagnostic {
            diagnostic_type: "warning".to_string(),
            code: "invalid_metadata".to_string(),
            message: error,
            path: file_path.to_string(),
        });
    }

    if description.as_deref().map_or(true, |d| d.trim().is_empty()) {
        return SkillLoadResult {
            skill: None,
            diagnostics,
        };
    }

    let disable_model_invocation = frontmatter
        .get("disable-model-invocation")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    SkillLoadResult {
        skill: Some(Skill {
            name,
            description: description.unwrap_or_default(),
            content: body,
            file_path: file_path.to_string(),
            disable_model_invocation,
        }),
        diagnostics,
    }
}

fn validate_name(name: &str, parent_dir_name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name != parent_dir_name {
        errors.push(format!(
            "name \"{}\" does not match parent directory \"{}\"",
            name, parent_dir_name
        ));
    }
    if name.len() > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {} characters ({})",
            MAX_NAME_LENGTH,
            name.len()
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        errors.push(
            "name contains invalid characters (must be lowercase a-z 0-9 hyphens only)".to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}

fn validate_description(description: Option<&str>) -> Vec<String> {
    let mut errors = Vec::new();
    match description {
        None | Some("") => {
            errors.push("description is required".to_string());
        }
        Some(d) if d.len() > MAX_DESCRIPTION_LENGTH => {
            errors.push(format!(
                "description exceeds {} characters ({})",
                MAX_DESCRIPTION_LENGTH,
                d.len()
            ));
        }
        _ => {}
    }
    errors
}

fn parse_frontmatter(content: &str) -> (serde_json::Map<String, serde_json::Value>, String) {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return (serde_json::Map::new(), normalized);
    }
    let end_marker = "\n---";
    let end_index = match normalized[3..].find(end_marker) {
        Some(i) => i + 3,
        None => return (serde_json::Map::new(), normalized),
    };

    let yaml_string = &normalized[4..end_index];
    let body = normalized[end_index + 4..].trim().to_string();

    let frontmatter: serde_json::Map<String, serde_json::Value> =
        match serde_yaml::from_str(yaml_string) {
            Ok(v) => v,
            Err(_) => serde_json::Map::new(),
        };

    (frontmatter, body)
}

/// A sourced skill input: a directory path and its source identifier.
#[derive(Debug, Clone)]
pub struct SourcedSkillInput<S: Clone = String> {
    pub path: String,
    pub source: S,
}

/// A skill with its source attached.
#[derive(Debug, Clone)]
pub struct SourcedSkill<S: Clone = String> {
    pub skill: Skill,
    pub source: S,
}

/// A diagnostic with source information.
#[derive(Debug, Clone)]
pub struct SourcedSkillDiagnostic<S: Clone = String> {
    pub diagnostic: SkillDiagnostic,
    pub source: S,
}

/// Load skills from multiple source directories, tracking origin.
///
/// Each input specifies a `path` to scan and a `source` identifier (e.g., "user", "project", "builtin").
/// Returns both skill→source mappings and diagnostics→source mappings.
pub async fn load_sourced_skills<S: Clone>(
    env: &dyn ExecutionEnv,
    inputs: &[SourcedSkillInput<S>],
) -> (Vec<SourcedSkill<S>>, Vec<SourcedSkillDiagnostic<S>>) {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();

    for input in inputs {
        let (mut s, mut d) = load_skills_from_directory(env, &input.path).await;
        for skill in s.drain(..) {
            skills.push(SourcedSkill {
                skill,
                source: input.source.clone(),
            });
        }
        for diag in d.drain(..) {
            diagnostics.push(SourcedSkillDiagnostic {
                diagnostic: diag,
                source: input.source.clone(),
            });
        }
    }

    (skills, diagnostics)
}

fn dirname(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    match normalized.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => normalized[..i].to_string(),
        None => ".".to_string(),
    }
}

fn basename(path: &str) -> String {
    let normalized = path.trim_end_matches('/');
    match normalized.rfind('/') {
        Some(i) => normalized[i + 1..].to_string(),
        None => normalized.to_string(),
    }
}
