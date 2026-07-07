use pi_agent_core::pi_ai_types::{Model, ThinkingLevel};
use pi_agent_core::types::{ConvertToLlmFn, StreamFn};

use std::sync::Arc;

use crate::core::agent_session::{AgentSession, AgentSessionOptions};
use crate::core::event_bus::EventBusController;
use crate::core::extensions::{ExtensionsRpcClient, ToolInfo};
use crate::core::model_registry::ModelRegistry;
use crate::core::model_resolver::{self, ScopedModel};
use crate::core::resource_loader::{self, ResourceLoaderOptions};
use crate::core::session_manager::SessionManager;
use crate::core::settings_manager::SettingsManager;
use crate::core::system_prompt::{ContextFile, SkillInfo};

/// Create the default StreamFn that bridges to the pi-ai provider system.
/// Public for testing.
pub fn create_default_stream_fn() -> pi_agent_core::types::StreamFn {
    use pi_agent_core::pi_ai_types::StreamResponse;

    std::sync::Arc::new(
        |model: pi_agent_core::pi_ai_types::Model,
         context: pi_agent_core::pi_ai_types::Context,
         _thinking: Option<pi_agent_core::pi_ai_types::ThinkingLevel>,
         options: pi_agent_core::types::StreamFnOptions| {
            Box::pin(async move {
                let mut stream_opts = pi_ai::types::StreamOptions::default();
                stream_opts.signal = options.signal;
                stream_opts.api_key = options.api_key;
                stream_opts.headers = options.headers;
                stream_opts.session_id = options.session_id;

                let event_stream =
                    pi_ai::stream::stream(&model, &context, Some(stream_opts));

                let boxed: StreamResponse =
                    Box::new(event_stream);

                Ok(boxed)
            })
        },
    )
}

#[derive(Clone)]
pub struct CreateAgentSessionOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
    pub scoped_models: Option<Vec<(Model, Option<ThinkingLevel>)>>,
    pub no_tools: Option<NoToolsMode>,
    pub tools: Option<Vec<String>>,
    pub exclude_tools: Option<Vec<String>>,
    pub custom_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub session_name: Option<String>,
    pub stream_fn: Option<StreamFn>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    /// Additional paths to extension files/directories.
    /// Extensions will also be auto-discovered from:
    ///   - {cwd}/.pi/extensions/
    ///   - {agentDir}/extensions/
    pub extension_paths: Vec<String>,
    /// If false, skip the extension RPC sidecar entirely.
    pub enable_extensions: bool,
    /// CLI provider override (from --provider / -P).
    pub cli_provider: Option<String>,
    /// CLI model override (from --model / -m).
    pub cli_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoToolsMode {
    All,
    Builtin,
}

#[derive(Debug, Clone)]
pub struct CreateAgentSessionResult {
    pub model_fallback_message: Option<String>,
}

