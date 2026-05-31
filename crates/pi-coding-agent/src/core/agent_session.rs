use std::sync::Arc;

use pi_agent_core::agent::Agent;
use pi_agent_core::pi_ai_types::{ContentBlock, Model, ThinkingLevel};
use pi_agent_core::types::{
    AgentEvent, AgentMessage, AgentState, ConvertToLlmFn, StreamFn,
};

use crate::core::compaction::{self, CompactionSettings};
use crate::core::context_usage::ContextUsage;
use crate::core::event_bus::EventBusController;
use crate::core::messages;
use crate::core::model_registry::ModelRegistry;
use crate::core::session_manager::SessionManager;
use crate::core::system_prompt::{self, BuildSystemPromptOptions, ContextFile, SkillInfo};
use crate::core::tools;

#[derive(Clone)]
pub struct AgentSessionOptions {
    pub cwd: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub custom_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub selected_tools: Option<Vec<String>>,
    pub tool_snippets: Option<std::collections::HashMap<String, String>>,
    pub prompt_guidelines: Option<Vec<String>>,
    pub context_files: Vec<ContextFile>,
    pub skills: Vec<SkillInfo>,
    pub session_name: Option<String>,
    pub stream_fn: Option<StreamFn>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub initial_active_tool_names: Option<Vec<String>>,
    pub allowed_tool_names: Option<Vec<String>>,
    pub excluded_tool_names: Option<Vec<String>>,
}

pub struct AgentSession {
    agent: Agent,
    session_manager: SessionManager,
    event_bus: EventBusController,
    model_registry: ModelRegistry,
    compaction_settings: CompactionSettings,
    cwd: String,
    scoped_models: Vec<(Model, Option<ThinkingLevel>)>,
    initial_active_tool_names: Vec<String>,
    allowed_tool_names: Option<Vec<String>>,
    excluded_tool_names: Option<Vec<String>>,
}

impl AgentSession {
    pub fn new(
        session_manager: SessionManager,
        event_bus: EventBusController,
        model_registry: ModelRegistry,
        options: AgentSessionOptions,
    ) -> Self {
        let system_prompt = system_prompt::build_system_prompt(&BuildSystemPromptOptions {
            cwd: options.cwd.clone(),
            custom_prompt: options.custom_prompt,
            append_system_prompt: options.append_system_prompt,
            selected_tools: options.selected_tools.clone(),
            tool_snippets: options.tool_snippets,
            prompt_guidelines: options.prompt_guidelines,
            context_files: Some(options.context_files),
            skills: Some(options.skills),
        });

        let tools_options = tools::ToolsOptions::default();
        let tool_list = tools::create_coding_tools(&options.cwd, Some(&tools_options));

        let tools: Vec<Arc<pi_agent_core::types::DynTool>> = tool_list
            .into_iter()
            .map(|t| Arc::new(t) as Arc<pi_agent_core::types::DynTool>)
            .collect();

        let initial_state = AgentState {
            system_prompt,
            model: options.model.clone(),
            thinking_level: options.thinking_level,
            tools,
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        };

        let convert_to_llm = options
            .convert_to_llm
            .unwrap_or_else(|| Arc::new(messages::convert_to_llm));

        let stream_fn = options.stream_fn.unwrap_or_else(|| {
            Arc::new(|_model, _ctx, _thinking, _opts| {
                Box::pin(async {
                    Err::<pi_agent_core::pi_ai_types::StreamResponse, _>(
                        "No stream function configured".into(),
                    )
                })
            })
        });

        let agent_options = pi_agent_core::agent::AgentOptions {
            initial_state: Some(initial_state),
            convert_to_llm: Some(convert_to_llm),
            stream_fn: Some(stream_fn),
            session_id: Some(session_manager.get_session_id().to_string()),
            ..Default::default()
        };

        let agent = Agent::new(agent_options);

        let initial_active_tool_names = options
            .initial_active_tool_names
            .unwrap_or_else(|| vec!["read", "bash", "edit", "write"].iter().map(|s| s.to_string()).collect());

        Self {
            agent,
            session_manager,
            event_bus,
            model_registry,
            compaction_settings: CompactionSettings::default(),
            cwd: options.cwd,
            scoped_models: Vec::new(),
            initial_active_tool_names,
            allowed_tool_names: options.allowed_tool_names,
            excluded_tool_names: options.excluded_tool_names,
        }
    }

