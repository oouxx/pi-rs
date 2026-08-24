//! Tests for AgentSessionRuntime — session lifecycle management.
//!
//! These tests verify that AgentSessionRuntime correctly manages session
//! lifecycle: creation, switch, new, fork, import, and dispose.
//!
//! Run with:
//!   cargo test -p pi-coding-agent --test agent_session_runtime_test -- --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use pi_coding_agent::core::agent_session::AgentSession;
use pi_coding_agent::core::agent_session_runtime::{
    AgentSessionRuntime, CreateAgentSessionRuntimeFactory, CreateAgentSessionRuntimeParams,
    CreateAgentSessionRuntimeResult,
};
use pi_coding_agent::core::agent_session_services::{
    AgentSessionServices, CreateAgentSessionServicesOptions,
};
use pi_coding_agent::core::session_manager::SessionManager;

/// Create a minimal AgentSession for testing.
/// This creates a session with default settings and no real LLM provider.
async fn create_test_session(
    cwd: &str,
    session_manager: SessionManager,
) -> (AgentSession, AgentSessionServices) {
    use pi_coding_agent::core::agent_session::AgentSessionConfig;
    use pi_coding_agent::core::extensions::ExtensionRegistry;
    use pi_coding_agent::core::model_registry::ModelRegistry;

    let services = pi_coding_agent::core::agent_session_services::create_agent_session_services(
        CreateAgentSessionServicesOptions {
            cwd: cwd.to_string(),
            agent_dir: None,
            auth_storage: None,
            settings_manager: None,
            model_registry: None,
            resource_loader_options: None,
        },
    )
    .await;

    let model_registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
    let available = model_registry.get_available();
    let model = available
        .into_iter()
        .next()
        .unwrap_or_else(|| pi_agent_core::pi_ai_types::Model {
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
        });

    let extension_registry = Arc::new(ExtensionRegistry::new());

    let settings_manager =
        pi_coding_agent::core::settings_manager::SettingsManager::create(cwd, None);
    let session_options = AgentSessionConfig {
        cwd: cwd.to_string(),
        model,
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
        extension_registry: Some(extension_registry),
        ui_context: None,
        custom_tools: None,
        tools_options: None,
        resources: None,
        extension_state_view: None,
        extension_action_rx: None,
    };

    let session = AgentSession::new(
        session_manager,
        settings_manager,
        model_registry,
        session_options,
    )
    .await;
    (session, services)
}

/// Create a runtime factory for testing.
fn create_test_factory() -> CreateAgentSessionRuntimeFactory {
    Arc::new(|params: CreateAgentSessionRuntimeParams| {
        Box::pin(async move {
            let (session, services) =
                create_test_session(&params.cwd, params.session_manager).await;
            CreateAgentSessionRuntimeResult {
                session,
                services,
                diagnostics: Vec::new(),
                model_fallback_message: None,
            }
        })
    })
}

/// Create a test runtime with a temp directory.
async fn create_test_runtime() -> (AgentSessionRuntime, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().to_string();
    let session_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();

    let session_manager =
        SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None);
    let factory = create_test_factory();

    let runtime = AgentSessionRuntime::new(
        create_test_session(&cwd, session_manager).await.0,
        create_test_session(
            &cwd,
            SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None),
        )
        .await
        .1,
        factory,
        Vec::new(),
        None,
    );

    (runtime, dir)
}

// ============================================================================
// Tests
// ============================================================================

/// runtime.queue_follow_up（扩展自动延续通道，/goal 续跑用它）会把文本
/// 推入 follow-up 镜像：Esc 中断时 TUI 通过 clear_all_queues 拿到它并还原
/// 回编辑器（对齐 TS `_queueFollowUp` 推入 `_followUpMessages`）。
#[tokio::test]
async fn test_extension_queue_follow_up_mirrors_text_for_editor_restore() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().to_string();
    let session_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();
    let session_manager =
        SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None);
    let (session, _services) = create_test_session(&cwd, session_manager).await;

    let ctx = session.get_extension_context().clone();
    (ctx.runtime.queue_follow_up)("queued continuation".to_string());

    // 镜像已记录文本（message_start 消费时才会移除）。
    assert_eq!(
        session.get_follow_up_messages(),
        vec!["queued continuation".to_string()]
    );
    assert_eq!(session.pending_message_count(), 1);

    // Esc 中断后 TUI 的 RestoreQueuedToEditor 路径：clear_all_queues 拿回
    // 文本并清空镜像。
    let (steering, follow_up) = session.clear_all_queues().await;
    assert!(steering.is_empty());
    assert_eq!(
        follow_up,
        vec!["queued continuation".to_string()]
    );
    assert_eq!(session.pending_message_count(), 0);
}

