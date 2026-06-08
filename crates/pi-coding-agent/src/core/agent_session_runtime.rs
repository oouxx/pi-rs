use crate::core::agent_session_services::{AgentSessionRuntimeDiagnostic, AgentSessionServices};
use crate::core::session_manager::SessionManager;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Return type for runtime creation
// ---------------------------------------------------------------------------

pub struct CreateAgentSessionRuntimeResult {
    pub session: String,
    pub services: AgentSessionServices,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    pub model_fallback_message: Option<String>,
}

// ---------------------------------------------------------------------------
// Runtime factory type
// ---------------------------------------------------------------------------

pub type CreateAgentSessionRuntimeFactory = Box<
    dyn Fn(
            CreateAgentSessionRuntimeParams,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = CreateAgentSessionRuntimeResult> + Send>,
        > + Send
        + Sync,
>;

pub struct CreateAgentSessionRuntimeParams {
    pub cwd: String,
    pub agent_dir: String,
    pub session_manager: SessionManager,
}

// ---------------------------------------------------------------------------
// AgentSessionRuntime
// ---------------------------------------------------------------------------

pub struct AgentSessionRuntime {
    session: String,
    services: AgentSessionServices,
    diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    model_fallback_message: Option<String>,
}

impl AgentSessionRuntime {
    pub fn new(
        session: String,
        services: AgentSessionServices,
        diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
        model_fallback_message: Option<String>,
    ) -> Self {
        Self {
            session,
            services,
            diagnostics,
            model_fallback_message,
        }
    }

    pub fn services(&self) -> &AgentSessionServices {
        &self.services
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn diagnostics(&self) -> &[AgentSessionRuntimeDiagnostic] {
        &self.diagnostics
    }

    pub fn model_fallback_message(&self) -> Option<&str> {
        self.model_fallback_message.as_deref()
    }
}

/// Create the initial runtime from a runtime factory.
pub async fn create_agent_session_runtime(
    factory: CreateAgentSessionRuntimeFactory,
    params: CreateAgentSessionRuntimeParams,
) -> AgentSessionRuntime {
    let result = factory(params).await;
    AgentSessionRuntime::new(
        result.session,
        result.services,
        result.diagnostics,
        result.model_fallback_message,
    )
}
