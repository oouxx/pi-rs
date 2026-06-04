use std::path::Path;

use crate::core::auth_storage::AuthStorage;
use crate::core::model_registry::ModelRegistry;
use crate::core::resource_loader::DefaultResourceLoader;
use crate::core::session_manager::SessionManager;
use crate::core::settings_manager::SettingsManager;

// ---------------------------------------------------------------------------
// Diagnostics collected during service creation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AgentSessionRuntimeDiagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

// ---------------------------------------------------------------------------
// Cwd-bound runtime services for one session
// ---------------------------------------------------------------------------

pub struct AgentSessionServices {
    pub cwd: String,
    pub agent_dir: String,
    pub auth_storage: AuthStorage,
    pub settings_manager: SettingsManager,
    pub model_registry: ModelRegistry,
    pub resource_loader: DefaultResourceLoader,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

impl AgentSessionServices {
    pub fn new(
        cwd: String,
        agent_dir: String,
        auth_storage: AuthStorage,
        settings_manager: SettingsManager,
        model_registry: ModelRegistry,
        resource_loader: DefaultResourceLoader,
        diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    ) -> Self {
        Self {
            cwd,
            agent_dir,
            auth_storage,
            settings_manager,
            model_registry,
            resource_loader,
            diagnostics,
        }
    }
}

fn default_agent_dir() -> String {
    dirs::home_dir()
        .map(|h| h.join(".pi").to_string_lossy().to_string())
        .unwrap_or_else(|| ".pi".to_string())
}

/// Create cwd-bound runtime services.
pub async fn create_agent_session_services(
    cwd: &str,
    agent_dir: Option<&str>,
) -> AgentSessionServices {
    let agent_dir = agent_dir
        .map(|s| s.to_string())
        .unwrap_or_else(default_agent_dir);

    let auth_storage = AuthStorage::create(Path::new(&agent_dir).join("auth.json"));
    let settings_manager = SettingsManager::create(cwd, &agent_dir);
    let model_registry = ModelRegistry::create(
        auth_storage,
        Path::new(&agent_dir).join("models.json").to_string_lossy().to_string(),
    );
    let resource_loader = DefaultResourceLoader::new(cwd, &agent_dir, &settings_manager);
    resource_loader.reload().await;

    AgentSessionServices {
        cwd: cwd.to_string(),
        agent_dir,
        auth_storage,
        settings_manager,
        model_registry,
        resource_loader,
        diagnostics: Vec::new(),
    }
}

/// Create an AgentSession from already-created services.
pub async fn create_agent_session_from_services(
    services: &AgentSessionServices,
    session_manager: SessionManager,
) -> Result<String, String> {
    let _ = services;
    let _ = session_manager;
    Err("create_agent_session not yet fully wired".to_string())
}