pub async fn create_agent_session(
    options: CreateAgentSessionOptions,
) -> Result<(AgentSession, CreateAgentSessionResult), Box<dyn std::error::Error + Send + Sync>> {
    // Ensure API providers are registered before any LLM calls
    pi_ai::providers::register_builtins::register_built_in_api_providers();

    let cwd = options.cwd.clone();
    let agent_dir = options
        .agent_dir
        .clone()
        .unwrap_or_else(|| crate::config::get_agent_dir().to_string_lossy().to_string());

    let settings_manager = SettingsManager::create(&cwd, Some(&agent_dir));
    let model_registry = ModelRegistry::new(ModelRegistry::builtin_models_list());

    let default_provider = settings_manager.get_settings().default_provider.clone();
    let default_model_id = settings_manager.get_settings().default_model.clone();
    let default_thinking_level = settings_manager.get_settings().thinking_level.clone();

    let scoped = options
        .scoped_models
        .as_ref()
        .map(|models| {
            models
                .iter()
                .map(|(m, tl)| ScopedModel {
                    model: m.clone(),
                    thinking_level: tl.as_ref().map(|t| format!("{:?}", t).to_lowercase()),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let initial_model = model_resolver::find_initial_model(
        options.cli_provider.as_deref(),
        options.cli_model.as_deref(),
        &scoped,
        false,
        default_provider.as_deref(),
        default_model_id.as_deref(),
        default_thinking_level.as_deref(),
        &model_registry,
    );

    let model = match initial_model.model {
        Some(m) => m,
        None => {
            let available = model_registry.get_available();
            if available.is_empty() {
                return Err("No models available. Please configure an API key.".into());
            }
            available.into_iter().next().unwrap()
        }
    };

    let thinking_level = match initial_model.thinking_level.as_str() {
        "high" => "high".to_string(),
        "medium" => "medium".to_string(),
        "low" => "low".to_string(),
        _ => "medium".to_string(),
    };

    let resource_options = ResourceLoaderOptions {
        cwd: cwd.clone(),
        agent_dir: Some(agent_dir.clone()),
        include_defaults: true,
        ..Default::default()
    };
    let resources = resource_loader::load_all_resources(&resource_options);

    let context_files: Vec<ContextFile> = resources
        .context_files
        .into_iter()
        .map(|cf| ContextFile {
            path: cf.path,
            content: cf.content,
        })
        .collect();

    let skills: Vec<SkillInfo> = resources
        .skills
        .into_iter()
        .map(|s| SkillInfo {
            name: s.name,
            description: s.description,
            instructions: s.instructions,
            tools: s.tools,
        })
        .collect();

    let session_dir = SessionManager::default_session_dir(&cwd, &agent_dir);
    let session_manager = SessionManager::new(&cwd, &session_dir, None, false, None);

    let event_bus = EventBusController::new();

    let default_active_tool_names: Vec<String> = match options.no_tools {
        Some(NoToolsMode::All) => Vec::new(),
        Some(NoToolsMode::Builtin) => Vec::new(),
        None => vec![
            "read".to_string(),
            "bash".to_string(),
            "edit".to_string(),
            "write".to_string(),
        ],
    };

    let initial_active_tool_names = options.tools.clone().unwrap_or(default_active_tool_names);

    let allowed_tool_names = options.tools.clone();
    let excluded_tool_names = options.exclude_tools.clone();

    // ── Extension RPC sidecar ──────────────────────────────────────────
    let mut extension_tools: Vec<ToolInfo> = Vec::new();
    let mut rpc_client: Option<Arc<ExtensionsRpcClient>> = None;

    if options.enable_extensions && ExtensionsRpcClient::is_available() {
        let client = Arc::new(ExtensionsRpcClient::new());
        match client.start().await {
            Ok(()) => {
                match client
                    .load_extensions(&cwd, &agent_dir, &options.extension_paths)
                    .await
                {
                    Ok(result) => {
                        extension_tools = result.tools;
                        if !result.errors.is_empty() {
                            for err in &result.errors {
                                eprintln!("[pi] Extension load warning: {} — {}", err.path, err.error);
                            }
                        }
                        rpc_client = Some(client);
                    }
                    Err(e) => {
                        eprintln!("[pi] Extension RPC error: {e}");
                        let _ = client.stop().await;
                    }
                }
            }
            Err(e) => {
                // Sidecar not available (bun not installed or path issue) — skip silently
                let _ = e;
            }
        }
    }

    let session_options = AgentSessionOptions {
        cwd: cwd.clone(),
        model,
        thinking_level,
        custom_prompt: options.custom_prompt,
        append_system_prompt: options.append_system_prompt,
        selected_tools: options.tools,
        tool_snippets: None,
        prompt_guidelines: None,
        context_files,
        skills,
        session_name: options.session_name,
        stream_fn: options.stream_fn.or_else(|| Some(create_default_stream_fn())),
        convert_to_llm: options.convert_to_llm,
        initial_active_tool_names: Some(initial_active_tool_names),
        allowed_tool_names,
        excluded_tool_names,
        extension_tools,
        rpc_client,
    };

    let session = AgentSession::new(session_manager, event_bus, model_registry, session_options);

    Ok((
        session,
        CreateAgentSessionResult {
            model_fallback_message: initial_model.fallback_message,
        },
    ))
}
