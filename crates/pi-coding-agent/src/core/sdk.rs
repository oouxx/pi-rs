use pi_agent_core::pi_ai_types::{Model, ThinkingLevel};
use pi_agent_core::types::{ConvertToLlmFn, StreamFn};

use std::sync::Arc;

use crate::core::agent_session::{AgentSession, AgentSessionConfig};
use crate::core::event_bus::EventBusController;
use crate::core::extensions::{ExtensionRegistry, ToolDefinition};
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
                let mut stream_opts = pi_agent_core::pi_ai::types::StreamOptions::default();
                stream_opts.signal = options.signal;
                stream_opts.api_key = options.api_key;
                stream_opts.headers = options.headers;
                stream_opts.session_id = options.session_id;

                let event_stream =
                    pi_agent_core::pi_ai::stream::stream(&model, &context, Some(stream_opts));

                let boxed: StreamResponse = Box::new(event_stream);

                Ok(boxed)
            })
        },
    )
}
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
    pub custom_tools: Option<Vec<ToolDefinition>>,
    /// Additional paths to extension files/directories.
    /// Extensions will also be auto-discovered from:
    ///   - {cwd}/.pi/extensions/
    ///   - {agentDir}/extensions/
    pub extension_paths: Vec<String>,
    /// If false, skip the extension RPC sidecar entirely.
    pub enable_extensions: bool,
    /// Pre-configured extension registry. When set, extensions are injected
    /// by the caller instead of being auto-discovered from disk.
    pub extension_registry: Option<ExtensionRegistry>,
    /// CLI provider override (from --provider / -P).
    pub cli_provider: Option<String>,
    /// CLI model override (from --model / -m).
    pub cli_model: Option<String>,
    /// Whether to persist session messages to a JSONL file on disk.
    /// Defaults to false (in-memory only).
    pub persist_session: bool,
    /// Optional session file path for JSONL persistence.
    /// If set, `persist_session` is implied true.
    pub session_file: Option<String>,
    /// Path to an existing session file to fork from.
    /// Creates a new session that copies all entries from the source.
    pub fork_from: Option<String>,
    /// Custom session directory (from --session-dir).
    pub session_dir: Option<String>,
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

// ============================================================================
// SDK Re-exports — pi-coding-agent public API for downstream consumers
//
// Use via:
//   use pi_coding_agent::sdk::prelude::*;
//
// This prelude groups the public API so that sdk.rs can keep its internal
// `use` imports (for function bodies) without name conflicts.
// ============================================================================

pub mod prelude {
    // ── Agent runtime ───────────────────────────────────────────────────────
    pub use crate::core::agent_session::{
        AgentSession, AgentSessionConfig, PromptOptions, SessionStats, TokenUsage,
    };

    // ── Session management ──────────────────────────────────────────────────
    pub use crate::core::session_manager::{
        build_session_context, derive_short_session_id, is_valid_session_file,
        list_sessions_concurrent, migrate_session_file, ModelInfo, NewSessionOptions,
        ReadonlySessionManager, SessionContext, SessionEntry, SessionHeader, SessionInfo,
        SessionListProgressCallback, SessionManager, SessionTreeNode,
    };

    // ── Extensions ──────────────────────────────────────────────────────────
    pub use crate::core::extensions::{
        CommandRegistry, EventResult, ExecResult, ExtensionAPI, ExtensionContext, ExtensionEvent,
        ExtensionRegistry, ExtensionUIContext, FlagRegistry, RegisteredCommand, RegisteredFlag,
        RegisteredShortcut, RegisteredTool, RuntimeHandle, SendMessageOptions,
        SendUserMessageOptions, ShortcutRegistry, ToolCallOutput, ToolDefinition, ToolInfo,
        ToolRegistry,
    };

    // ── Slash commands & skills & prompts ───────────────────────────────────
    pub use crate::core::prompt_templates::PromptTemplate;
    pub use crate::core::skills::Skill;
    pub use crate::core::slash_commands::{SlashCommandInfo, SlashCommandSource};

    // ── Tool types and factory functions ────────────────────────────────────
    pub use crate::core::tools::bash::create_bash_tool;
    pub use crate::core::tools::edit::create_edit_tool;
    pub use crate::core::tools::file_mutation_queue::with_file_mutation_queue;
    pub use crate::core::tools::find::create_find_tool;
    pub use crate::core::tools::grep::create_grep_tool;
    pub use crate::core::tools::ls::create_ls_tool;
    pub use crate::core::tools::path_utils::resolve_read_path;
    pub use crate::core::tools::read::create_read_tool;
    pub use crate::core::tools::tool_definition_wrapper::wrap_tool_definitions;
    pub use crate::core::tools::write::create_write_tool;
    pub use crate::core::tools::{
        create_coding_tools, create_read_only_tools, OutputAccumulator, OutputAccumulatorOptions,
        OutputSnapshot, ToolName, TruncationOptions, TruncationResult,
    };

