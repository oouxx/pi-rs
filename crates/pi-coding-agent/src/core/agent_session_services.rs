use std::path::Path;

use crate::core::auth_storage::AuthStorage;
use crate::core::model_registry::ModelRegistry;
use crate::core::resource_loader::{self, LoadedResources, ResourceLoaderOptions};
use crate::core::session_manager::SessionManager;
use crate::core::settings_manager::SettingsManager;

pub use crate::core::diagnostics::ResourceDiagnostic as AgentSessionRuntimeDiagnostic;

// ============================================================================
// AgentSessionServices
// ============================================================================

/// Coherent cwd-bound runtime services for one effective session cwd.
///
/// This is infrastructure only. The AgentSession itself is created separately so
/// session options can be resolved against these services first.
pub struct AgentSessionServices {
    pub cwd: String,
    pub agent_dir: String,
    pub auth_storage: AuthStorage,
    pub settings_manager: SettingsManager,
    pub model_registry: ModelRegistry,
    pub resources: LoadedResources,
    pub diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
}

impl AgentSessionServices {
    pub fn new(
        cwd: String,
        agent_dir: String,
        auth_storage: AuthStorage,
        settings_manager: SettingsManager,
        model_registry: ModelRegistry,
        resources: LoadedResources,
        diagnostics: Vec<AgentSessionRuntimeDiagnostic>,
    ) -> Self {
        Self {
            cwd,
            agent_dir,
            auth_storage,
            settings_manager,
            model_registry,
            resources,
            diagnostics,
        }
    }
}

// ============================================================================
// Options
// ============================================================================

/// Inputs for creating cwd-bound runtime services.
pub struct CreateAgentSessionServicesOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub auth_storage: Option<AuthStorage>,
    pub settings_manager: Option<SettingsManager>,
    pub model_registry: Option<ModelRegistry>,
    pub resource_loader_options: Option<ResourceLoaderOptions>,
}

/// Inputs for creating an AgentSession from already-created services.
pub struct CreateAgentSessionFromServicesOptions {
    pub services: AgentSessionServices,
    pub session_manager: SessionManager,
}

// ============================================================================
// Factory functions
// ============================================================================

fn default_agent_dir() -> String {
    dirs::home_dir()
        .map(|h| h.join(".pi").to_string_lossy().to_string())
        .unwrap_or_else(|| ".pi".to_string())
}

/// Create cwd-bound runtime services.
///
/// Returns services plus diagnostics. It does not create an AgentSession.
pub async fn create_agent_session_services(
    options: CreateAgentSessionServicesOptions,
) -> AgentSessionServices {
    let cwd = options.cwd;
    let agent_dir = options
        .agent_dir
        .unwrap_or_else(default_agent_dir);

    let auth_storage = options.auth_storage.unwrap_or_else(|| {
        AuthStorage::create(Path::new(&agent_dir).join("auth.json"))
    });

    let settings_manager = options.settings_manager.unwrap_or_else(|| {
        SettingsManager::create(&cwd, Some(&agent_dir))
    });

    let model_registry = options.model_registry.unwrap_or_else(|| {
        ModelRegistry::new(vec![])
    });

    let resource_opts = options.resource_loader_options.unwrap_or_else(|| {
        ResourceLoaderOptions {
            cwd: cwd.clone(),
            agent_dir: Some(agent_dir.clone()),
            include_defaults: true,
            ..Default::default()
        }
    });

    let resources = resource_loader::load_all_resources(&resource_opts);
    let diagnostics: Vec<AgentSessionRuntimeDiagnostic> = resources
        .diagnostics
        .iter()
        .map(|d| match d {
            crate::core::diagnostics::ResourceDiagnostic::Warning { message, path } => {
                AgentSessionRuntimeDiagnostic::Warning {
                    message: format!("{}: {}", message, path),
                    path: path.clone(),
                }
            }
            crate::core::diagnostics::ResourceDiagnostic::Collision {
                message,
                path,
                collision: _,
            } => AgentSessionRuntimeDiagnostic::Warning {
                message: message.clone(),
                path: path.clone(),
            },
        })
        .collect();

    AgentSessionServices {
        cwd,
        agent_dir,
        auth_storage,
        settings_manager,
        model_registry,
        resources,
        diagnostics,
    }
}

/// Create an AgentSession from previously created services.
///
/// This keeps session creation separate from service creation so callers can
/// resolve model, thinking, tools, and other session inputs against the target
/// cwd before constructing the session.
pub async fn create_agent_session_from_services(
    options: CreateAgentSessionFromServicesOptions,
) -> Result<String, String> {
    let _services = options.services;
    let _session_manager = options.session_manager;
    // TODO: wire up full AgentSession creation from services
    Err("create_agent_session_from_services not yet fully wired".to_string())
}
