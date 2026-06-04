use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::harness::types::{
    AbortResult, AgentHarnessOwnEvent, AgentHarnessResources, AgentHarnessStreamOptions,
    CompactResult, HarnessError, NavigateTreeResult, PromptTemplate, QueueMode, Session, Skill,
};
use crate::pi_ai_types::{ContentBlock, Model, ThinkingLevel};
use crate::types::{AgentEvent, AgentMessage};

type HarnessListener<S, P> = Box<
    dyn Fn(
            AgentHarnessEvent<S, P>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone)]
pub enum AgentHarnessEvent<S: Clone = Skill, P: Clone = PromptTemplate> {
    Agent(AgentEvent),
    Own(AgentHarnessOwnEvent<S, P>),
}

pub struct AgentHarness<
    S: Clone + Send + Sync + 'static = Skill,
    P: Clone + Send + Sync + 'static = PromptTemplate,
> {
    session: Arc<RwLock<Session>>,
    model: Arc<RwLock<Model>>,
    thinking_level: Arc<RwLock<ThinkingLevel>>,
    tools: Arc<RwLock<HashMap<String, String>>>,
    active_tool_names: Arc<RwLock<Vec<String>>>,
    resources: Arc<RwLock<AgentHarnessResources<S, P>>>,
    stream_options: Arc<RwLock<AgentHarnessStreamOptions>>,
    steering_mode: Arc<RwLock<QueueMode>>,
    follow_up_mode: Arc<RwLock<QueueMode>>,
    steer_queue: Arc<RwLock<Vec<AgentMessage>>>,
    follow_up_queue: Arc<RwLock<Vec<AgentMessage>>>,
    listeners: Arc<RwLock<Vec<HarnessListener<S, P>>>>,
    phase: Arc<RwLock<HarnessPhase>>,
    run_promise: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
    idle_notify: tokio::sync::watch::Sender<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HarnessPhase {
    Idle,
    Running,
}

impl<S: Clone + Send + Sync + 'static, P: Clone + Send + Sync + 'static> AgentHarness<S, P> {
    pub fn new(
        session: Session,
        model: Model,
        options: Option<AgentHarnessOptions<S, P>>,
    ) -> Self {
        let opts = options.unwrap_or_default();

        Self {
            session: Arc::new(RwLock::new(session)),
            model: Arc::new(RwLock::new(model)),
            thinking_level: Arc::new(RwLock::new(
                opts.thinking_level.unwrap_or_else(|| "off".to_string()),
            )),
            tools: Arc::new(RwLock::new(HashMap::new())),
            active_tool_names: Arc::new(RwLock::new(opts.active_tool_names.unwrap_or_default())),
            resources: Arc::new(RwLock::new(opts.resources.unwrap_or(
                AgentHarnessResources {
                    skills: None,
                    prompt_templates: None,
                },
            ))),
            stream_options: Arc::new(RwLock::new(opts.stream_options.unwrap_or(
                AgentHarnessStreamOptions {
                    temperature: None,
                    top_p: None,
                    max_tokens: None,
                },
            ))),
            steering_mode: Arc::new(RwLock::new(opts.steering_mode.unwrap_or(QueueMode::Queue))),
            follow_up_mode: Arc::new(RwLock::new(opts.follow_up_mode.unwrap_or(QueueMode::Queue))),
            steer_queue: Arc::new(RwLock::new(Vec::new())),
            follow_up_queue: Arc::new(RwLock::new(Vec::new())),
            listeners: Arc::new(RwLock::new(Vec::new())),
            phase: Arc::new(RwLock::new(HarnessPhase::Idle)),
            run_promise: Arc::new(RwLock::new(None)),
            idle_notify: tokio::sync::watch::channel(true).0,
        }
    }

    pub async fn model(&self) -> Model {
        self.model.read().await.clone()
    }

    pub async fn set_model(&self, model: Model) -> std::result::Result<(), HarnessError> {
        let _previous = self.model.read().await.clone();
        *self.model.write().await = model.clone();
        let mut session = self.session.write().await;
        let _ = session
            .append_model_change(model.provider.clone(), model.id.clone())
            .await;
        Ok(())
    }

    pub async fn thinking_level(&self) -> ThinkingLevel {
        self.thinking_level.read().await.clone()
    }

    pub async fn set_thinking_level(
        &self,
        level: ThinkingLevel,
    ) -> std::result::Result<(), HarnessError> {
        let _previous = self.thinking_level.read().await.clone();
        *self.thinking_level.write().await = level.clone();
        let mut session = self.session.write().await;
        let _ = session.append_thinking_level_change(level.clone()).await;
        Ok(())
    }

    pub async fn get_tools(&self) -> Vec<String> {
        self.tools.read().await.keys().cloned().collect()
    }

    pub async fn get_active_tools(&self) -> Vec<String> {
        self.active_tool_names.read().await.clone()
    }

    pub async fn set_active_tools(
        &self,
        tool_names: Vec<String>,
    ) -> std::result::Result<(), HarnessError> {
        self.validate_tool_names(&tool_names).await?;
        let _previous = self.active_tool_names.read().await.clone();
        *self.active_tool_names.write().await = tool_names.clone();

        let phase = *self.phase.read().await;
        if phase == HarnessPhase::Idle {
            let mut session = self.session.write().await;
            let _ = session.append_active_tools_change(tool_names.clone()).await;
        }

        Ok(())
    }

    async fn validate_tool_names(
        &self,
        _names: &[String],
    ) -> std::result::Result<(), HarnessError> {
        let _tools = self.tools.read().await;
        Ok(())
    }

    pub async fn steering_mode(&self) -> QueueMode {
        *self.steering_mode.read().await
    }

    pub async fn set_steering_mode(&self, mode: QueueMode) {
        *self.steering_mode.write().await = mode;
    }

    pub async fn follow_up_mode(&self) -> QueueMode {
        *self.follow_up_mode.read().await
    }

    pub async fn set_follow_up_mode(&self, mode: QueueMode) {
        *self.follow_up_mode.write().await = mode;
    }

    pub async fn get_resources(&self) -> AgentHarnessResources<S, P> {
        let r = self.resources.read().await;
        AgentHarnessResources {
            skills: r.skills.clone(),
            prompt_templates: r.prompt_templates.clone(),
        }
    }

    pub async fn set_resources(&self, resources: AgentHarnessResources<S, P>) {
        let _previous = self.get_resources().await;
        *self.resources.write().await = AgentHarnessResources {
            skills: resources.skills.clone(),
            prompt_templates: resources.prompt_templates.clone(),
        };
    }

    pub async fn get_stream_options(&self) -> AgentHarnessStreamOptions {
        self.stream_options.read().await.clone()
    }

    pub async fn set_stream_options(&self, options: AgentHarnessStreamOptions) {
        *self.stream_options.write().await = options;
    }

    pub async fn steer(&self, message: AgentMessage) {
        let mode = *self.steering_mode.read().await;
        let mut queue = self.steer_queue.write().await;
        match mode {
            QueueMode::Queue => queue.push(message),
            QueueMode::Replace => {
                queue.clear();
                queue.push(message);
            }
            QueueMode::Drop => {
                if queue.is_empty() {
                    queue.push(message);
                }
            }
        }
    }

    pub async fn follow_up(&self, message: AgentMessage) {
        let mode = *self.follow_up_mode.read().await;
        let mut queue = self.follow_up_queue.write().await;
        match mode {
            QueueMode::Queue => queue.push(message),
            QueueMode::Replace => {
                queue.clear();
                queue.push(message);
            }
            QueueMode::Drop => {
                if queue.is_empty() {
                    queue.push(message);
                }
            }
        }
    }

    pub async fn abort(&self) -> std::result::Result<AbortResult, HarnessError> {
        let cleared_steer: Vec<AgentMessage> = self.steer_queue.write().await.drain(..).collect();
        let cleared_follow_up: Vec<AgentMessage> =
            self.follow_up_queue.write().await.drain(..).collect();

        if let Some(handle) = self.run_promise.write().await.take() {
            handle.abort();
        }

        Ok(AbortResult {
            cleared_steer,
            cleared_follow_up,
        })
    }

    pub async fn wait_for_idle(&self) {
        let mut rx = self.idle_notify.subscribe();
        let _ = rx.changed().await;
    }

    pub async fn subscribe(&self, listener: HarnessListener<S, P>) {
        self.listeners.write().await.push(listener);
    }

    pub async fn prompt(
        &self,
        text: &str,
        images: Option<Vec<ContentBlock>>,
    ) -> std::result::Result<(), HarnessError> {
        let mut content = vec![ContentBlock::Text {
            text: text.to_string(),
            text_signature: None,
        }];
        if let Some(imgs) = images {
            content.extend(imgs);
        }

        let message = AgentMessage::User {
            content,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        {
            let mut session = self.session.write().await;
            session
                .append_message(message)
                .await
                .map_err(HarnessError::Session)?;
        }

        *self.phase.write().await = HarnessPhase::Running;

        Ok(())
    }

    pub async fn compact(
        &self,
        custom_instructions: Option<&str>,
    ) -> std::result::Result<Option<CompactResult>, HarnessError> {
        let session = self.session.read().await;
        let _context = session
            .build_context()
            .await
            .map_err(HarnessError::Session)?;

        let model = self.model.read().await.clone();
        let settings = crate::harness::types::DEFAULT_COMPACTION_SETTINGS.clone();

        let entries = session.get_entries().await;
        let preparation = crate::harness::compaction::compaction::prepare_compaction(
            &entries,
            model.context_window,
            &settings,
        );

        match preparation {
            Ok(prep) => {
                // Resolve API key from environment
                let api_key =
                    pi_ai::env_api_keys::get_env_api_key(&model.provider).unwrap_or_default();

                let result = crate::harness::compaction::compaction::compact(
                    prep,
                    &model,
                    &api_key,
                    None,
                    custom_instructions,
                    None,
                    None,
                )
                .await
                .map_err(|e| HarnessError::Compaction(e))?;

                {
                    let mut session = self.session.write().await;
                    session
                        .append_compaction(
                            result.summary.clone(),
                            result.first_kept_entry_id.clone(),
                            result.tokens_before,
                            result.details.clone(),
                            Some(true),
                        )
                        .await
                        .map_err(HarnessError::Session)?;
                }

                Ok(Some(result))
            }
            Err(crate::harness::types::CompactionError::NoCompactionNeeded) => Ok(None),
            Err(e) => Err(HarnessError::Compaction(e)),
        }
    }

    pub async fn navigate_tree(
        &self,
        target_id: &str,
        summary: Option<crate::harness::types::MoveToSummary>,
    ) -> std::result::Result<NavigateTreeResult, HarnessError> {
        let mut session = self.session.write().await;
        session
            .move_to(Some(target_id), summary)
            .await
            .map_err(HarnessError::Session)?;

        Ok(NavigateTreeResult {
            cancelled: false,
            editor_text: None,
            summary_entry: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AgentHarnessOptions<S: Clone = Skill, P: Clone = PromptTemplate> {
    pub thinking_level: Option<ThinkingLevel>,
    pub active_tool_names: Option<Vec<String>>,
    pub resources: Option<AgentHarnessResources<S, P>>,
    pub stream_options: Option<AgentHarnessStreamOptions>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
}

impl<S: Clone, P: Clone> Default for AgentHarnessOptions<S, P> {
    fn default() -> Self {
        Self {
            thinking_level: None,
            active_tool_names: None,
            resources: None,
            stream_options: None,
            steering_mode: None,
            follow_up_mode: None,
        }
    }
}
