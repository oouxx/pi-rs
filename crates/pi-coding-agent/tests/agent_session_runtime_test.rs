//! Tests for AgentSessionRuntime — session lifecycle management.
//!
//! These tests verify that AgentSessionRuntime correctly manages session
//! lifecycle: creation, switch, new, fork, import, and dispose.
//!
//! Run with:
//!   cargo test -p pi-coding-agent --test agent_session_runtime_test -- --nocapture

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

    let extension_registry = Arc::new(ExtensionRegistry::new());

    let settings_manager = pi_coding_agent::core::settings_manager::SettingsManager::create(cwd, None);
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
        custom_tools: None,
        resources: None,
    };

    let session = AgentSession::new(session_manager, settings_manager, model_registry, session_options).await;
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
    let other_session_file = other_mgr.get_session_file().unwrap().to_string_lossy().to_string();
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

    // Switching to a nonexistent file should fail with file-not-found error
    let result = runtime.switch_session("/nonexistent/session.jsonl", None).await;
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
    let (mut runtime, _dir) = create_test_runtime().await;
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