#[tokio::test]
async fn test_runtime_creation() {
    let (runtime, _dir) = create_test_runtime().await;

    // Verify basic accessors
    assert!(!runtime.cwd().is_empty());
    assert!(runtime.diagnostics().is_empty());
    assert!(runtime.model_fallback_message().is_none());
    assert!(!runtime.session().get_session_id().is_empty());
}

#[tokio::test]
async fn test_runtime_services_accessor() {
    let (runtime, _dir) = create_test_runtime().await;

    let services = runtime.services();
    assert!(!services.cwd.is_empty());
    assert!(!services.agent_dir.is_empty());
}

#[tokio::test]
async fn test_runtime_session_arc() {
    let (runtime, _dir) = create_test_runtime().await;

    // session_arc should give a shared handle to the same session
    let session_id = runtime.session().get_session_id();
    let session_arc_id = runtime.session_arc().get_session_id();
    assert_eq!(session_id, session_arc_id);
}

#[tokio::test]
async fn test_runtime_new_session() {
    let (mut runtime, _dir) = create_test_runtime().await;
    let original_session_id = runtime.session().get_session_id();

    // Create a new session
    let result = runtime.new_session(None).await;
    assert!(result.is_ok());
    assert!(result.unwrap());

    // Session ID should have changed
    let new_session_id = runtime.session().get_session_id();
    assert_ne!(original_session_id, new_session_id);
}

#[tokio::test]
async fn test_runtime_dispose() {
    let (runtime, _dir) = create_test_runtime().await;

    // Dispose should not panic
    runtime.dispose().await;
}

