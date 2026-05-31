use std::sync::Arc;

use pi_agent_core::pi_ai_types::{ContentBlock, Model, ThinkingLevel};
use pi_agent_core::types::{AgentMessage, AgentState, CustomContent};

use crate::core::compaction::{self, CompactionSettings};
use crate::core::context_usage::ContextUsage;
use crate::core::event_bus::EventBusController;
use crate::core::messages;
use crate::core::model_registry::ModelRegistry;
use crate::core::session_manager::SessionManager;
use crate::core::system_prompt::{self, BuildSystemPromptOptions, ContextFile, SkillInfo};
use crate::core::tools;

#[derive(Debug, Clone)]
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
}

pub struct AgentSession {
    session_manager: SessionManager,
    event_bus: EventBusController,
    model_registry: ModelRegistry,
    state: AgentState,
    compaction_settings: CompactionSettings,
    tools_options: tools::ToolsOptions,
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

        let tools: Vec<Arc<dyn std::any::Any + Send + Sync>> = tool_list
            .into_iter()
            .map(|t| Arc::new(t) as Arc<dyn std::any::Any + Send + Sync>)
            .collect();

        let state = AgentState {
            system_prompt,
            model: options.model,
            thinking_level: options.thinking_level,
            tools,
            messages: Vec::new(),
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        };