    // ── Model registry & resolution ─────────────────────────────────────────
    pub use crate::core::model_registry::{
        builtin_models, ApiKeyResult, ModelRegistry, ModelRegistryEntry, ProviderConfig,
        ProviderConfigInput,
    };
    pub use crate::core::model_resolver::{find_initial_model, ScopedModel};

    // ── Settings ────────────────────────────────────────────────────────────
    pub use crate::core::settings_manager::{
        BranchSummarySettings, CompactionSettings, FileSettingsStorage, ImageSettings,
        MarkdownSettings, ProviderRetrySettings, RetrySettings, Settings, SettingsManager,
        SettingsScope, SettingsStorage, TerminalSettings, ThinkingBudgetsSettings, WarningSettings,
    };

    // ── Project trust & auth ────────────────────────────────────────────────
    pub use crate::core::auth_storage::{
        AuthCredential, AuthStorage, AuthStorageBackend, OAuthCredentials,
    };
    pub use crate::core::project_trust::{
        resolve_project_trusted, DefaultProjectTrust, ProjectTrustContext,
        ResolveProjectTrustedOptions,
    };
    pub use crate::core::trust_manager::{
        find_nearest_trust_entry, get_project_trust_options, get_project_trust_parent_path,
        has_trust_requiring_project_resources, ProjectTrustOption, ProjectTrustStore,
        ProjectTrustStoreEntry, ProjectTrustUpdate,
    };

    // ── Message pipeline ────────────────────────────────────────────────────
    pub use crate::core::messages::{
        bash_execution_to_text, convert_to_llm, normalize_ingested_message,
    };

    // ── System prompt ───────────────────────────────────────────────────────
    pub use crate::core::system_prompt::{
        build_system_prompt, BuildSystemPromptOptions, ContextFile, SkillInfo,
    };

    // ── Event bus ───────────────────────────────────────────────────────────
    pub use crate::core::event_bus::EventBusController;

    // ── Config helpers ──────────────────────────────────────────────────────
    pub use crate::config::{
        expand_tilde_path, get_agent_dir, get_auth_path, get_bin_dir, get_debug_log_path,
        get_models_path, get_prompts_dir, get_sessions_dir, get_settings_path, get_tools_dir,
        APP_NAME, APP_TITLE, CONFIG_DIR_NAME, PACKAGE_NAME, VERSION,
    };

    // ── Agent-core types (re-exported for convenience) ──────────────────────
    pub use pi_agent_core::types::AfterToolCallContext;
    pub use pi_agent_core::types::AfterToolCallResult;
    pub use pi_agent_core::types::AgentEvent;
    pub use pi_agent_core::types::AgentMessage;
    pub use pi_agent_core::types::AgentState;
    pub use pi_agent_core::types::AgentTool;
    pub use pi_agent_core::types::AgentToolResult;
    pub use pi_agent_core::types::BeforeToolCallContext;
    pub use pi_agent_core::types::BeforeToolCallResult;
    pub use pi_agent_core::types::ConvertToLlmFn;
    pub use pi_agent_core::types::StreamFn;
    pub use pi_agent_core::types::StreamFnOptions;

    /// Re-exports from agent_session_runtime (AgentSessionRuntime etc.).
    pub use crate::core::agent_session_runtime::*;
}

// ============================================================================

// ============================================================================

/// Collect prompt_guidelines from extension tools.
///
/// Must be called BEFORE wrapping the registry in Arc, because
/// `collect_tools()` requires `&mut self`.
pub fn collect_prompt_guidelines(
    registry: &mut crate::core::extensions::ExtensionRegistry,
) -> Option<Vec<String>> {
    let tools = registry.collect_tools();
    let mut guidelines: Vec<String> = Vec::new();
    for t in &tools {
        if let Some(gl) = &t.definition.prompt_guidelines {
            guidelines.extend(gl.iter().cloned());
        }
    }
    if guidelines.is_empty() {
        None
    } else {
        Some(guidelines)
    }
}