#[tokio::test]
async fn test_runtime_set_rebind_session() {
    let (mut runtime, _dir) = create_test_runtime().await;
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();

    runtime.set_rebind_session(Some(Arc::new(move |_session: &AgentSession| {
        let c = called_clone.clone();
        Box::pin(async move {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    })));

    // The callback is stored but not called until session replacement
    // We can verify it was stored by checking that new_session triggers it
    // (new_session calls finish_session_replacement which calls rebind_session)
    let result = runtime.new_session(None).await;
    assert!(result.is_ok());

    // The rebind callback should have been called
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn test_runtime_set_before_session_invalidate() {
    let (mut runtime, _dir) = create_test_runtime().await;
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();

    runtime.set_before_session_invalidate(Some(Box::new(move || {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    })));

    // The callback is called during teardown_current, which happens during new_session
    let result = runtime.new_session(None).await;
    assert!(result.is_ok());

    // The before_session_invalidate callback should have been called
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn test_runtime_switch_session() {
    let (mut runtime, dir) = create_test_runtime().await;
    let original_session_id = runtime.session().get_session_id();

    // Create another session file to switch to
    let session_dir = dir.path().join("sessions");
    let other_mgr = SessionManager::new(
        dir.path().to_string_lossy().as_ref(),
        &session_dir.to_string_lossy(),
        None,
        true,
        None,
    );
    let other_session_file = other_mgr
        .get_session_file()
        .unwrap()
        .to_string_lossy()
        .to_string();
    // Write session header to disk (SessionManager::new() sets the path but
    // doesn't write the file until the first assistant message arrives)
    {
        use std::io::Write;
        let header = serde_json::json!({
            "type": "session",
            "version": pi_coding_agent::core::session_manager::CURRENT_SESSION_VERSION,
            "id": other_mgr.get_session_id(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "cwd": other_mgr.get_cwd(),
        });
        let mut f = std::fs::File::create(&other_session_file).unwrap();
        writeln!(f, "{}", serde_json::to_string(&header).unwrap()).unwrap();
    }

    // Switch to the other session
    let result = runtime
        .switch_session(&other_session_file, None, None)
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap());

    // Session ID should have changed
    let new_session_id = runtime.session().get_session_id();
    assert_ne!(original_session_id, new_session_id);
}

#[tokio::test]
async fn test_runtime_switch_session_nonexistent() {
    let (mut runtime, _dir) = create_test_runtime().await;

    // Switching to a nonexistent file should fail with file-not-found error
    let result = runtime
        .switch_session("/nonexistent/session.jsonl", None, None)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_runtime_import_from_jsonl() {
    let (mut runtime, dir) = create_test_runtime().await;

    // Create a valid session file to import
    let import_path = dir.path().join("import.jsonl");
    let session_dir = dir.path().join("sessions");
    let import_mgr = SessionManager::new(
        dir.path().to_string_lossy().as_ref(),
        &session_dir.to_string_lossy(),
        None,
        true,
        None,
    );
    // Write the session file to the import path
    let session_file = import_mgr.get_session_file().unwrap();
    // Write session header to disk first
    {
        use std::io::Write;
        let header = serde_json::json!({
            "type": "session",
            "version": pi_coding_agent::core::session_manager::CURRENT_SESSION_VERSION,
            "id": import_mgr.get_session_id(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "cwd": import_mgr.get_cwd(),
        });
        let mut f = std::fs::File::create(session_file).unwrap();
        writeln!(f, "{}", serde_json::to_string(&header).unwrap()).unwrap();
    }

    std::fs::copy(session_file, &import_path).unwrap();

    // Import the session
    let result = runtime
        .import_from_jsonl(&import_path.to_string_lossy(), None)
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[tokio::test]
async fn test_runtime_import_from_jsonl_nonexistent() {
    let (mut runtime, _dir) = create_test_runtime().await;

    // Importing a nonexistent file should fail
    let result = runtime
        .import_from_jsonl("/nonexistent/session.jsonl", None)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_runtime_fork() {
    let (mut runtime, _dir) = create_test_runtime().await;
    let original_session_id = runtime.session().get_session_id();

    // Add a message so we have something to fork from
    {
        let session = runtime.session_arc();
        let mut mgr = session.get_session_manager();
        mgr.append_message(serde_json::json!({
            "role": "user",
            "content": "Hello"
        }));
    }

    // Get the leaf entry ID
    let leaf_id = {
        let session = runtime.session();
        let mgr = session.get_session_manager();
        mgr.get_leaf_id().map(ToString::to_string)
    };
    assert!(leaf_id.is_some());

    // Fork at the leaf
    let leaf_id = leaf_id.unwrap();
    let result = runtime.fork(&leaf_id, Some("at")).await;
    assert!(result.is_ok(), "Fork failed: {:?}", result.err());
    let (cancelled, selected_text) = result.unwrap();
    assert!(!cancelled);
    assert!(selected_text.is_none());

    // Session ID should have changed after fork
    let new_session_id = runtime.session().get_session_id();
    assert_ne!(original_session_id, new_session_id);
}

#[tokio::test]
async fn test_runtime_fork_invalid_entry() {
    let (mut runtime, _dir) = create_test_runtime().await;

    // Forking with an invalid entry ID should fail
    let result = runtime.fork("nonexistent-entry", Some("at")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_create_agent_session_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().to_string();
    let session_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();

    let session_manager =
        SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None);
    let factory = create_test_factory();

    let runtime = pi_coding_agent::core::agent_session_runtime::create_agent_session_runtime(
        factory,
        CreateAgentSessionRuntimeParams {
            session_start_event: None,
            cwd: cwd.clone(),
            agent_dir: cwd.clone(),
            session_manager,
        },
    )
    .await;

    assert!(!runtime.cwd().is_empty());
    assert!(!runtime.session().get_session_id().is_empty());
}

/// Extension action bus: write-actions queued via the bus are applied by
/// `drain_extension_actions()` at turn boundaries; read-actions see the
/// refreshed state snapshot.
#[tokio::test]
async fn test_extension_action_bus_drain_and_state_refresh() {
    use pi_coding_agent::core::agent_session::AgentSessionConfig;
    use pi_coding_agent::core::extensions::action_bus::{
        ExtensionAction, ExtensionActionSender,
    };
    use pi_coding_agent::core::extensions::ExtensionRegistry;
    use pi_coding_agent::core::model_registry::ModelRegistry;

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().to_string();
    let session_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();
    let session_manager =
        SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None);
    let settings_manager =
        pi_coding_agent::core::settings_manager::SettingsManager::create(&cwd, None);

    let (sender, rx, state_view) = ExtensionActionSender::new();

    let model_registry = ModelRegistry::new(ModelRegistry::builtin_models_list());
    // Directly constructed thinking-capable model (the builtin models don't
    // support thinking, which would clamp set_thinking_level to "off").
    let model = pi_agent_core::pi_ai_types::Model {
        id: "test-model".to_string(),
        name: "Test Model".to_string(),
        api: "test-api".to_string(),
        provider: "test".to_string(),
        base_url: "http://localhost".to_string(),
        reasoning: true,
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
    };

    let extension_registry = Arc::new(ExtensionRegistry::new());
    let options = AgentSessionConfig {
        cwd: cwd.clone(),
        model,
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
        extension_registry: Some(extension_registry),
        ui_context: None,
        custom_tools: None,
        tools_options: None,
        resources: None,
        extension_state_view: Some(state_view),
        extension_action_rx: Some(rx),
    };

    let session = AgentSession::new(session_manager, settings_manager, model_registry, options)
        .await;

    // ── Write-actions: queue via the sender, apply via drain ──
    sender.send(ExtensionAction::SetSessionName("bus-test".to_string()));
    sender.send(ExtensionAction::SetThinkingLevel("high".to_string()));
    sender.send(ExtensionAction::SetActiveTools(vec!["read".to_string()]));
    session.drain_extension_actions().await;

    assert_eq!(session.get_session_name().as_deref(), Some("bus-test"));
    assert_eq!(session.get_thinking_level().await, "high");
    assert_eq!(session.get_active_tool_names().await, vec!["read"]);

    // ── Read-actions: state snapshot refreshed by drain ──
    let view = sender.state();
    {
        let guard = view.lock().unwrap();
        assert_eq!(guard.session_name.as_deref(), Some("bus-test"));
        assert_eq!(guard.thinking_level, "high");
        assert_eq!(guard.active_tools, vec!["read"]);
        assert!(!guard.model_id.as_deref().unwrap_or("").is_empty());
    }

    // ── appendEntry + setLabel through the bus ──
    sender.send(ExtensionAction::AppendEntry {
        custom_type: "test-entry".to_string(),
        data_json: Some("{\"x\":1}".to_string()),
    });
    session.drain_extension_actions().await;
    // Scope the session_manager guard so it's dropped before the next drain
    // (a held guard would deadlock the session_manager lock inside drain).
    let entry_found = {
        let mgr = session.get_session_manager();
        mgr.get_entries().iter().any(|e| {
            matches!(
                e,
                pi_coding_agent::core::session_manager::SessionEntry::Custom {
                    custom_type, ..
                } if custom_type == "test-entry"
            )
        })
    };
    assert!(entry_found, "custom entry should be appended");

    // ── sendMessage (triggerTurn=false) appends to agent messages ──
    sender.send(ExtensionAction::SendMessage {
        custom_type: "msg-type".to_string(),
        content: "hello from extension".to_string(),
        options_json: None,
    });
    session.drain_extension_actions().await;
    let messages = session.get_agent().messages().await;
    assert!(
        messages.iter().any(|m| {
            matches!(
                m,
                pi_agent_core::types::AgentMessage::User { content, .. }
                    if content.iter().any(|c| matches!(
                        c,
                        pi_agent_core::pi_ai_types::ContentBlock::Text { text, .. }
                            if text == "hello from extension"
                    ))
            )
        }),
        "sendMessage must append a user message"
    );
}

/// Session switch must trigger the JS-runtime invalidator (stale-ctx guard):
/// `session_mgr_switch` calls the callback wired by the SDK.
#[tokio::test]
async fn test_session_switch_invalidates_js_runtime() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use pi_coding_agent::core::agent_session::AgentSessionConfig;
    use pi_coding_agent::core::extensions::ExtensionRegistry;
    use pi_coding_agent::core::model_registry::ModelRegistry;

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().to_string();
    let session_dir = dir.path().join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();
    let session_manager =
        SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None);
    let settings_manager =
        pi_coding_agent::core::settings_manager::SettingsManager::create(&cwd, None);
    let model = pi_agent_core::pi_ai_types::Model {
        id: "test-model".to_string(),
        name: "Test Model".to_string(),
        api: "test-api".to_string(),
        provider: "test".to_string(),
        base_url: "http://localhost".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: Vec::new(),
        cost: pi_agent_core::pi_ai_types::ModelCost::default(),
        context_window: 128000,
        max_tokens: 4096,
        sampling_params: None,
        headers: None,
        compat: None,
    };
    let options = AgentSessionConfig {
        cwd: cwd.clone(),
        model,
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
        resources: None,
        extension_state_view: None,
        extension_action_rx: None,
    };
    let mut session =
        AgentSession::new(session_manager, settings_manager, ModelRegistry::new(ModelRegistry::builtin_models_list()), options).await;

    let invalidated = Arc::new(AtomicUsize::new(0));
    let inv = invalidated.clone();
    session.set_js_invalidator(Some(Arc::new(move || {
        inv.fetch_add(1, Ordering::SeqCst);
    })));

    // A minimal valid session file for switch_session to load.
    let target = dir.path().join("target.jsonl");
    std::fs::write(
        &target,
        r#"{"type":"session","version":3,"id":"target-session","timestamp":"2026-01-01T00:00:00Z","cwd":"/tmp"}"#,
    )
    .unwrap();

    session
        .session_mgr_switch(&target.to_string_lossy(), None)
        .await
        .expect("switch_session");
    assert_eq!(
        invalidated.load(Ordering::SeqCst),
        1,
        "session switch must invalidate the JS runtime"
    );
}
