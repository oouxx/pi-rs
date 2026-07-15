//! Tests for AgentSessionRuntime — session lifecycle management.
//!
//! These tests verify that AgentSessionRuntime correctly manages session
//! lifecycle: creation, switch, new, fork, import, and dispose.
//!
//! Run with:
//!   cargo test -p pi-coding-agent --test agent_session_runtime_test -- --nocapture

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
    use pi_coding_agent::core::event_bus::EventBusController;
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
    let model = available.into_iter().next().unwrap_or_else(|| {
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
            },
            context_window: 128000,
            max_tokens: 4096,
            headers: None,
            compat: None,
        }
    });

    let event_bus = EventBusController::new();
    let extension_registry = Arc::new(ExtensionRegistry::new());

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
        resources: None,
    };

    let session = AgentSession::new(session_manager, event_bus, model_registry, session_options).await;
    (session, services)
}

/// Create a runtime factory for testing.
fn create_test_factory() -> CreateAgentSessionRuntimeFactory {
    Box::new(|params: CreateAgentSessionRuntimeParams| {
        Box::pin(async move {
            let (session, services) = create_test_session(
                &params.cwd,
                params.session_manager,
            )
            .await;
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

    let session_manager = SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None);
    let factory = create_test_factory();

    let runtime = AgentSessionRuntime::new(
        create_test_session(&cwd, session_manager).await.0,
        create_test_session(&cwd, SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None)).await.1,
        factory,
        Vec::new(),
        None,
    );

    (runtime, dir)
}

// ============================================================================
// Tests
// ============================================================================

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
async fn test_runtime_session_mut() {
    let (mut runtime, _dir) = create_test_runtime().await;

    // session_mut should give mutable access to the session
    let session_id = runtime.session().get_session_id();
    let session_mut_id = runtime.session_mut().get_session_id();
    assert_eq!(session_id, session_mut_id);
}

#[tokio::test]
async fn test_runtime_new_session() {
    let (mut runtime, dir) = create_test_runtime().await;
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
    let other_session_file = other_mgr.get_session_file().unwrap().to_string_lossy().to_string();

    // Switch to the other session
    let result = runtime.switch_session(&other_session_file, None).await;
    assert!(result.is_ok());
    assert!(result.unwrap());

    // Session ID should have changed
    let new_session_id = runtime.session().get_session_id();
    assert_ne!(original_session_id, new_session_id);
}

#[tokio::test]
async fn test_runtime_switch_session_nonexistent() {
    let (mut runtime, _dir) = create_test_runtime().await;

    // Switching to a nonexistent file creates a new session (SessionManager
    // creates a new file if the target doesn't exist)
    let result = runtime.switch_session("/nonexistent/session.jsonl", None).await;
    // This should succeed (creates a new session at the path)
    assert!(result.is_ok());
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
    std::fs::copy(&session_file, &import_path).unwrap();

    // Import the session
    let result = runtime.import_from_jsonl(
        &import_path.to_string_lossy(),
        None,
    ).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[tokio::test]
async fn test_runtime_import_from_jsonl_nonexistent() {
    let (mut runtime, _dir) = create_test_runtime().await;

    // Importing a nonexistent file should fail
    let result = runtime.import_from_jsonl("/nonexistent/session.jsonl", None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_runtime_fork() {
    let (mut runtime, dir) = create_test_runtime().await;
    let original_session_id = runtime.session().get_session_id();

    // Add a message so we have something to fork from
    {
        let session = runtime.session_mut();
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
        mgr.get_leaf_id().map(|s| s.to_string())
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

    let session_manager = SessionManager::new(&cwd, &session_dir.to_string_lossy(), None, false, None);
    let factory = create_test_factory();

    let runtime = pi_coding_agent::core::agent_session_runtime::create_agent_session_runtime(
        factory,
        CreateAgentSessionRuntimeParams {
            cwd: cwd.clone(),
            agent_dir: cwd.clone(),
            session_manager,
        },
    )
    .await;

    assert!(!runtime.cwd().is_empty());
    assert!(!runtime.session().get_session_id().is_empty());
}