/// Create an AgentSession from resolved options.
///
/// This is the single entry point for session creation. It resolves the
/// model, thinking level, session manager, event bus, and extension registry
/// from the provided `CreateAgentSessionOptions`, then assembles the
/// `AgentSession`.
///
/// `create_agent_session_from_services()` (in `agent_session_services.rs`)
/// builds a complete `CreateAgentSessionOptions` and delegates here, so all
/// session-creation logic lives in one place.
pub async fn create_agent_session(
    mut options: CreateAgentSessionOptions,
) -> Result<(AgentSession, CreateAgentSessionResult), Box<dyn std::error::Error + Send + Sync>> {
    // Ensure API providers are registered before any LLM calls
    pi_agent_core::pi_ai::providers::register_builtins::register_built_in_api_providers();

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

    // Resolve the model. When the caller has already resolved a model (e.g.
    // `create_agent_session_from_services`), honor it directly and skip the
    // default resolution path. Otherwise resolve from CLI flags / scoped
    // models / settings, falling back to the first available model.
    let (model, thinking_level, fallback_message) = if let Some(m) = options.model.clone() {
        let tl = options
            .thinking_level
            .clone()
            .map(|t| match t.as_str() {
                "high" => "high".to_string(),
                "low" => "low".to_string(),
                _ => "medium".to_string(),
            })
            .unwrap_or_else(|| "medium".to_string());
        (m, tl, None)
    } else {
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
                // SAFETY: is_empty() check above guarantees at least one element
                available
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| unreachable!())
            }
        };

        let thinking_level = match initial_model.thinking_level.as_str() {
            "high" => "high".to_string(),
            "medium" => "medium".to_string(),
            "low" => "low".to_string(),
            _ => "medium".to_string(),
        };

        (model, thinking_level, initial_model.fallback_message)
    };

    // Resolve session directory: --session-dir overrides default
    let session_dir = options
        .session_dir
        .clone()
        .unwrap_or_else(|| SessionManager::default_session_dir(&cwd, &agent_dir));

    // Create or restore session manager
    let session_manager = if let Some(ref fork_path) = options.fork_from {
        SessionManager::fork_from(fork_path, &cwd, Some(&session_dir), None)
            .map_err(|e| format!("Failed to fork session: {e}"))?
    } else {
        let persist = options.persist_session || options.session_file.is_some();
        SessionManager::new(
            &cwd,
            &session_dir,
            options.session_file.as_deref(),
            persist,
            None,
        )
    };

    let event_bus = EventBusController::new();

    // ── Extension registry (Rust native extensions) ───────────────────
    let mut extension_registry = options
        .extension_registry
        .take()
        .unwrap_or_else(ExtensionRegistry::new);
    // Collect prompt_guidelines BEFORE wrapping in Arc
    // (collect_tools() requires &mut self, which Arc doesn't provide).
    let prompt_guidelines = collect_prompt_guidelines(&mut extension_registry);
    let extension_registry_arc = std::sync::Arc::new(extension_registry);

    // Dispatch session_start to extensions before session creation.
    let ext_ctx = crate::core::extensions::ExtensionContext::new(
        cwd.clone(),
        false,
        crate::core::extensions::ExtensionUIContext {
            notify: std::sync::Arc::new(|msg, _level| eprintln!("[pi] {msg}")),
            set_status: std::sync::Arc::new(|_key, _value| {}),
            confirm: std::sync::Arc::new(|_title, _msg| false),
        },
        crate::core::extensions::RuntimeHandle::noop(),
    );
    crate::core::extensions::dispatcher::dispatch_session_start(
        &extension_registry_arc,
        "startup",
        &ext_ctx,
    )
    .await;

    // Load resources for context files and skills
    let resource_options = ResourceLoaderOptions {
        cwd: cwd.clone(),
        agent_dir: Some(agent_dir.clone()),
        include_defaults: true,
        ..Default::default()
    };
    let resources = resource_loader::load_all_resources(&resource_options);

    let context_files: Vec<ContextFile> = resources
        .clone()
        .context_files
        .into_iter()
        .map(|cf| ContextFile {
            path: cf.path,
            content: cf.content,
        })
        .collect();

    let skills: Vec<SkillInfo> = resources
        .clone()
        .skills
        .into_iter()
        .map(|s| SkillInfo {
            name: s.name,
            description: s.description,
            file_path: s.file_path,
            base_dir: s.base_dir,
        })
        .collect();

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

    let session_options = AgentSessionConfig {
        cwd: cwd.clone(),
        model,
        thinking_level,
        custom_prompt: options.custom_prompt,
        append_system_prompt: options.append_system_prompt,
        selected_tools: options.tools,
        tool_snippets: None,
        prompt_guidelines,
        context_files,
        skills,
        session_name: options.session_name,
        stream_fn: options
            .stream_fn
            .or_else(|| Some(create_default_stream_fn())),
        convert_to_llm: options.convert_to_llm,
        initial_active_tool_names: Some(initial_active_tool_names),
        allowed_tool_names,
        excluded_tool_names,
        extension_registry: Some(extension_registry_arc),
        resources: Some(resources),
        custom_tools: options.custom_tools,
    };

    let session =
        AgentSession::new(session_manager, event_bus, model_registry, session_options).await;

    // Load persisted messages into agent state if restoring from a session file
    if session.get_session_manager().get_session_file().is_some() {
        let count = session.load_messages_from_session().await;
        if count > 0 {
            eprintln!("[pi] Restored {count} messages from session file");
        }
    }

    Ok((
        session,
        CreateAgentSessionResult {
            model_fallback_message: fallback_message,
        },
    ))
}
