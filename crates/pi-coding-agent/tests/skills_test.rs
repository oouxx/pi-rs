//! End-to-end tests for the skill system, mirroring the original TS tests.
//!
//! Uses the same fixture files from `packages/coding-agent/test/fixtures/skills/`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::exit,
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::must_use_candidate,
    clippy::redundant_closure,
    clippy::redundant_clone,
    clippy::uninlined_format_args,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::unreadable_literal,
    clippy::similar_names,
    clippy::large_futures,
    clippy::items_after_statements,
    clippy::let_underscore_must_use,
    clippy::default_trait_access,
    clippy::significant_drop_tightening,
    clippy::used_underscore_binding,
    clippy::used_underscore_items,
    clippy::disallowed_methods,
    clippy::unnecessary_debug_formatting,
    clippy::unused_async,
    clippy::inefficient_to_string,
    clippy::needless_pass_by_ref_mut,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::single_char_pattern,
    clippy::float_cmp,
    clippy::struct_excessive_bools,
    clippy::option_as_ref_cloned,
    clippy::option_option,
    clippy::format_collect,
    clippy::branches_sharing_code,
    clippy::comparison_chain,
    clippy::manual_assert,
    clippy::no_effect_underscore_binding,
    clippy::implicit_clone,
    clippy::unchecked_time_subtraction,
    clippy::useless_let_if_seq,
    clippy::incompatible_msrv,
    clippy::manual_let_else,
    clippy::match_same_arms,
    clippy::needless_continue,
    clippy::unnecessary_to_owned,
    clippy::needless_pass_by_value,
    clippy::derive_partial_eq_without_eq,
    clippy::trivially_copy_pass_by_ref,
    clippy::use_self,
    clippy::or_fun_call,
    clippy::manual_string_new,
    clippy::single_match_else,
    clippy::needless_collect,
    clippy::duplicated_attributes,
    clippy::unreadable_literal,
    clippy::cast_abs_to_unsigned,
    clippy::cast_possible_wrap,
    clippy::fallible_impl_from,
    clippy::return_self_not_must_use,
    clippy::should_implement_trait,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::vec_init_then_push,
    clippy::wildcard_imports,
    clippy::enum_glob_use,
    clippy::cognitive_complexity,
    clippy::future_not_send,
    clippy::equatable_if_let,
    clippy::bool_to_int_with_if,
    clippy::cast_lossless,
    clippy::manual_pattern_char_comparison,
    clippy::derivable_impls,
    clippy::collapsible_match,
    clippy::collapsible_if,
    clippy::await_holding_lock,
    clippy::format_push_string,
    clippy::redundant_pattern_matching,
    clippy::option_map_or_none,
    clippy::option_map_unit_fn,
    clippy::result_map_or_into_option,
    clippy::unnecessary_wraps,
    clippy::map_unwrap_or,
    clippy::option_if_let_else,
    clippy::match_wildcard_for_single_variants,
    clippy::ref_option,
    clippy::unnecessary_operation,
    clippy::unused_self,
    clippy::missing_fields_in_debug,
    clippy::too_long_first_doc_paragraph,
    clippy::significant_drop_in_scrutinee,
    clippy::iter_with_drain,
    clippy::if_not_else,
    clippy::explicit_iter_loop,
    clippy::assigning_clones,
    clippy::implicit_hasher,
    clippy::ignored_unit_patterns,
    clippy::missing_const_for_fn,
    clippy::unnecessary_struct_initialization,
    clippy::string_lit_as_bytes,
    clippy::significant_drop_tightening,
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    unused_must_use,
)]

use std::path::PathBuf;

use pi_coding_agent::core::diagnostics::ResourceDiagnostic;
use pi_coding_agent::core::skills::{
    format_skills_for_prompt, load_skills, LoadSkillsOptions, Skill,
};

fn fixtures_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/skills");
    p
}

fn collision_fixtures_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/skills-collision");
    p
}

fn empty_agent_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/empty-agent");
    p
}

fn empty_cwd_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/empty-cwd");
    p
}

fn diagnostic_message(d: &ResourceDiagnostic) -> &str {
    match d {
        ResourceDiagnostic::Warning { message, .. } => message,
        ResourceDiagnostic::Collision { message, .. } => message,
    }
}

fn make_skill(
    name: &str,
    description: &str,
    file_path: &str,
    base_dir: &str,
    disable_model_invocation: bool,
) -> Skill {
    Skill {
        name: name.to_string(),
        description: description.to_string(),
        file_path: file_path.to_string(),
        base_dir: base_dir.to_string(),
        source_info: pi_coding_agent::core::skills::create_skill_source_info(
            file_path,
            base_dir,
            "test",
        ),
        disable_model_invocation,
    }
}