    pub fn get_agent(&self) -> &Agent {
        &self.agent
    }

    pub fn get_messages(&self) -> &[AgentMessage] {
        unimplemented!("Use get_agent().state() instead")
    }

    pub fn get_system_prompt(&self) -> &str {
        unimplemented!("Use get_agent().state() instead")
    }

    pub fn get_model(&self) -> &Model {
        unimplemented!("Use get_agent().state() instead")
    }

    pub fn get_thinking_level(&self) -> &ThinkingLevel {
        unimplemented!("Use get_agent().state() instead")
    }

    pub fn get_cwd(&self) -> &str {
        &self.cwd
    }

    pub fn get_session_id(&self) -> String {
        self.session_manager.get_session_id().to_string()
    }

    pub fn get_session_name(&self) -> Option<String> {
        self.session_manager.get_session_name()
    }

    pub fn set_session_name(&mut self, name: &str) {
        self.session_manager.append_session_info(name);
    }

    pub fn is_streaming(&self) -> bool {
        false
    }

    pub fn get_error_message(&self) -> Option<&str> {
        None
    }

    pub fn get_context_usage(&self) -> ContextUsage {
        ContextUsage::default()
    }

    pub fn should_compact(&self) -> bool {
        false
    }

    pub fn get_last_assistant_text(&self) -> Option<String> {
        None
    }

    pub fn get_scoped_models(&self) -> &[(Model, Option<ThinkingLevel>)] {
        &self.scoped_models
    }

    pub fn set_scoped_models(&mut self, models: Vec<(Model, Option<ThinkingLevel>)>) {
        self.scoped_models = models;
    }

    pub fn get_session_manager(&self) -> &SessionManager {
        &self.session_manager
    }

    pub fn get_session_manager_mut(&mut self) -> &mut SessionManager {
        &mut self.session_manager
    }

    pub fn get_event_bus(&self) -> &EventBusController {
        &self.event_bus
    }

    pub fn get_model_registry(&self) -> &ModelRegistry {
        &self.model_registry
    }

    pub fn get_compaction_settings(&self) -> &CompactionSettings {
        &self.compaction_settings
    }

    pub fn set_compaction_settings(&mut self, settings: CompactionSettings) {
        self.compaction_settings = settings;
    }

    pub fn get_initial_active_tool_names(&self) -> &[String] {
        &self.initial_active_tool_names
    }

    pub fn get_allowed_tool_names(&self) -> Option<&[String]> {
        self.allowed_tool_names.as_deref()
    }

    pub fn get_excluded_tool_names(&self) -> Option<&[String]> {
        self.excluded_tool_names.as_deref()
    }

    pub async fn add_user_message(&mut self, content: Vec<ContentBlock>) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content,
            timestamp,
        };
        let json = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
        self.session_manager.append_message(json);
        self.agent.process(vec![message]).await.ok();
    }

    pub async fn add_user_text(&mut self, text: &str) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content: vec![ContentBlock::text(text)],
            timestamp,
        };
        let json = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
        self.session_manager.append_message(json);
        self.agent.process(vec![message]).await.ok();
    }

    pub async fn abort(&self) {
        self.agent.abort().await;
    }

    pub fn subscribe(
        &self,
        listener: Arc<
            dyn Fn(
                    AgentEvent,
                    Option<tokio::sync::watch::Receiver<bool>>,
                ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
                + Send
                + Sync,
        >,
    ) -> impl std::future::Future<Output = ()> {
        self.agent.subscribe(listener)
    }

    pub fn dispose(self) {}
}