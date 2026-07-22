use std::sync::Arc;

use crate::core::agent_session::AgentSession;
use crate::core::agent_session_services::{AgentSessionRuntimeDiagnostic, AgentSessionServices};

use crate::core::extensions::{ExtensionContext, ExtensionUIContext, RuntimeHandle};
use crate::core::session_manager::SessionManager;

// ============================================================================
// Errors
// ============================================================================

/// Thrown when /import references a JSONL file path that does not exist.
#[derive(Debug)]
pub struct SessionImportFileNotFoundError {
    pub file_path: String,
}

impl std::fmt::Display for SessionImportFileNotFoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "File not found: {}", self.file_path)
    }
}

impl std::error::Error for SessionImportFileNotFoundError {}

// ============================================================================
// Return type for runtime creation
// ============================================================================

/// Result returned by runtime creation.
///
/// The caller gets the created session, its cwd-bound services, and all
/// diagnostics collected during setup.
pub struct CreateAgentSessionRuntimeResult {
    pub session: AgentSession,
    pub services: AgentSessionServices,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    pub model_fallback_message: Option<String>,
}

// ============================================================================
// Runtime factory type
// ============================================================================

/// Parameters for creating a new runtime via the factory.
pub struct CreateAgentSessionRuntimeParams {
    pub cwd: String,
    pub agent_dir: String,
    pub session_manager: SessionManager,
}

