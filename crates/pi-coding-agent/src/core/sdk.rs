use pi_agent_core::pi_ai_types::{Model, ThinkingLevel};
use pi_agent_core::types::{ConvertToLlmFn, StreamFn};


use crate::core::agent_session::{AgentSession, AgentSessionConfig};
use crate::core::tools::ToolsOptions;
use crate::core::extensions::{ExtensionRegistry, ToolDefinition};
use crate::core::model_registry::ModelRegistry;
use crate::core::model_resolver::{self, ScopedModel};
use crate::core::auth_storage::AuthStorage;
use crate::core::resource_loader::{self, ResourceLoaderOptions};
use crate::core::session_manager::{SessionEntry, SessionManager};
use crate::core::settings_manager::SettingsManager;
use crate::core::system_prompt::{ContextFile, SkillInfo};

/// Create the default StreamFn that bridges to the pi-ai provider system.
/// Public for testing.
pub fn create_default_stream_fn(has_telemetry: bool) -> pi_agent_core::types::StreamFn {
    use pi_agent_core::pi_ai_types::StreamResponse;

    std::sync::Arc::new(
        move |model: pi_agent_core::pi_ai_types::Model,
         context: pi_agent_core::pi_ai_types::Context,
         thinking: Option<pi_agent_core::pi_ai_types::ThinkingLevel>,
         options: pi_agent_core::types::StreamFnOptions| {
            Box::pin(async move {
                // Provider attribution headers (match TS `transformHeaders` →
                // `mergeProviderAttributionHeaders`): `x-opencode-session` /
                // `x-opencode-client` session affinity plus the telemetry-gated
                // OpenRouter/NVIDIA/Cloudflare markers. Request headers override.
                let request_headers: Vec<(String, String)> =
                    options.headers.clone().unwrap_or_default().into_iter().collect();
                let headers = crate::core::provider_attribution::merge_provider_attribution_headers(
                    &crate::core::provider_attribution::ModelInfo {
                        provider: model.provider.clone(),
                        base_url: model.base_url.clone(),
                    },
                    has_telemetry,
                    options.session_id.as_deref(),
                    &[request_headers],
                )
                .map(|entries| entries.into_iter().collect::<std::collections::HashMap<_, _>>());

                // Forward every request-affecting field (match TS `streamSimple`
                // options spread). Dropping these silently lost temperature,
                // maxTokens, timeouts, retries, cache retention, tool choice,
                // service tier, metadata, thinking budgets and the reasoning level.
                let transport = options.transport.as_ref().and_then(|t| {
                    serde_json::from_value::<pi_agent_core::pi_ai::types::Transport>(
                        serde_json::json!(t),
                    )
                    .ok()
                });
                let stream_opts = pi_agent_core::pi_ai::types::StreamOptions {
                    temperature: options.temperature,
                    // Default the output cap to the model's max, clamped to the
                    // context window (match TS `buildBaseOptions`).
                    max_tokens: options.max_tokens.or_else(|| {
                        Some(
                            pi_agent_core::pi_ai::providers::simple_options::clamp_max_tokens_to_context(
                                &model,
                                &context,
                                model.max_tokens,
                            ),
                        )
                    }),
                    signal: options.signal,
                    api_key: options.api_key,
                    transport,
                    cache_retention: options.cache_retention,
                    session_id: options.session_id,
                    headers,
                    timeout_ms: options.timeout_ms,
                    websocket_connect_timeout_ms: options.websocket_connect_timeout_ms,
                    max_retries: options.max_retries,
                    max_retry_delay_ms: options.max_retry_delay_ms,
                    metadata: options.metadata,
                    env: options.env,
                    tool_choice: options.tool_choice,
                    service_tier: options.service_tier,
                    reasoning_effort: thinking,
                    thinking_budgets: options.thinking_budgets,
                    on_payload: options.on_payload,
                    on_headers: options.on_headers,
                    on_provider_response: options.on_provider_response,
                    ..Default::default()
                };



                let event_stream =
                    pi_agent_core::pi_ai::stream::stream(&model, &context, Some(stream_opts));

                let boxed: StreamResponse = Box::new(event_stream);

                Ok(boxed)
            })
        },
    )
}
/// Why a session start event was fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStartReason {
    /// Initial startup.
    Startup,
    /// Session reload (e.g. config change).
    Reload,
    /// Brand new session.
    New,
    /// Resuming an existing session.
    Resume,
    /// Forked from another session.
    Fork,
}

