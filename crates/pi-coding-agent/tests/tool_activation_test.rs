//! Built-in tool registry and activation, aligned with the TypeScript original.
//!
//! TS registers **all** built-in tool definitions (`read`, `bash`, `edit`,
//! `write`, `grep`, `find`, `ls`) and activates only `read`/`bash`/`edit`/`write`
//! by default (`DEFAULT_TOOL_NAMES`). Activation changes come from `--tools`
//! (`allowedToolNames`), the `defaultTools` setting, or `setActiveTools()`.
//! These tests pin that behavior:
//!
//!   cargo test -p pi-coding-agent --test tool_activation_test

#![allow(clippy::unwrap_used, clippy::expect_used)]

use pi_agent_core::pi_ai_types::{Model, ModelCost};
use pi_coding_agent::core::agent_session::AgentSession;
use pi_coding_agent::core::sdk::{create_agent_session, CreateAgentSessionOptions};
use pi_coding_agent::core::settings_manager::{
    InMemorySettingsStorage, Settings, SettingsManager,
};

fn make_model() -> Model {
    Model {
        input_limits: None,
        prompt_cache: None,
        id: "tool-activation-test-model".to_string(),
        name: "Tool activation test model".to_string(),
        api: "openai-completions".to_string(),
        provider: "test".to_string(),
        base_url: "http://localhost:1234/v1".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".to_string()],
        cost: ModelCost::default(),
        context_window: 128000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    }
}

async fn create_session(
    agent_dir: &tempfile::TempDir,
    configure: impl FnOnce(&mut CreateAgentSessionOptions),
) -> AgentSession {
    let mut options = CreateAgentSessionOptions {
        cwd: ".".to_string(),
        agent_dir: Some(agent_dir.path().to_string_lossy().to_string()),
        model: Some(make_model()),
        persist_session: false,
        ..Default::default()
    };
    configure(&mut options);
    let (session, _result) = create_agent_session(options)
        .await
        .expect("create_agent_session failed");
    session
}

/// All seven built-in tools are registered, but only the TS default set is
/// active.
#[tokio::test]
async fn default_session_registers_all_builtins_and_activates_four() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session = create_session(&tmp, |_| {}).await;

    let registered: Vec<String> = session.get_all_tools().into_iter().map(|t| t.name).collect();
    for name in ["read", "bash", "edit", "write", "grep", "find", "ls"] {
        assert!(
            registered.contains(&name.to_string()),
            "{name} must be registered (getAllTools), got {registered:?}"
        );
    }

    assert_eq!(
        session.get_active_tool_names().await,
        vec!["read", "bash", "edit", "write"],
        "default active set is TS DEFAULT_TOOL_NAMES"
    );
}

/// `--tools read,grep,find,ls` (TS `allowedToolNames`): naming a tool activates
/// it even when it is not active by default, and the allowlist is a
/// replacement rather than an addition.
#[tokio::test]
async fn tools_allowlist_activates_grep_find_ls() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session = create_session(&tmp, |options| {
        options.tools = Some(vec![
            "read".to_string(),
            "grep".to_string(),
            "find".to_string(),
            "ls".to_string(),
        ]);
    })
    .await;

    assert_eq!(
        session.get_active_tool_names().await,
        vec!["read", "grep", "find", "ls"],
        "grep/find/ls must be activatable through the allowlist"
    );
    assert!(
        !session
            .get_active_tool_names()
            .await
            .contains(&"bash".to_string()),
        "allowlist replaces the default selection"
    );

    // The system prompt only declares active tools.
    let state = session.get_agent().state().await;
    assert!(state.system_prompt.contains("- read:"), "read declared");
    assert!(state.system_prompt.contains("- grep:"), "grep declared");
    assert!(
        !state.system_prompt.contains("- bash:"),
        "bash is not active, so it must not be declared"
    );
}