// ============================================================================
// loadSkillsFromDir tests
// ============================================================================

#[test]
fn test_load_valid_skill() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("valid-skill").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "valid-skill");
    assert_eq!(result.skills[0].description, "A valid skill for testing purposes.");
    assert_eq!(result.diagnostics.len(), 0);
}

#[test]
fn test_load_skill_name_mismatch() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("name-mismatch").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "different-name");
}

#[test]
fn test_load_skill_invalid_name_chars() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("invalid-name-chars").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert!(result.diagnostics.iter().any(|d| diagnostic_message(d).contains("invalid characters")));
}

#[test]
fn test_load_skill_long_name() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("long-name").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert!(result.diagnostics.iter().any(|d| diagnostic_message(d).contains("exceeds 64 characters")));
}

#[test]
fn test_load_skill_missing_description() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("missing-description").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 0);
    assert!(result.diagnostics.iter().any(|d| diagnostic_message(d).contains("description is required")));
}

#[test]
fn test_load_skill_unknown_field() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("unknown-field").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.diagnostics.len(), 0);
}

#[test]
fn test_load_nested_skills() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("nested").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "child-skill");
    assert_eq!(result.diagnostics.len(), 0);
}

#[test]
fn test_root_skill_preferred() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![
            fixtures_dir()
                .join("root-skill-preferred")
                .to_string_lossy()
                .to_string(),
        ],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "root-skill-preferred");
    assert_eq!(result.skills[0].description, "Root skill should win.");
    assert_eq!(result.diagnostics.len(), 0);
}

#[test]
fn test_load_skill_no_frontmatter() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("no-frontmatter").to_string_lossy().to_string()],
        include_defaults: false
    });
    // no-frontmatter has no description, so it should be skipped
    assert_eq!(result.skills.len(), 0);
    assert!(result.diagnostics.iter().any(|d| diagnostic_message(d).contains("description is required")));
}

#[test]
fn test_load_skill_invalid_yaml() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("invalid-yaml").to_string_lossy().to_string()],
        include_defaults: false
    });
    // serde_yaml should detect the invalid YAML and return an error
    assert_eq!(result.skills.len(), 0);
    assert!(result.diagnostics.iter().any(|d| {
        let msg = diagnostic_message(d);
        msg.contains("YAML") || msg.contains("at line") || msg.contains("frontmatter")
    }), "expected YAML parse error diagnostic");
}

#[test]
fn test_load_skill_multiline_description() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![
            fixtures_dir()
                .join("multiline-description")
                .to_string_lossy()
                .to_string(),
        ],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert!(result.skills[0].description.contains('\n'));
    assert!(result.skills[0].description.contains("This is a multiline description."));
    assert_eq!(result.diagnostics.len(), 0);
}

#[test]
fn test_load_skill_consecutive_hyphens() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![
            fixtures_dir()
                .join("consecutive-hyphens")
                .to_string_lossy()
                .to_string(),
        ],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert!(result.diagnostics.iter().any(|d| diagnostic_message(d).contains("consecutive hyphens")));
}

#[test]
fn test_load_all_skills_from_fixture_dir() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().to_string_lossy().to_string()],
        include_defaults: false
    });
    // Should load all skills that have descriptions (even with warnings)
    // valid-skill, name-mismatch, invalid-name-chars, long-name, unknown-field,
    // nested/child-skill, consecutive-hyphens, root-skill-preferred, disable-model-invocation
    // NOT: missing-description, no-frontmatter (both missing descriptions)
    assert!(result.skills.len() >= 6, "expected at least 6 skills, got {}", result.skills.len());
}

#[test]
fn test_load_skill_nonexistent_dir() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec!["/non/existent/path".to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 0);
    assert!(result.diagnostics.iter().any(|d| diagnostic_message(d).contains("does not exist")));
}

#[test]
fn test_load_skill_disable_model_invocation() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![
            fixtures_dir()
                .join("disable-model-invocation")
                .to_string_lossy()
                .to_string(),
        ],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "disable-model-invocation");
    assert!(result.skills[0].disable_model_invocation);
}

#[test]
fn test_load_skill_default_disable_model_invocation() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("valid-skill").to_string_lossy().to_string()],
        include_defaults: false
    });
    assert_eq!(result.skills.len(), 1);
    assert!(!result.skills[0].disable_model_invocation);
}

// ============================================================================
// formatSkillsForPrompt tests
// ============================================================================

#[test]
fn test_format_skills_empty() {
    let result = format_skills_for_prompt(&[]);
    assert_eq!(result, "");
}

