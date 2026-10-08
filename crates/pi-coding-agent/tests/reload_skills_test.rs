//! Regression test for `/reload` rediscovering skills.
//!
//! Protects against the former behavior where `AgentSession::reload()` only
//! reloaded settings but never re-scanned resource paths, so a skill added or
//! edited while a session was running could not be picked up by `/reload`.
//!
//! Run with:
//!   cargo test -p pi-coding-agent --test reload_skills_test -- --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use pi_coding_agent::core::agent_session::{AgentSession, AgentSessionConfig};
use pi_coding_agent::core::agent_session_services::{
    create_agent_session_services, CreateAgentSessionServicesOptions,
};
use pi_coding_agent::core::extensions::ExtensionRegistry;
use pi_coding_agent::core::model_registry::ModelRegistry;
use pi_coding_agent::core::resource_loader::{DefaultResourceLoader, ResourceLoader, ResourceLoaderOptions};
use pi_coding_agent::core::session_manager::SessionManager;

fn test_model() -> pi_agent_core::pi_ai_types::Model {
    pi_agent_core::pi_ai_types::Model {
        id: "test-model".to_string(),
        name: "Test Model".to_string(),
        api: "test-api".to_string(),
        provider: "test".to_string(),
        base_url: "http://localhost".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: Vec::new(),
        cost: pi_agent_core::pi_ai_types::ModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: vec![],
        },
        context_window: 128000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

async fn create_session(cwd: &str, agent_dir: &str) -> AgentSession {
    let resource_opts = ResourceLoaderOptions {
        cwd: cwd.to_string(),
        agent_dir: Some(agent_dir.to_string()),
        include_defaults: true,
        ..Default::default()
    };

    let services = create_agent_session_services(CreateAgentSessionServicesOptions {
        cwd: cwd.to_string(),
        agent_dir: Some(agent_dir.to_string()),
        auth_storage: None,
        settings_manager: None,
        model_registry: None,
        resource_loader_options: Some(resource_opts.clone()),
    })
    .await;

    let session_manager = SessionManager::new(cwd, &format!("{agent_dir}/sessions"), None, false, None);
    let settings_manager =
        pi_coding_agent::core::settings_manager::SettingsManager::create(cwd, Some(agent_dir));
    let model_registry = ModelRegistry::new(ModelRegistry::builtin_models_list());

    let _ = services;

    // The session retains the loader instance so `reload()` rescans from disk,
    // matching TS `AgentSessionConfig.resourceLoader`.
    let mut loader = DefaultResourceLoader::new(resource_opts);
    let _initial = loader.reload();

    let options = AgentSessionConfig {
        cwd: cwd.to_string(),
        model: test_model(),
        thinking_level: "medium".to_string(),
        custom_prompt: None,
        append_system_prompt: None,
        selected_tools: None,
        tool_snippets: None,
        prompt_guidelines: None,
        context_files: Vec::new(),
        skills: Vec::new(),
        session_name: None,
        stream_fn: None,
        convert_to_llm: None,
        initial_active_tool_names: None,
        allowed_tool_names: None,
        excluded_tool_names: None,
        extension_registry: Some(Arc::new(ExtensionRegistry::new())),
        ui_context: None,
        custom_tools: None,
        tools_options: None,
        resource_loader: Some(Box::new(loader)),
        extension_state_view: None,
        extension_action_rx: None,
    };

    AgentSession::new(session_manager, settings_manager, model_registry, options).await
}

fn skill_names(session: &AgentSession) -> Vec<String> {
    session
        .resource_loader()
        .map(|r| r.skills.iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default()
}

fn command_names(session: &AgentSession) -> Vec<String> {
    session
        .get_commands_info()
        .into_iter()
        .map(|c| c.name)
        .collect()
}

#[tokio::test]
async fn reload_discovers_newly_added_skill() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_string_lossy().to_string();
    let agent_dir = root.path().join("agent");
    let agent_dir_str = agent_dir.to_string_lossy().to_string();
    std::fs::create_dir_all(root.path().join(".pi-rs").join("skills")).unwrap();

    let session = create_session(&cwd, &agent_dir_str).await;

    // Baseline: no skills yet.
    assert!(
        !skill_names(&session).contains(&"check-docs".to_string()),
        "skill should not exist before it is written"
    );

    // Add a skill while the session is already constructed.
    std::fs::write(
        root.path().join(".pi-rs").join("skills").join("check-docs.md"),
        "---\nname: check-docs\ndescription: Check the repository documentation\n---\n\n# Check docs\n",
    )
    .unwrap();

    // Before reload the running session must not see it.
    assert!(
        !skill_names(&session).contains(&"check-docs".to_string()),
        "skill must not appear without an explicit reload"
    );

    session.reload().await;

    assert!(
        skill_names(&session).contains(&"check-docs".to_string()),
        "reload() must rediscover the newly added skill"
    );
    assert!(
        command_names(&session).contains(&"skill:check-docs".to_string()),
        "reload() must expose the skill as a /skill:<name> command"
    );
}

#[tokio::test]
async fn reload_is_idempotent_when_nothing_changed() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_string_lossy().to_string();
    let agent_dir = root.path().join("agent");
    let agent_dir_str = agent_dir.to_string_lossy().to_string();
    std::fs::create_dir_all(root.path().join(".pi-rs").join("skills")).unwrap();
    std::fs::write(
        root.path().join(".pi-rs").join("skills").join("stable.md"),
        "---\nname: stable\ndescription: Stable skill\n---\n\n# Stable\n",
    )
    .unwrap();

    let session = create_session(&cwd, &agent_dir_str).await;
    assert!(skill_names(&session).contains(&"stable".to_string()));

    session.reload().await;

    let names = skill_names(&session);
    assert_eq!(
        names.iter().filter(|n| *n == "stable").count(),
        1,
        "reload must not duplicate skills, got {names:?}"
    );
}