impl std::fmt::Display for SessionStartReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Startup => write!(f, "startup"),
            Self::Reload => write!(f, "reload"),
            Self::New => write!(f, "new"),
            Self::Resume => write!(f, "resume"),
            Self::Fork => write!(f, "fork"),
        }
    }
}

/// Fired when a session is started, loaded, or reloaded.
/// Mirrors the TypeScript `SessionStartEvent` interface.
#[derive(Debug, Clone)]
pub struct SessionStartEvent {
    /// Why this session start happened.
    pub reason: SessionStartReason,
    /// Previously active session file. Present for "new", "resume", and "fork".
    pub previous_session_file: Option<String>,
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
    /// Per-tool options (e.g. bash default timeout). `None` = defaults.
    pub tools_options: Option<ToolsOptions>,
    /// CLI-passed extension flag values (e.g. `--my-flag value`). Sent to the
    /// JS extension runtime so `pi.getFlag()` sees them.
    pub extension_flags: Option<std::collections::HashMap<String, String>>,
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
    /// Pre-configured auth storage. When set, used instead of creating a new one.
    pub auth_storage: Option<AuthStorage>,
    /// Pre-configured model registry. When set, used instead of creating a new one.
    pub model_registry: Option<ModelRegistry>,
    /// Custom resource loader options. When set, used instead of defaults.
    pub resource_loader: Option<ResourceLoaderOptions>,
    /// Pre-configured session manager. When set, used instead of creating a new one.
    /// Takes precedence over `session_file`, `fork_from`, and `session_dir`.
    pub session_manager: Option<SessionManager>,
    /// Pre-configured settings manager. When set, used instead of creating a new one.
    pub settings_manager: Option<SettingsManager>,
    /// Session start event metadata for extension runtime startup.
    /// When set, used instead of the default "startup" reason.
    pub session_start_event: Option<SessionStartEvent>,
    /// Extension UI context. When set, used instead of the default no-op
    /// context (headless modes wire dialogs/notifications to their client).
    pub ui_context: Option<crate::core::extensions::ExtensionUIContext>,
}

impl Default for CreateAgentSessionOptions {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string()),
            agent_dir: None,
            model: None,
            thinking_level: None,
            scoped_models: None,
            no_tools: None,
            tools: None,
            exclude_tools: None,
            custom_prompt: None,
            append_system_prompt: None,
            session_name: None,
            stream_fn: None,
            convert_to_llm: None,
            custom_tools: None,
            tools_options: None,
            extension_flags: None,
            extension_paths: Vec::new(),
            enable_extensions: true,
            extension_registry: None,
            cli_provider: None,
            cli_model: None,
            persist_session: false,
            session_file: None,
            fork_from: None,
            session_dir: None,
            auth_storage: None,
            model_registry: None,
            resource_loader: None,
            session_manager: None,
            settings_manager: None,
            session_start_event: None,
        ui_context: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoToolsMode {
    All,
    Builtin,
}

pub struct CreateAgentSessionResult {
    pub model_fallback_message: Option<String>,
    /// Loaded extensions result. Populated when extensions are enabled.
    pub extensions_result: Option<ExtensionsResult>,
}

impl std::fmt::Debug for CreateAgentSessionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateAgentSessionResult")
            .field("model_fallback_message", &self.model_fallback_message)
            .field("extensions_result", &self.extensions_result)
            .finish()
    }
}

/// Result of loading extensions.
/// Mirrors the TypeScript `LoadExtensionsResult` interface.
#[derive(Debug, Clone)]
pub struct ExtensionsResult {
    /// Loaded extension names.
    pub extensions: Vec<String>,
    /// Errors encountered during extension loading.
    pub errors: Vec<ExtensionError>,
}

/// An error encountered during extension loading.
#[derive(Debug, Clone)]
pub struct ExtensionError {
    /// Path to the extension that failed to load.
    pub path: String,
    /// Error message.
    pub error: String,
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
        CommandRegistry, EventPublisher, ExtensionContext, ExtensionRegistry, ExtensionUIContext,
        FlagRegistry, HookHandler, HookResult, HookRunner, RegisteredCommand, RegisteredFlag,
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

