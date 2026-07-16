use std::path::Path;
use std::sync::Arc;

use crate::core::agent_session::AgentSession;
use crate::core::auth_storage::AuthStorage;
use crate::core::event_bus::EventBusController;
use crate::core::extensions::ExtensionRegistry;
use crate::core::model_registry::ModelRegistry;
use crate::core::resource_loader::{self, LoadedResources, ResourceLoaderOptions};
use crate::core::sdk::{CreateAgentSessionResult, NoToolsMode};
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
    /// Create a new AgentSessionServices instance.
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
    pub model: Option<pi_agent_core::pi_ai_types::Model>,
    pub thinking_level: Option<pi_agent_core::pi_ai_types::ThinkingLevel>,
    pub scoped_models: Option<Vec<(pi_agent_core::pi_ai_types::Model, Option<pi_agent_core::pi_ai_types::ThinkingLevel>)>>,
    pub tools: Option<Vec<String>>,
    pub no_tools: Option<NoToolsMode>,
    /// Pre-configured extension registry. When set, extensions are injected
    /// by the caller instead of being auto-discovered from disk.
    pub extension_registry: Option<ExtensionRegistry>,
    /// Model fallback message, propagated from model resolution.
    pub fallback_message: Option<String>,
}

// ============================================================================
// Factory functions
// ============================================================================

fn default_agent_dir() -> String {
    dirs::home_dir()
        .map(|h| h.join(crate::config::CONFIG_DIR_NAME).to_string_lossy().to_string())
        .unwrap_or_else(|| crate::config::CONFIG_DIR_NAME.to_string())
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
) -> Result<(AgentSession, CreateAgentSessionResult), Box<dyn std::error::Error + Send + Sync>> {
    let services = options.services;
    let session_manager = options.session_manager;

    // Resolve model: use provided model, or fall back to the first available
    // model from the registry
    let model = match options.model {
        Some(m) => m,
        None => {
            let available = services.model_registry.get_available();
            if available.is_empty() {
                return Err("No models available. Please configure an API key.".into());
            }
            // SAFETY: is_empty() check above guarantees at least one element
            available.into_iter().next().unwrap_or_else(|| unreachable!())
        }
    };

    let thinking_level = match options.thinking_level {
        Some(t) => t,
        None => "medium".to_string(),
    };

    let event_bus = EventBusController::new();

    // Use caller-provided extension registry, or create an empty one
    let mut extension_registry = options.extension_registry.unwrap_or_else(ExtensionRegistry::new);
    // Collect prompt_guidelines BEFORE wrapping in Arc
    let prompt_guidelines = crate::core::sdk::collect_prompt_guidelines(&mut extension_registry);
    let extension_registry = Arc::new(extension_registry);

    // Build the options struct for the inner creation function
    let sdk_options = crate::core::sdk::CreateAgentSessionOptions {
        cwd: services.cwd.clone(),
        agent_dir: Some(services.agent_dir.clone()),
        model: Some(model.clone()),
        thinking_level: Some(thinking_level.clone()),
        scoped_models: options.scoped_models,
        no_tools: options.no_tools,
        tools: options.tools,
        exclude_tools: None,
        custom_prompt: None,
        append_system_prompt: None,
        session_name: None,
        stream_fn: None,
        convert_to_llm: None,
        extension_paths: Vec::new(),
        enable_extensions: false,
        extension_registry: None,
        cli_provider: None,
        cli_model: None,
        persist_session: false,
        session_file: None,
        fork_from: None,
        session_dir: None,
        custom_tools: None,
    };

    let (session, result) = crate::core::sdk::create_agent_session_inner(
        crate::core::sdk::CreateAgentSessionInnerParams {
            cwd: services.cwd,
            agent_dir: services.agent_dir,
            model,
            thinking_level,
            model_registry: services.model_registry,
            session_manager,
            event_bus,
            extension_registry,
            options: sdk_options,
            fallback_message: options.fallback_message,
            prompt_guidelines,
        },
    )
    .await;

    Ok((session, result))
}