        Self {
            session_manager,
            event_bus,
            model_registry,
            state,
            compaction_settings: CompactionSettings::default(),
            tools_options,
        }
    }

    pub fn get_messages(&self) -> &[AgentMessage] {
        &self.state.messages
    }

    pub fn get_system_prompt(&self) -> &str {
        &self.state.system_prompt
    }

    pub fn get_model(&self) -> &Model {
        &self.state.model
    }

    pub fn get_thinking_level(&self) -> &ThinkingLevel {
        &self.state.thinking_level
    }

    pub fn set_model(&mut self, model: Model) {
        self.session_manager.append_model_change(&model.provider, &model.id);
        self.state.model = model;
    }

    pub fn set_thinking_level(&mut self, level: ThinkingLevel) {
        self.session_manager.append_thinking_level_change(&format!("{:?}", level).to_lowercase());
        self.state.thinking_level = level;
    }

    pub fn is_streaming(&self) -> bool {
        self.state.is_streaming
    }

    pub fn get_error_message(&self) -> Option<&str> {
        self.state.error_message.as_deref()
    }

    pub fn add_user_message(&mut self, content: Vec<ContentBlock>) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::User {
            content,
            timestamp,
        };
        let json = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
        self.state.messages.push(message);
        self.session_manager.append_message(json);
    }

    pub fn add_user_text(&mut self, text: &str) {
        self.add_user_message(vec![ContentBlock::text(text)]);
    }

    pub fn add_assistant_message(&mut self, message: AgentMessage) {
        let json = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
        self.state.messages.push(message);
        self.session_manager.append_message(json);
    }

    pub fn add_tool_result(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        content: Vec<ContentBlock>,
        details: serde_json::Value,
        is_error: bool,
    ) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            details,
            is_error,
            timestamp,
        };
        let json = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
        self.state.messages.push(message);
        self.session_manager.append_message(json);
    }

    pub fn add_bash_execution(
        &mut self,
        command: String,
        output: String,
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
        exclude_from_context: Option<bool>,
    ) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::BashExecution {
            command,
            output,
            exit_code,
            cancelled,
            truncated,
            full_output_path,
            timestamp,
            exclude_from_context,
        };
        let json = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
        self.state.messages.push(message);
        self.session_manager.append_message(json);
    }

    pub fn add_custom_message(
        &mut self,
        custom_type: String,
        content: CustomContent,
        display: bool,
        details: Option<serde_json::Value>,
    ) {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let message = AgentMessage::Custom {
            custom_type,
            content,
            display,
            details,
            timestamp,
        };
        let json = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
        self.state.messages.push(message);
        self.session_manager.append_message(json);
    }

    pub fn get_context_usage(&self) -> ContextUsage {
        let mut total_input: u64 = 0;
        let mut total_output: u64 = 0;
        let mut cache_read: Option<u64> = None;
        let mut cache_write: Option<u64> = None;

        for msg in &self.state.messages {
            if let AgentMessage::Assistant { usage, .. } = msg {
                total_input += usage.input_tokens;
                total_output += usage.output_tokens;
                if let Some(cr) = usage.cache_read_input_tokens {
                    cache_read = Some(cache_read.unwrap_or(0) + cr);
                }
                if let Some(cw) = usage.cache_write_input_tokens {
                    cache_write = Some(cache_write.unwrap_or(0) + cw);
                }
            }
        }

        crate::core::context_usage::compute_context_usage(
            &self.state.model,
            self.state.messages.len(),
            total_input,
            total_output,
            cache_read,
            cache_write,
        )
    }

    pub fn should_compact(&self) -> bool {
        let usage = self.get_context_usage();
        compaction::should_compact(
            usage.total_tokens,
            self.state.model.context_window,
            &self.compaction_settings,
        )
    }

    pub fn get_last_assistant_text(&self) -> Option<String> {
        for msg in self.state.messages.iter().rev() {
            if let AgentMessage::Assistant {
                content,
                stop_reason,
                ..
            } = msg
            {
                if *stop_reason == Some(pi_agent_core::pi_ai_types::StopReason::Aborted)
                    && content.is_empty()
                {
                    continue;
                }
                let mut text = String::new();
                for block in content {
                    if let ContentBlock::Text { text: t, .. } = block {
                        text.push_str(t);
                    }
                }
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
        None
    }

    pub fn set_session_name(&mut self, name: &str) {
        self.session_manager.append_session_info(name);
    }

    pub fn get_session_name(&self) -> Option<String> {
        self.session_manager.get_session_name()
    }

    pub fn get_session_id(&self) -> String {
        self.session_manager.get_session_id().to_string()
    }

    pub fn get_cwd(&self) -> String {
        self.session_manager.get_cwd().to_string()
    }

    pub fn get_llm_messages(&self) -> Vec<pi_agent_core::pi_ai_types::Message> {
        messages::convert_to_llm(&self.state.messages)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model_registry::builtin_models;

    fn create_test_session() -> AgentSession {
        let model = builtin_models().into_iter().next().unwrap();
        let session_manager = SessionManager::new("/tmp", "/tmp/test-session", None, false, None);
        let event_bus = EventBusController::new();
        let model_registry = ModelRegistry::new(builtin_models());

        AgentSession::new(
            session_manager,
            event_bus,
            model_registry,
            AgentSessionOptions {
                cwd: "/tmp".to_string(),
                model,
                thinking_level: ThinkingLevel::Medium,
                custom_prompt: None,
                append_system_prompt: None,
                selected_tools: None,
                tool_snippets: None,
                prompt_guidelines: None,
                context_files: vec![],
                skills: vec![],
                session_name: None,
            },
        )
    }

    #[test]
    fn test_agent_session_creation() {
        let session = create_test_session();
        assert!(!session.get_system_prompt().is_empty());
        assert!(!session.is_streaming());
    }

    #[test]
    fn test_add_user_text() {
        let mut session = create_test_session();
        session.add_user_text("Hello");
        assert_eq!(session.get_messages().len(), 1);
    }

    #[test]
    fn test_get_last_assistant_text() {
        let mut session = create_test_session();
        assert!(session.get_last_assistant_text().is_none());

        session.add_assistant_message(AgentMessage::Assistant {
            content: vec![ContentBlock::text("Hello there")],
            api: "test".into(),
            provider: "test".into(),
            model: "test".into(),
            usage: Default::default(),
            stop_reason: None,
            error_message: None,
            timestamp: 1000,
        });
        assert_eq!(session.get_last_assistant_text(), Some("Hello there".to_string()));
    }

    #[test]
    fn test_get_context_usage() {
        let session = create_test_session();
        let usage = session.get_context_usage();
        assert_eq!(usage.total_tokens, 0);
    }
}