    // ── Config helpers ──────────────────────────────────────────────────────
    pub use crate::config::{
        expand_tilde_path, get_agent_dir, get_auth_path, get_bin_dir, get_debug_log_path,
        get_models_path, get_prompts_dir, get_sessions_dir, get_settings_path, get_tools_dir,
        APP_NAME, APP_TITLE, CONFIG_DIR_NAME, PACKAGE_NAME, VERSION,
    };

    // ── Agent-core types (re-exported for convenience) ──────────────────────
    /// Type alias matching the TypeScript `Tool` export.
    pub use pi_agent_core::types::AgentTool as Tool;
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
    registry: &crate::core::extensions::ExtensionRegistry,
) -> Option<Vec<String>> {
    let tools = registry.tools();
    let mut guidelines: Vec<String> = Vec::new();
    for t in tools {
        if let Some(gl) = &t.definition.prompt_guidelines {
            for g in gl { guidelines.push(g.clone()); }
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

    let settings_manager = options
        .settings_manager
        .take()
        .unwrap_or_else(|| SettingsManager::create(&cwd, Some(&agent_dir)));
    let mut model_registry = match options.model_registry.take() {
        Some(r) => r,
        None => {
            // 内置模型列表（与 TS 原版一致，无 Ollama 自动发现；本地/
            // 自定义 OpenAI 兼容端点通过 models.json 配置）。
            let builtins = ModelRegistry::builtin_models_list();
            ModelRegistry::new(builtins)
        }
    };
    // Stored credentials (`/login` → auth.json) must be visible to auth checks
    // and requests, exactly like TS `RuntimeCredentials` → `Models.getAuth`.
    model_registry.wire_auth_resolver(std::path::Path::new(&agent_dir).join("auth.json"));

    let default_provider = settings_manager.get_settings().default_provider.clone();
    let default_model_id = settings_manager.get_settings().default_model.clone();
    let default_thinking_level = settings_manager.get_settings().default_thinking_level.clone();

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

    // Resolve session directory: --session-dir overrides default. The default
    // is the encoded-cwd subdirectory (`sessions/--<encoded-cwd>--`), matching
    // TS `getDefaultSessionDir`. Using the bare `sessions/` root here made
    // `usesDefaultSessionDir()` never true (so the resume hint always printed
    // `--session-dir`) and made id lookups miss sessions created by default.
    let session_dir = options.session_dir.clone().unwrap_or_else(|| {
        crate::config::get_default_session_dir(&cwd, Some(&agent_dir))
            .to_string_lossy()
            .to_string()
    });

    // Create or restore session manager
    let session_manager = if let Some(sm) = options.session_manager {
        sm
    } else if let Some(ref fork_path) = options.fork_from {
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

    // Existing session data drives both model restore and the `isContinuing`
    // flag passed to `find_initial_model` (TS sdk.ts `buildSessionContext()` →
    // `hasExistingSession` / `hasThinkingEntry`).
    let existing_session = session_manager.build_context();
    let has_existing_session = !existing_session.messages.is_empty();
    let has_thinking_entry = session_manager
        .get_branch(None)
        .iter()
        .any(|entry| matches!(entry, SessionEntry::ThinkingLevelChange { .. }));

    // Resolve the model. When the caller has already resolved a model (e.g.
    // `create_agent_session_from_services`), honor it directly. Otherwise
    // restore the model recorded in the session when resuming, then fall back
    // to CLI flags / scoped models / settings / first available model.
    let mut model: Option<Model> = options.model.clone();
    let mut fallback_message: Option<String> = None;

    if model.is_none() && has_existing_session {
        if let Some(saved) = &existing_session.model {
            let restored = model_registry
                .find(&saved.provider, &saved.model_id)
                .filter(|m| model_registry.has_configured_auth(m));
            match restored {
                Some(m) => model = Some(m),
                None => {
                    fallback_message =
                        Some(format!("Could not restore model {}/{}", saved.provider, saved.model_id));
                }
            }
        }
    }

    // Thinking level: an explicit caller/CLI value wins, then the session's
    // recorded level, then the settings default (TS sdk.ts order). The CLI
    // passes the raw `--models` scope instead of pre-resolving it, so a scoped
    // pattern's explicit level (`provider/id:high`, applied by TS main.ts to
    // `options.thinkingLevel`) is folded in here for new sessions.
    let mut resolved_thinking_level: Option<String> = options.thinking_level.clone().or_else(|| {
        if options.model.is_none() && !has_existing_session {
            scoped.first().and_then(|s| s.thinking_level.clone())
        } else {
            None
        }
    });

    if model.is_none() {
        let initial = model_resolver::find_initial_model(
            options.cli_provider.as_deref(),
            options.cli_model.as_deref(),
            &scoped,
            has_existing_session,
            default_provider.as_deref(),
            default_model_id.as_deref(),
            default_thinking_level.as_deref(),
            &model_registry,
        );
        model = initial.model;
        match (&model, &fallback_message) {
            (None, _) => {
                // TS 原版行为（sdk.ts）：没有任何可用模型时，session 仍创建，
                // 只是不带模型（model 为空 id），thinkingLevel = "off"，
                // modelFallbackMessage = formatNoModelsAvailableMessage()。
                // prompt() 会报"没有选择模型"（formatNoModelSelectedMessage）。
                fallback_message = Some(
                    crate::core::auth_guidance::format_no_models_available_message(
                        &crate::config::get_docs_path().to_string_lossy(),
                    ),
                );
            }
            (Some(m), Some(previous)) => {
                fallback_message = Some(format!("{previous}. Using {}/{}", m.provider, m.id));
            }
            (Some(_), None) => {}
        }
    }

    // Restore the session's thinking level when resuming; only then fall back
    // to the settings default (TS sdk.ts thinking-level order).
    if resolved_thinking_level.is_none() && has_existing_session {
        resolved_thinking_level = Some(if has_thinking_entry {
            existing_session.thinking_level.clone()
        } else {
            default_thinking_level
                .clone()
                .unwrap_or_else(|| crate::core::defaults::DEFAULT_THINKING_LEVEL.to_string())
        });
    }

    let thinking_level = resolved_thinking_level.unwrap_or_else(|| {
        default_thinking_level
            .clone()
            .unwrap_or_else(|| crate::core::defaults::DEFAULT_THINKING_LEVEL.to_string())
    });

    // Clamp to the model's capabilities; a session without a resolved model is
    // always "off" (TS sdk.ts `clampThinkingLevel`).
    let (model, thinking_level) = match model {
        Some(m) => {
            let level = pi_agent_core::pi_ai_types::clamp_thinking_level(&m, &thinking_level);
            (m, level)
        }
        None => {
            let empty_model = Model {
                id: String::new(),
                name: String::new(),
                api: String::new(),
                provider: String::new(),
                base_url: String::new(),
                reasoning: false,
                thinking_level_map: None,
                input: Vec::new(),
                cost: pi_agent_core::pi_ai_types::ModelCost::default(),
                context_window: 0,
                max_tokens: 0,
                sampling_params: None,
                headers: None,
                compat: None,
            };
            (empty_model, "off".to_string())
        }
    };

    // ── Extension registry (Rust native extensions) ───────────────────
    let extension_registry = options
        .extension_registry
        .take()
        .unwrap_or_default();

    // Collect prompt_guidelines BEFORE wrapping in Arc
    // (collect_tools() requires &mut self, which Arc doesn't provide).
    let prompt_guidelines = collect_prompt_guidelines(&extension_registry);
    let extension_registry_arc = std::sync::Arc::new(extension_registry);

    // Collect extension names before the Arc is moved into AgentSessionConfig
    let extension_names: Vec<String> = if options.enable_extensions {
        extension_registry_arc
            .hook_runner()
            .handlers()
            .iter()
            .map(|ext| ext.name().to_string())
            .collect()
    } else {
        Vec::new()
    };

    // Dispatch session_start to extensions before session creation.
    // RuntimeHandle 提供真实 cwd/agent_dir（noop() 的 get_cwd/get_agent_dir
    // 返回空串，扩展 spawn 子进程或写状态文件时会失败/落错位置）。
    let mut ext_runtime_handle = crate::core::extensions::RuntimeHandle::noop();
    let ext_cwd = cwd.clone();
    ext_runtime_handle.get_cwd = std::sync::Arc::new(move || ext_cwd.clone());
    let ext_agent_dir = agent_dir.clone();
    ext_runtime_handle.get_agent_dir = std::sync::Arc::new(move || ext_agent_dir.clone());
    // Extensions can register providers (match TS `registerProvider`, #019e4ad68).
    crate::core::extensions::install_register_provider_hook(
        &mut ext_runtime_handle,
        model_registry.clone(),
    );
    let ext_ctx = crate::core::extensions::ExtensionContext::new(
        cwd.clone(),
        false,
        options
            .ui_context
            .clone()
            .unwrap_or_else(crate::core::extensions::ExtensionUIContext::noop),
        ext_runtime_handle,
    );
    let session_start_reason = options
        .session_start_event
        .as_ref()
        .map(|e| e.reason.to_string())
        .unwrap_or_else(|| "startup".to_string());
    let previous_session_file = options
        .session_start_event
        .as_ref()
        .and_then(|e| e.previous_session_file.as_deref());
    crate::core::extensions::dispatcher::dispatch_session_start(
        &extension_registry_arc,
        &session_start_reason,
        &ext_ctx,
        previous_session_file,
    )
    .await;

    // Load resources for context files and skills
    let resource_options = options
        .resource_loader
        .clone()
        .unwrap_or_else(|| ResourceLoaderOptions {
            cwd: cwd.clone(),
            agent_dir: Some(agent_dir.clone()),
            include_defaults: true,
            ..Default::default()
        });
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

    // Extension action bus: JS extension read-actions read the shared state
    // snapshot; write-actions are queued and drained by the session at turn
    // boundaries. Always created (negligible cost) so the config fields are
    // unconditionally populated.
    let (_extension_action_sender, extension_action_rx, extension_state_view) =
        crate::core::extensions::action_bus::ExtensionActionSender::new();

    let has_telemetry = crate::core::telemetry::is_install_telemetry_enabled(
        &settings_manager,
        std::env::var("PI_TELEMETRY").ok().as_deref(),
    );

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
            .or_else(|| Some(create_default_stream_fn(has_telemetry))),
        convert_to_llm: options.convert_to_llm,
        initial_active_tool_names: Some(initial_active_tool_names),
        allowed_tool_names,
        excluded_tool_names,
        extension_registry: Some(extension_registry_arc),
        ui_context: options.ui_context.clone(),
        resources: Some(resources),
        custom_tools: options.custom_tools,
        tools_options: options.tools_options,
        extension_state_view: Some(extension_state_view),
        extension_action_rx: Some(extension_action_rx),
    };

    let session =
        AgentSession::new(session_manager, settings_manager, model_registry, session_options).await;

    // Load persisted messages into agent state if restoring from a session file
    if session.get_session_manager().get_session_file().is_some() {
        let count = session.load_messages_from_session().await;
        if count > 0 {
            eprintln!("[pi] Restored {count} messages from session file");
        }
    }

    // Build extensions result from pre-collected names
    let extensions_result = if options.enable_extensions {
        Some(ExtensionsResult {
            extensions: extension_names,
            errors: Vec::new(),
        })
    } else {
        None
    };

    Ok((
        session,
        CreateAgentSessionResult {
            model_fallback_message: fallback_message,
            extensions_result,
        },
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::core::model_registry::ProviderConfig;
    use pi_agent_core::pi_ai_types::ModelCost;

    /// Minimal model definition for the session-restore tests.
    fn test_model(provider: &str, id: &str, reasoning: bool) -> Model {
        Model {
            id: id.to_string(),
            name: id.to_string(),
            api: "openai-completions".to_string(),
            provider: provider.to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            reasoning,
            thinking_level_map: None,
            input: vec!["text".to_string()],
            cost: ModelCost::default(),
            context_window: 128_000,
            max_tokens: 4096,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    /// Registry backed by a non-existent models.json, whose provider carries an
    /// API key so `has_configured_auth` is true (models are selectable and
    /// restorable). Bypasses the developer's real `~/.pi/models.json`.
    fn registry_with(
        models: Vec<Model>,
        provider: &str,
        models_path: &std::path::Path,
    ) -> ModelRegistry {
        let registry = ModelRegistry::new_with_models_path(models, models_path);
        registry.register_provider(
            provider,
            ProviderConfig {
                name: None,
                base_url: None,
                api_key: Some("test-key".to_string()),
                api: None,
                headers: None,
                auth_header: None,
                models: None,
            },
        )
        .unwrap();
        registry
    }

    /// A session containing one user message plus optional model/thinking
    /// entries, so `hasExistingSession` is true on the next create call.
    fn existing_session(cwd: &str, session_dir: &str) -> SessionManager {
        let mut sm = SessionManager::new(cwd, session_dir, None, false, None);
        sm.append_message(serde_json::json!({"role": "user", "content": "hello"}));
        sm
    }

    fn temp_cwd(tmp: &tempfile::TempDir) -> String {
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        cwd.to_string_lossy().to_string()
    }

    /// Resuming a session restores the recorded model and thinking level
    /// (TS sdk.ts `existingSession.model` + `hasThinkingEntry`).
    #[tokio::test]
    async fn create_agent_session_restores_model_and_thinking_from_session() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = temp_cwd(&tmp);
        let session_dir = tmp.path().join("sessions");
        let registry = registry_with(
            vec![test_model("testp", "m1", true)],
            "testp",
            &tmp.path().join("models.json"),
        );

        let mut sm = existing_session(&cwd, session_dir.to_str().unwrap());
        sm.append_model_change("testp", "m1");
        sm.append_thinking_level_change("high");

        let (session, result) = create_agent_session(CreateAgentSessionOptions {
            cwd: cwd.clone(),
            agent_dir: Some(tmp.path().join("agent").to_string_lossy().to_string()),
            model: None,
            model_registry: Some(registry),
            session_manager: Some(sm),
            ..Default::default()
        })
        .await
        .unwrap();

        assert!(result.model_fallback_message.is_none());
        let model = session.get_model().await;
        assert_eq!(model.provider, "testp");
        assert_eq!(model.id, "m1");
        assert_eq!(session.get_thinking_level().await, "high");
    }

    /// When the saved model can no longer be restored, the session falls back
    /// to the next available model and reports it in the fallback message; the
    /// scoped models are skipped because the session is continuing
    /// (TS `isContinuing: hasExistingSession`).
    #[tokio::test]
    async fn restore_failure_reports_fallback_and_skips_scoped_models() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = temp_cwd(&tmp);
        let session_dir = tmp.path().join("sessions");
        let registry = registry_with(
            vec![
                test_model("testp", "other-model", false),
                test_model("testp", "scoped-model", false),
            ],
            "testp",
            &tmp.path().join("models.json"),
        );

        let mut sm = existing_session(&cwd, session_dir.to_str().unwrap());
        sm.append_model_change("missing", "gone");
        let scoped = test_model("testp", "scoped-model", false);

        let (session, result) = create_agent_session(CreateAgentSessionOptions {
            cwd: cwd.clone(),
            agent_dir: Some(tmp.path().join("agent").to_string_lossy().to_string()),
            model: None,
            scoped_models: Some(vec![(scoped, None)]),
            model_registry: Some(registry),
            session_manager: Some(sm),
            ..Default::default()
        })
        .await
        .unwrap();

        assert_eq!(session.get_model().await.id, "other-model");
        let message = result.model_fallback_message.expect("fallback message");
        assert!(
            message.starts_with("Could not restore model missing/gone. Using testp/other-model"),
            "unexpected fallback message: {message}"
        );
    }

    /// A session that recorded a thinking level but no model still restores
    /// the thinking level; the initial-model fallback level (settings/default)
    /// must not shadow it. Regression: the initial-model level was applied
    /// before the session restore, so this case silently got "medium".
    #[tokio::test]
    async fn session_thinking_level_wins_over_initial_model_default() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = temp_cwd(&tmp);
        let session_dir = tmp.path().join("sessions");
        let registry = registry_with(
            vec![test_model("testp", "m1", true)],
            "testp",
            &tmp.path().join("models.json"),
        );

        // Messages + thinking entry, but no model change entry.
        let mut sm = existing_session(&cwd, session_dir.to_str().unwrap());
        sm.append_thinking_level_change("high");

        let (session, result) = create_agent_session(CreateAgentSessionOptions {
            cwd: cwd.clone(),
            agent_dir: Some(tmp.path().join("agent").to_string_lossy().to_string()),
            model: None,
            model_registry: Some(registry),
            session_manager: Some(sm),
            ..Default::default()
        })
        .await
        .unwrap();

        assert!(result.model_fallback_message.is_none());
        assert_eq!(session.get_model().await.id, "m1");
        assert_eq!(session.get_thinking_level().await, "high");
    }

    /// Scoped models still pick the initial model for a brand-new session.
    #[tokio::test]
    async fn scoped_model_is_used_when_not_continuing() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = temp_cwd(&tmp);
        let registry = registry_with(
            vec![
                test_model("testp", "other-model", false),
                test_model("testp", "scoped-model", false),
            ],
            "testp",
            &tmp.path().join("models.json"),
        );
        let scoped = test_model("testp", "scoped-model", false);

        let (session, _result) = create_agent_session(CreateAgentSessionOptions {
            cwd: cwd.clone(),
            agent_dir: Some(tmp.path().join("agent").to_string_lossy().to_string()),
            model: None,
            scoped_models: Some(vec![(scoped, None)]),
            model_registry: Some(registry),
            ..Default::default()
        })
        .await
        .unwrap();

        assert_eq!(session.get_model().await.id, "scoped-model");
    }

    /// An explicit thinking level wins over the value restored from the
    /// session (TS sdk.ts checks `options.thinkingLevel` first).
    #[tokio::test]
    async fn explicit_thinking_level_overrides_session_value() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = temp_cwd(&tmp);
        let session_dir = tmp.path().join("sessions");
        let registry = registry_with(
            vec![test_model("testp", "m1", true)],
            "testp",
            &tmp.path().join("models.json"),
        );

        let mut sm = existing_session(&cwd, session_dir.to_str().unwrap());
        sm.append_model_change("testp", "m1");
        sm.append_thinking_level_change("high");

        let (session, _result) = create_agent_session(CreateAgentSessionOptions {
            cwd: cwd.clone(),
            agent_dir: Some(tmp.path().join("agent").to_string_lossy().to_string()),
            model: None,
            thinking_level: Some("low".to_string()),
            model_registry: Some(registry),
            session_manager: Some(sm),
            ..Default::default()
        })
        .await
        .unwrap();

        assert_eq!(session.get_thinking_level().await, "low");
    }

    /// A non-reasoning model clamps the resolved thinking level to "off"
    /// (TS sdk.ts `clampThinkingLevel`). Previously the SDK hardcoded
    /// "medium" for caller-provided models.
    #[tokio::test]
    async fn thinking_level_is_clamped_to_model_capabilities() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = temp_cwd(&tmp);
        let registry = registry_with(
            vec![test_model("testp", "plain", false)],
            "testp",
            &tmp.path().join("models.json"),
        );

        let (session, _result) = create_agent_session(CreateAgentSessionOptions {
            cwd: cwd.clone(),
            agent_dir: Some(tmp.path().join("agent").to_string_lossy().to_string()),
            model: Some(test_model("testp", "plain", false)),
            model_registry: Some(registry),
            ..Default::default()
        })
        .await
        .unwrap();

        assert_eq!(session.get_thinking_level().await, "off");
    }

    /// With no explicit thinking level and no session, the settings default is
    /// used even when the model comes from `--provider`/`--model` or the first
    /// available model (TS sdk.ts: `defaultThinkingLevel ?? DEFAULT`).
    /// Regression: the initial-model path hardcoded `DEFAULT_THINKING_LEVEL`,
    /// so a configured default was silently ignored on those paths.
    #[tokio::test]
    async fn settings_default_thinking_level_is_used_without_session() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = temp_cwd(&tmp);
        let registry = registry_with(
            vec![test_model("testp", "m1", true)],
            "testp",
            &tmp.path().join("models.json"),
        );
        let agent_dir = tmp.path().join("agent");
        let mut settings = SettingsManager::create(&cwd, Some(agent_dir.to_str().unwrap()));
        settings.set_default_thinking_level("high");

        let (session, _result) = create_agent_session(CreateAgentSessionOptions {
            cwd: cwd.clone(),
            agent_dir: Some(agent_dir.to_string_lossy().to_string()),
            model: None,
            cli_provider: Some("testp".to_string()),
            cli_model: Some("m1".to_string()),
            settings_manager: Some(settings),
            model_registry: Some(registry),
            ..Default::default()
        })
        .await
        .unwrap();

        assert_eq!(session.get_model().await.id, "m1");
        assert_eq!(session.get_thinking_level().await, "high");
    }
}