/// Factory type for creating a new runtime.
///
/// The factory closes over process-global fixed inputs, recreates cwd-bound
/// services for the effective cwd, resolves session options against those
/// services, and finally creates the AgentSession.
pub type CreateAgentSessionRuntimeFactory = Box<
    dyn Fn(
            CreateAgentSessionRuntimeParams,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = CreateAgentSessionRuntimeResult> + Send>,
        > + Send
        + Sync,
>;

// ============================================================================
// AgentSessionRuntime
// ============================================================================

/// Owns the current AgentSession plus its cwd-bound services.
///
/// Session replacement methods tear down the current runtime first, then create
/// and apply the next runtime. If creation fails, the error is propagated to the
/// caller. The caller is responsible for user-facing error handling.
///
/// This is the primary entry point for session lifecycle management:
/// - `switch_session()` — resume an existing session file
/// - `new_session()` — create a fresh session
/// - `fork()` — fork from an entry in the current session
/// - `import_from_jsonl()` — import a session from a JSONL file
/// - `dispose()` — clean up the current session
pub struct AgentSessionRuntime {
    session: AgentSession,
    services: AgentSessionServices,
    create_runtime: CreateAgentSessionRuntimeFactory,
    diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    model_fallback_message: Option<String>,
    /// Callback invoked after a session replacement to rebind the new session
    /// to the host (e.g., TUI event listeners).
    rebind_session: Option<
        Arc<
            dyn Fn(
                    &AgentSession,
                )
                    -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                + Send
                + Sync,
        >,
    >,
    /// Synchronous callback that runs after `session_shutdown` handlers finish
    /// but before the current session is invalidated. Used for host-owned UI
    /// teardown (e.g., detaching extension-provided TUI components).
    before_session_invalidate: Option<Box<dyn Fn() + Send + Sync>>,
}

impl AgentSessionRuntime {
    /// Create a new AgentSessionRuntime.
    ///
    /// The `create_runtime` factory is stored and reused for later session
    /// replacement operations (switch, new, fork, import).
    pub fn new(
        session: AgentSession,
        services: AgentSessionServices,
        create_runtime: CreateAgentSessionRuntimeFactory,
        diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
        model_fallback_message: Option<String>,
    ) -> Self {
        Self {
            session,
            services,
            create_runtime,
            diagnostics,
            model_fallback_message,
            rebind_session: None,
            before_session_invalidate: None,
        }
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    /// Returns a reference to the cwd-bound services.
    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    /// Returns a reference to the current AgentSession.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Returns a mutable reference to the current AgentSession.
    pub fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// Returns the effective working directory of the current session.
    pub fn cwd(&self) -> &str {
        &self.services.cwd
    }

    /// Returns diagnostics collected during runtime creation.
    pub fn diagnostics(&self) -> &[AgentSessionRuntimeDiagnostic] {
        &self.diagnostics
    }

    /// Returns the model fallback message, if any.
    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
    }

    // =========================================================================
    // Callbacks
    // =========================================================================

    /// Set a callback that runs after session replacement to rebind the new
    /// session to the host (e.g., TUI event listeners).
    pub fn set_rebind_session(
        &mut self,
        rebind_session: Option<
            Arc<
                dyn Fn(
                        &AgentSession,
                    )
                        -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                    + Send
                    + Sync,
            >,
        >,
    ) {
        self.rebind_session = rebind_session;
    }

    /// Set a synchronous callback that runs after `session_shutdown` handlers
    /// finish but before the current session is invalidated.
    ///
    /// This is for host-owned UI teardown that must not yield to the event loop,
    /// such as detaching extension-provided TUI components before the old
    /// extension context becomes stale.
    pub fn set_before_session_invalidate(
        &mut self,
        before_session_invalidate: Option<Box<dyn Fn() + Send + Sync>>,
    ) {
        self.before_session_invalidate = before_session_invalidate;
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    /// Create a noop ExtensionContext for the current cwd.
    /// Used for extension event dispatch during lifecycle operations.
    fn noop_ext_ctx(&self) -> ExtensionContext {
        ExtensionContext::new(
            self.cwd().to_string(),
            false,
            ExtensionUIContext {
                notify: Arc::new(|msg, _level| eprintln!("[pi] {msg}")),
                set_status: Arc::new(|_key, _value| {}),
                confirm: Arc::new(|_title, _msg| false),
            },
            RuntimeHandle::noop(),
        )
    }

    /// Emit `session_before_switch` to extensions and return whether the
    /// operation was cancelled.
    async fn emit_before_switch(
        &self,
        reason: &str,
        target_session_file: Option<&str>,
    ) -> bool {
        if let Some(ref registry) = self.session.get_extension_registry() {
            let result = registry
                .hook_runner()
                .run_before_session_switch(
                    reason.to_string(),
                    target_session_file.map(|s| s.to_string()),
                )
                .await;
            if result.is_cancel() {
                return true;
            }
        }
        false
    }

    /// Emit `session_before_fork` to extensions and return whether the
    /// operation was cancelled.
    async fn emit_before_fork(&self, entry_id: &str, position: &str) -> bool {
        if let Some(ref registry) = self.session.get_extension_registry() {
            let result = registry
                .hook_runner()
                .run_before_session_fork(entry_id.to_string(), position.to_string())
                .await;
            if result.is_cancel() {
                return true;
            }
        }
        false
    }

    /// Tear down the current session: emit shutdown, invalidate extension
    /// context, and call before_invalidate.
    ///
    /// The old session is NOT disposed here — it will be dropped when `apply()`
    /// replaces it with the new session. The `session_shutdown` event is
    /// dispatched so extensions can perform cleanup before the session is
    /// replaced.
    async fn teardown_current(&mut self, reason: &str) {
        // Emit session_shutdown to extensions
        if let Some(ref registry) = self.session.get_extension_registry() {
            registry.hook_runner().fire_session_shutdown(reason, None).await;
        }

        // Invalidate the extension context so any captured references
        // to the old session's context are detected as stale.
        self.session.invalidate_ext_ctx();

        // Call before_session_invalidate (synchronous)
        if let Some(ref invalidate) = self.before_session_invalidate {
            invalidate();
        }
    }

    /// Apply a new runtime result, replacing the current session and services.
    fn apply(&mut self, result: CreateAgentSessionRuntimeResult) {
        self.session = result.session;
        self.services = result.services;
        self.diagnostics = result.diagnostics;
        self.model_fallback_message = result.model_fallback_message;
    }

    /// Finish session replacement: call rebind_session.
    async fn finish_session_replacement(&self) {
        if let Some(ref rebind) = self.rebind_session {
            rebind(&self.session).await;
        }
    }

    // =========================================================================
    // Session Lifecycle
    // =========================================================================

    /// Switch to a different session file, matching the original
    /// AgentSessionRuntime.switchSession().
    ///
    /// Returns `true` if the switch was completed, `false` if cancelled by
    /// an extension.
    pub async fn switch_session(
        &mut self,
        session_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<bool, String> {
        // Validate the target session file exists and is a valid session file
        let path = std::path::Path::new(session_path);
        if !path.exists() {
            return Err(format!("Session file not found: {}", session_path));
        }
        if !crate::core::session_manager::is_valid_session_file(path) {
            return Err(format!("Invalid session file: {}", session_path));
        }

        // Emit session_before_switch (can cancel)
        let cancelled = self.emit_before_switch("resume", Some(session_path)).await;
        if cancelled {
            return Ok(false);
        }

        let session_dir = self.session.get_session_dir().to_string_lossy().to_string();

        // Open the target session
        let effective_cwd = cwd_override.unwrap_or(self.cwd());
        let session_manager = SessionManager::new(
            effective_cwd,
            &session_dir,
            Some(session_path),
            true,
            None,
        );

        // Validate session cwd exists
        let session_cwd = session_manager.get_cwd().to_string();
        if !session_cwd.is_empty() && !std::path::Path::new(&session_cwd).exists() {
            return Err(format!(
                "Stored session working directory does not exist: {}",
                session_cwd
            ));
        }

        // Teardown current session
        self.teardown_current("resume").await;

        // Create new runtime via factory
        let result = (self.create_runtime)(CreateAgentSessionRuntimeParams {
            cwd: session_manager.get_cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })
        .await;

        // Apply new state
        self.apply(result);

        // Finish replacement
        self.finish_session_replacement().await;

        Ok(true)
    }

    /// Create a new session, matching the original
    /// AgentSessionRuntime.newSession().
    ///
    /// Returns `true` if the new session was created, `false` if cancelled by
    /// an extension.
    pub async fn new_session(
        &mut self,
        parent_session: Option<&str>,
    ) -> Result<bool, String> {
        // Emit session_before_switch (can cancel)
        let cancelled = self.emit_before_switch("new", None).await;
        if cancelled {
            return Ok(false);
        }

        let session_dir = self.session.get_session_dir().to_string_lossy().to_string();

        // Create a new session manager
        let new_session_opts = parent_session.map(|p| {
            crate::core::session_manager::NewSessionOptions {
                id: None,
                parent_session: Some(p.to_string()),
            }
        });
        let session_manager = SessionManager::new(
            self.cwd(),
            &session_dir,
            None,
            true,
            new_session_opts,
        );

        // Teardown current session
        self.teardown_current("new").await;

        // Create new runtime via factory
        let result = (self.create_runtime)(CreateAgentSessionRuntimeParams {
            cwd: self.cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })
        .await;

        // Apply new state
        self.apply(result);

        // Finish replacement
        self.finish_session_replacement().await;

        Ok(true)
    }

    /// Fork the session at a specific entry, matching the original
    /// AgentSessionRuntime.fork().
    ///
    /// Returns `(cancelled, selected_text)` where `cancelled` indicates whether
    /// the operation was cancelled by an extension, and `selected_text` is the
    /// extracted user message text when forking "before" a user message.
    pub async fn fork(
        &mut self,
        entry_id: &str,
        position: Option<&str>,
    ) -> Result<(bool, Option<String>), String> {
        let position = position.unwrap_or("before");

        // Emit session_before_fork (can cancel)
        let cancelled = self.emit_before_fork(entry_id, position).await;
        if cancelled {
            return Ok((true, None));
        }

        // Validate the entry exists
        let entry = self.session.get_session_manager().get_entry(entry_id)
            .ok_or_else(|| format!("Invalid entry ID for forking: {}", entry_id))?
            .clone();

        // Determine target leaf ID and selected text
        let (target_leaf_id, selected_text) = if position == "at" {
            (Some(entry_id.to_string()), None)
        } else {
            // "before" position: fork before the entry
            // For user messages, extract the text
            let parent_id = entry.parent_id().map(|s| s.to_string());
            let text = if let crate::core::session_manager::SessionEntry::Message { message, .. } = &entry {
                extract_user_message_text(message)
            } else {
                None
            };
            (parent_id, text)
        };

        let session_dir = self.session.get_session_dir().to_string_lossy().to_string();

        // Create the forked session
        let session_manager = if let Some(ref leaf_id) = target_leaf_id {
            // Create a branched session from the target leaf.
            // For both persisted and in-memory sessions, we use
            // create_branched_session which copies entries up to the
            // target leaf into a new session file.
            let cwd = self.cwd().to_string();
            let branch_path = {
                let mut mgr = self.session.get_session_manager();
                mgr.create_branched_session(leaf_id, None)?
            };
            SessionManager::new(
                &cwd,
                &session_dir,
                Some(&branch_path),
                true,
                None,
            )
        } else {
            // No target leaf: create a fresh session
            SessionManager::new(
                self.cwd(),
                &session_dir,
                None,
                true,
                Some(crate::core::session_manager::NewSessionOptions {
                    id: None,
                    parent_session: None,
                }),
            )
        };

        // Teardown current session
        self.teardown_current("fork").await;

        // Create new runtime via factory
        let result = (self.create_runtime)(CreateAgentSessionRuntimeParams {
            cwd: session_manager.get_cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })
        .await;

        // Apply new state
        self.apply(result);

        // Finish replacement
        self.finish_session_replacement().await;

        Ok((false, selected_text))
    }

    /// Import a session from a JSONL file, matching the original
    /// AgentSessionRuntime.importFromJsonl().
    ///
    /// Returns `true` if the import was completed, `false` if cancelled by
    /// an extension.
    pub async fn import_from_jsonl(
        &mut self,
        input_path: &str,
        cwd_override: Option<&str>,
    ) -> Result<bool, String> {
        let resolved_path = std::path::Path::new(input_path);
        if !resolved_path.exists() {
            return Err(format!("File not found: {}", input_path));
        }

        let session_dir = self.session.get_session_dir().to_string_lossy().to_string();

        // Ensure session directory exists
        if !std::path::Path::new(&session_dir).exists() {
            std::fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;
        }

        // Emit session_before_switch (can cancel) BEFORE copying the file,
        // so a cancelled import doesn't leave an orphaned file.
        let file_name = resolved_path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("Input path has no file name: {}", input_path))?;
        let destination_path = std::path::Path::new(&session_dir).join(file_name);

        let cancelled = self
            .emit_before_switch("resume", Some(&destination_path.to_string_lossy()))
            .await;
        if cancelled {
            return Ok(false);
        }

        // Copy the file to the session directory
        if destination_path != resolved_path {
            std::fs::copy(resolved_path, &destination_path).map_err(|e| e.to_string())?;
        }

        // Open the imported session
        let effective_cwd = cwd_override.unwrap_or(self.cwd());
        let session_manager = SessionManager::new(
            effective_cwd,
            &session_dir,
            Some(&destination_path.to_string_lossy()),
            true,
            None,
        );

        // Validate session cwd exists
        let session_cwd = session_manager.get_cwd().to_string();
        if !session_cwd.is_empty() && !std::path::Path::new(&session_cwd).exists() {
            return Err(format!(
                "Stored session working directory does not exist: {}",
                session_cwd
            ));
        }

        // Teardown current session
        self.teardown_current("resume").await;

        // Create new runtime via factory
        let result = (self.create_runtime)(CreateAgentSessionRuntimeParams {
            cwd: session_manager.get_cwd().to_string(),
            agent_dir: self.services.agent_dir.clone(),
            session_manager,
        })
        .await;

        // Apply new state
        self.apply(result);

        // Finish replacement
        self.finish_session_replacement().await;

        Ok(true)
    }

    /// Dispose the runtime, emitting session_shutdown to extensions.
    pub async fn dispose(mut self) {
        self.teardown_current("quit").await;
    }
}

// ============================================================================
// Helper: extract user message text from a message value
// ============================================================================

fn extract_user_message_text(message: &serde_json::Value) -> Option<String> {
    // Only extract text from user messages
    if message.get("role").and_then(|r| r.as_str()) != Some("user") {
        return None;
    }
    if let Some(content) = message.get("content") {
        if let Some(text) = content.as_str() {
            return Some(text.to_string());
        }
        if let Some(blocks) = content.as_array() {
            let texts: Vec<String> = blocks
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                        block.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            if !texts.is_empty() {
                return Some(texts.join(""));
            }
        }
    }
    None
}

/// Create the initial runtime from a runtime factory.
///
/// The same factory is stored on the returned AgentSessionRuntime and reused for
/// later /new, /resume, /fork, and import flows.
pub async fn create_agent_session_runtime(
    factory: CreateAgentSessionRuntimeFactory,
    params: CreateAgentSessionRuntimeParams,
) -> AgentSessionRuntime {
    let result = factory(params).await;
    AgentSessionRuntime::new(
        result.session,
        result.services,
        factory,
        result.diagnostics,
        result.model_fallback_message,
    )
}