/// `setActiveToolsByName()` resolves against the full registry, so a session
/// started with the defaults can enable grep/find/ls later.
#[tokio::test]
async fn set_active_tools_by_name_activates_non_default_builtins() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session = create_session(&tmp, |_| {}).await;

    session
        .set_active_tools_by_name(&[
            "read".to_string(),
            "grep".to_string(),
            "find".to_string(),
            "ls".to_string(),
        ])
        .await;

    assert_eq!(
        session.get_active_tool_names().await,
        vec!["read", "grep", "find", "ls"]
    );
    let state = session.get_agent().state().await;
    assert!(
        state.system_prompt.contains("- find:"),
        "system prompt rebuild must declare the new tools"
    );
    assert!(!state.system_prompt.contains("- bash:"));

    // Unknown names are ignored (TS filters them against the registry).
    session
        .set_active_tools_by_name(&["read".to_string(), "does-not-exist".to_string()])
        .await;
    assert_eq!(session.get_active_tool_names().await, vec!["read"]);
}

/// `defaultTools` with `+name` modifiers (TS `resolveDefaultTools`) enables the
/// new tools on top of the defaults.
#[tokio::test]
async fn default_tools_setting_modifiers_activate_grep_find_ls() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings_manager = SettingsManager::new(
        Box::new(InMemorySettingsStorage::new()),
        Settings {
            default_tools: Some(vec![
                "+grep".to_string(),
                "+find".to_string(),
                "+ls".to_string(),
            ]),
            ..Default::default()
        },
        Settings::default(),
    );
    let session = create_session(&tmp, |options| {
        options.settings_manager = Some(settings_manager);
    })
    .await;

    assert_eq!(
        session.get_active_tool_names().await,
        vec!["read", "bash", "edit", "write", "grep", "find", "ls"]
    );
}

/// Denylisted tools are removed from the registry, so they cannot be
/// re-activated (TS `_isAllowedTool` filters the registry).
#[tokio::test]
async fn excluded_tools_are_not_registered() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let session = create_session(&tmp, |options| {
        options.exclude_tools = Some(vec!["bash".to_string(), "write".to_string()]);
    })
    .await;

    assert_eq!(session.get_active_tool_names().await, vec!["read", "edit"]);
    let registered: Vec<String> = session.get_all_tools().into_iter().map(|t| t.name).collect();
    assert!(!registered.contains(&"bash".to_string()));
    assert!(!registered.contains(&"write".to_string()));

    session
        .set_active_tools_by_name(&["read".to_string(), "bash".to_string()])
        .await;
    assert_eq!(
        session.get_active_tool_names().await,
        vec!["read"],
        "an excluded tool is not in the registry and cannot be re-enabled"
    );
}

/// `/reload` activates tools newly added to `defaultTools` (TS
/// `addedDefaultTools`), but leaves removed ones active.
#[tokio::test]
async fn reload_activates_tools_newly_added_to_default_tools() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings_path = tmp.path().join("settings.json");
    std::fs::write(&settings_path, r#"{"defaultTools": ["read"]}"#).expect("write settings");

    let session = create_session(&tmp, |_| {}).await;
    assert_eq!(session.get_active_tool_names().await, vec!["read"]);

    std::fs::write(&settings_path, r#"{"defaultTools": ["read", "grep"]}"#)
        .expect("rewrite settings");
    session.reload().await;

    assert_eq!(
        session.get_active_tool_names().await,
        vec!["read", "grep"],
        "reload must activate tools newly added to defaultTools"
    );

    // Removing the setting falls back to DEFAULT_TOOL_NAMES (TS
    // `getDefaultTools() ?? DEFAULT_TOOL_NAMES`), so defaults that were not
    // active before are added; previously active tools stay active.
    std::fs::write(&settings_path, "{}").expect("clear settings");
    session.reload().await;
    assert_eq!(
        session.get_active_tool_names().await,
        vec!["read", "grep", "bash", "edit", "write"]
    );
}