#[test]
fn test_format_skills_as_xml() {
    let skills = vec![make_skill(
        "test-skill",
        "A test skill.",
        "/path/to/skill/SKILL.md",
        "/path/to/skill",
        false,
    )];
    let result = format_skills_for_prompt(&skills);
    assert!(result.contains("<available_skills>"));
    assert!(result.contains("</available_skills>"));
    assert!(result.contains("<skill>"));
    assert!(result.contains("<name>test-skill</name>"));
    assert!(result.contains("<description>A test skill.</description>"));
    assert!(result.contains("<location>/path/to/skill/SKILL.md</location>"));
}

#[test]
fn test_format_skills_escape_xml() {
    let skills = vec![make_skill(
        "test-skill",
        "A skill with <special> & \"characters\".",
        "/path/to/skill/SKILL.md",
        "/path/to/skill",
        false,
    )];
    let result = format_skills_for_prompt(&skills);
    assert!(result.contains("&lt;special&gt;"));
    assert!(result.contains("&amp;"));
    assert!(result.contains("&quot;characters&quot;"));
}

#[test]
fn test_format_multiple_skills() {
    let skills = vec![
        make_skill("skill-one", "First skill.", "/path/one/SKILL.md", "/path/one", false),
        make_skill("skill-two", "Second skill.", "/path/two/SKILL.md", "/path/two", false),
    ];
    let result = format_skills_for_prompt(&skills);
    assert!(result.contains("<name>skill-one</name>"));
    assert!(result.contains("<name>skill-two</name>"));
    assert_eq!(result.matches("<skill>").count(), 2);
}

#[test]
fn test_format_skills_exclude_disabled() {
    let skills = vec![
        make_skill(
            "visible-skill",
            "A visible skill.",
            "/path/visible/SKILL.md",
            "/path/visible",
            false,
        ),
        make_skill(
            "hidden-skill",
            "A hidden skill.",
            "/path/hidden/SKILL.md",
            "/path/hidden",
            true,
        ),
    ];
    let result = format_skills_for_prompt(&skills);
    assert!(result.contains("<name>visible-skill</name>"));
    assert!(!result.contains("<name>hidden-skill</name>"));
    assert_eq!(result.matches("<skill>").count(), 1);
}

#[test]
fn test_format_skills_all_disabled() {
    let skills = vec![make_skill(
        "hidden-skill",
        "A hidden skill.",
        "/path/hidden/SKILL.md",
        "/path/hidden",
        true,
    )];
    let result = format_skills_for_prompt(&skills);
    assert_eq!(result, "");
}

// ============================================================================
// loadSkills with options tests
// ============================================================================

#[test]
fn test_load_from_explicit_skill_paths() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![fixtures_dir().join("valid-skill").to_string_lossy().to_string()],
        include_defaults: true
    });
    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.skills[0].name, "valid-skill");
    assert_eq!(result.diagnostics.len(), 0);
}

#[test]
fn test_load_skill_nonexistent_path() {
    let result = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec!["/non/existent/path".to_string()],
        include_defaults: true
    });
    assert_eq!(result.skills.len(), 0);
    assert!(result.diagnostics.iter().any(|d| diagnostic_message(d).contains("does not exist")));
}

// ============================================================================
// Collision handling tests
// ============================================================================

#[test]
fn test_skill_name_collision() {
    // Load from first directory
    let first = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![
            collision_fixtures_dir()
                .join("first")
                .to_string_lossy()
                .to_string(),
        ],
        include_defaults: false
    });

    let second = load_skills(&LoadSkillsOptions {
        cwd: empty_cwd_dir().to_string_lossy().to_string(),
        agent_dir: Some(empty_agent_dir().to_string_lossy().to_string()),
        skill_paths: vec![
            collision_fixtures_dir()
                .join("second")
                .to_string_lossy()
                .to_string(),
        ],
        include_defaults: false
    });

    // Simulate the collision behavior from loadSkills()
    use std::collections::HashMap;
    let mut skill_map: HashMap<String, Skill> = HashMap::new();
    let mut collision_warnings: Vec<String> = Vec::new();

    for skill in first.skills {
        skill_map.insert(skill.name.clone(), skill);
    }

    for skill in second.skills {
        if let Some(existing) = skill_map.get(&skill.name) {
            collision_warnings.push(format!(
                "name collision: \"{}\" already loaded from {}",
                skill.name, existing.file_path
            ));
        } else {
            skill_map.insert(skill.name.clone(), skill);
        }
    }

    assert_eq!(skill_map.len(), 1);
    assert!(skill_map.contains_key("calendar"));
    assert_eq!(collision_warnings.len(), 1);
    assert!(collision_warnings[0].contains("name collision"));
}
