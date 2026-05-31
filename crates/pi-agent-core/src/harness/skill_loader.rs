use crate::harness::types::Skill;
use crate::harness::types::ExecutionEnv;

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 256;

#[derive(Debug, Clone)]
pub struct SkillDiagnostic {
    pub diagnostic_type: String,
    pub code: String,
    pub message: String,
    pub path: String,
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

    if info.kind != "directory" {
        return (skills, diagnostics);
    }

    let entries = match env.list_dir(directory).await {
        Ok(entries) => entries,
        Err(e) => {
            diagnostics.push(SkillDiagnostic {
                diagnostic_type: "warning".to_string(),
                code: "list_dir_failed".to_string(),
                message: e.message,
                path: directory.to_string(),
            });
            return (skills, diagnostics);
        }
    };

    for entry in entries {
        if entry.kind == "directory" {
            let skill_file = format!("{}/SKILL.md", entry.path);
            let result = load_skill_from_file(env, &skill_file).await;
            diagnostics.extend(result.diagnostics);
            if let Some(skill) = result.skill {
                skills.push(skill);
            }
        } else if entry.name == "SKILL.md" {
            let result = load_skill_from_file(env, &entry.path).await;
            diagnostics.extend(result.diagnostics);
            if let Some(skill) = result.skill {
                skills.push(skill);
            }
        }
    }

    (skills, diagnostics)
}

struct SkillLoadResult {
    skill: Option<Skill>,
    diagnostics: Vec<SkillDiagnostic>,
}

async fn load_skill_from_file(env: &dyn ExecutionEnv, file_path: &str) -> SkillLoadResult {
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

    let name = frontmatter_name.unwrap_or_else(|| parent_dir_name.clone());

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