use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::pi_ai_types::{Model, ThinkingLevel};
use crate::types::{AgentEvent, AgentMessage};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub file_path: String,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentHarnessResources<S: Clone = Skill, P: Clone = PromptTemplate> {
    pub skills: Option<Vec<S>>,
    pub prompt_templates: Option<Vec<P>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum QueueMode {
    Queue,
    Replace,
    Drop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentHarnessStreamOptions {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentHarnessStreamOptionsPatch {
    pub temperature: Option<Option<f64>>,
    pub top_p: Option<Option<f64>>,
    pub max_tokens: Option<Option<u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentHarnessOwnEvent<S: Clone = Skill, P: Clone = PromptTemplate> {
    QueueUpdate {
        steer_queue: Vec<AgentMessage>,
        follow_up_queue: Vec<AgentMessage>,
    },
    SavePoint,
    Abort {
        cleared_steer: Vec<AgentMessage>,
        cleared_follow_up: Vec<AgentMessage>,
    },
    Settled,
    BeforeAgentStart {
        env: ExecutionEnvInfo,
        session: SessionInfo,
        model: Model,
        thinking_level: ThinkingLevel,
        active_tools: Vec<String>,
        resources: AgentHarnessResources<S, P>,
    },
    Context {
        messages: Vec<AgentMessage>,
    },
    BeforeProviderRequest {
        model: Model,
        thinking_level: ThinkingLevel,
    },
    BeforeProviderPayload {
        payload: serde_json::Value,
    },
    AfterProviderResponse {
        model: Model,
        thinking_level: ThinkingLevel,
    },
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        result: serde_json::Value,
        is_error: bool,
    },
    SessionBeforeCompact {
        preparation: CompactionPreparation,
    },
    SessionCompact {
        result: CompactResult,
    },
    SessionBeforeTree {
        preparation: TreePreparation,
    },
    SessionTree {
        summary: Option<String>,
    },
    ModelUpdate {
        model: Model,
        previous_model: Model,
    },
    ThinkingLevelUpdate {
        thinking_level: ThinkingLevel,
        previous_thinking_level: ThinkingLevel,
    },
    ResourcesUpdate {
        resources: AgentHarnessResources<S, P>,
        previous_resources: AgentHarnessResources<S, P>,
    },
    ToolsUpdate {
        tool_names: Vec<String>,
        previous_tool_names: Vec<String>,
        active_tool_names: Vec<String>,
        previous_active_tool_names: Vec<String>,
        source: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentHarnessEvent<S: Clone = Skill, P: Clone = PromptTemplate> {
    Agent(AgentEvent),
    Own(AgentHarnessOwnEvent<S, P>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionEnvInfo {
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 8192,
        }
    }
}

pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 8192,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: String,
    pub messages_to_summarize: Vec<AgentMessage>,
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
    pub settings: CompactionSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileOperations {
    pub read: Vec<String>,
    pub written: Vec<String>,
    pub edited: Vec<String>,
}

impl FileOperations {
    pub fn new() -> Self {
        Self {
            read: Vec::new(),
            written: Vec::new(),
            edited: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TreePreparation {
    pub target_id: String,
    pub old_leaf_id: Option<String>,
    pub common_ancestor_id: Option<String>,
    pub entries_to_summarize: Vec<SessionTreeEntry>,
    pub user_wants_summary: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbortResult {
    pub cleared_steer: Vec<AgentMessage>,
    pub cleared_follow_up: Vec<AgentMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NavigateTreeResult {
    pub cancelled: bool,
    pub editor_text: Option<String>,
    pub summary_entry: Option<BranchSummaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum SessionTreeEntry {
    Message {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        message: AgentMessage,
    },
    ThinkingLevelChange {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        thinking_level: String,
    },
    ModelChange {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        provider: String,
        model_id: String,
    },
    ActiveToolsChange {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        active_tool_names: Vec<String>,
    },
    Compaction {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u64,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    },
    Custom {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        data: Option<serde_json::Value>,
    },
    CustomMessage {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        content: serde_json::Value,
        display: bool,
        details: Option<serde_json::Value>,
    },
    Label {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        target_id: String,
        label: Option<String>,
    },
    SessionInfo {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        name: String,
    },
    Leaf {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        target_id: Option<String>,
    },
    BranchSummary {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        from_id: String,
        summary: String,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    },
}

impl SessionTreeEntry {
    pub fn id(&self) -> &str {
        match self {
            SessionTreeEntry::Message { id, .. } => id,
            SessionTreeEntry::ThinkingLevelChange { id, .. } => id,
            SessionTreeEntry::ModelChange { id, .. } => id,
            SessionTreeEntry::ActiveToolsChange { id, .. } => id,
            SessionTreeEntry::Compaction { id, .. } => id,
            SessionTreeEntry::Custom { id, .. } => id,
            SessionTreeEntry::CustomMessage { id, .. } => id,
            SessionTreeEntry::Label { id, .. } => id,
            SessionTreeEntry::SessionInfo { id, .. } => id,
            SessionTreeEntry::Leaf { id, .. } => id,
            SessionTreeEntry::BranchSummary { id, .. } => id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionTreeEntry::Message { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::ThinkingLevelChange { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::ModelChange { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::ActiveToolsChange { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::Compaction { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::Custom { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::CustomMessage { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::Label { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::SessionInfo { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::Leaf { parent_id, .. } => parent_id.as_deref(),
            SessionTreeEntry::BranchSummary { parent_id, .. } => parent_id.as_deref(),
        }
    }

    pub fn entry_type(&self) -> &str {
        match self {
            SessionTreeEntry::Message { .. } => "message",
            SessionTreeEntry::ThinkingLevelChange { .. } => "thinking_level_change",
            SessionTreeEntry::ModelChange { .. } => "model_change",
            SessionTreeEntry::ActiveToolsChange { .. } => "active_tools_change",
            SessionTreeEntry::Compaction { .. } => "compaction",
            SessionTreeEntry::Custom { .. } => "custom",
            SessionTreeEntry::CustomMessage { .. } => "custom_message",
            SessionTreeEntry::Label { .. } => "label",
            SessionTreeEntry::SessionInfo { .. } => "session_info",
            SessionTreeEntry::Leaf { .. } => "leaf",
            SessionTreeEntry::BranchSummary { .. } => "branch_summary",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchSummaryEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub timestamp: String,
    pub from_id: String,
    pub summary: String,
    pub details: Option<serde_json::Value>,
    pub from_hook: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: String,
    pub cwd: Option<String>,
    pub parent_session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub active_tool_names: Vec<String>,
    pub thinking_level: Option<String>,
    pub model: Option<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("not_found: {0}")]
    NotFound(String),
    #[error("invalid_session: {0}")]
    InvalidSession(String),
    #[error("storage: {0}")]
    Storage(String),
    #[error("invalid_fork_target: {0}")]
    InvalidForkTarget(String),
    #[error("invalid_argument: {0}")]
    InvalidArgument(String),
}

#[derive(Debug, Error)]
pub enum CompactionError {
    #[error("invalid_session: {0}")]
    InvalidSession(String),
    #[error("aborted: {0}")]
    Aborted(String),
    #[error("summarization_failed: {0}")]
    SummarizationFailed(String),
    #[error("no_compaction_needed")]
    NoCompactionNeeded,
}

#[derive(Debug, Error)]
pub enum BranchSummaryError {
    #[error("aborted: {0}")]
    Aborted(String),
    #[error("summarization_failed: {0}")]
    SummarizationFailed(String),
}

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("session: {0}")]
    Session(#[from] SessionError),
    #[error("compaction: {0}")]
    Compaction(#[from] CompactionError),
    #[error("invalid_argument: {0}")]
    InvalidArgument(String),
    #[error("hook: {0}")]
    Hook(String),
    #[error("agent: {0}")]
    Agent(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Result<T, E> {
    pub ok: bool,
    pub value: Option<T>,
    pub error: Option<E>,
}

impl<T, E> Result<T, E> {
    pub fn ok(value: T) -> Self {
        Self {
            ok: true,
            value: Some(value),
            error: None,
        }
    }

    pub fn err(error: E) -> Self {
        Self {
            ok: false,
            value: None,
            error: Some(error),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.ok
    }

    pub fn unwrap(self) -> T {
        self.value.expect("called unwrap on Err")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileInfoType {
    pub kind: String,
    pub name: String,
    pub path: String,
}

pub struct ExecutionEnvExecOptions {
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub on_stdout: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pub on_stderr: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("unknown: {0}")]
    Unknown(String),
    #[error("aborted: {0}")]
    Aborted(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("not_found: {0}")]
    NotFound(String),
}

#[async_trait]
pub trait ExecutionEnv: Send + Sync {
    fn cwd(&self) -> &str;
    async fn read_text_file(
        &self,
        path: &str,
        options: Option<ReadTextFileOptions>,
    ) -> std::result::Result<String, FileError>;
    async fn write_file(
        &self,
        path: &str,
        content: &str,
        abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> std::result::Result<(), FileError>;
    async fn append_file(&self, path: &str, content: &str) -> std::result::Result<(), FileError>;
    async fn file_info(&self, path: &str) -> std::result::Result<FileInfoType, FileError>;
    async fn list_dir(&self, path: &str) -> std::result::Result<Vec<FileInfoType>, FileError>;
    async fn canonical_path(&self, path: &str) -> std::result::Result<String, FileError>;
    async fn exists(&self, path: &str) -> std::result::Result<bool, FileError>;
    async fn create_dir(
        &self,
        path: &str,
        options: Option<CreateDirOptions>,
    ) -> std::result::Result<(), FileError>;
    async fn remove(
        &self,
        path: &str,
        options: Option<RemoveOptions>,
    ) -> std::result::Result<(), FileError>;
    async fn create_temp_dir(&self, prefix: &str) -> std::result::Result<String, FileError>;
    async fn create_temp_file(
        &self,
        options: Option<TempFileOptions>,
    ) -> std::result::Result<String, FileError>;
    async fn exec(
        &self,
        command: &str,
        options: ExecutionEnvExecOptions,
    ) -> std::result::Result<ExecResult, ExecutionError>;
    async fn cleanup(&self);
}

#[derive(Debug, Clone)]
pub struct ReadTextFileOptions {
    pub max_lines: Option<usize>,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
}

#[derive(Debug, Clone)]
pub struct CreateDirOptions {
    pub recursive: bool,
}

#[derive(Debug, Clone)]
pub struct RemoveOptions {
    pub recursive: bool,
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct TempFileOptions {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait SessionStorage<M: Clone + Send + Sync = SessionMetadata>: Send + Sync {
    async fn get_metadata(&self) -> M;
    async fn get_leaf_id(&self) -> Option<String>;
    async fn set_leaf_id(
        &mut self,
        leaf_id: Option<String>,
    ) -> std::result::Result<(), SessionError>;
    async fn create_entry_id(&self) -> String;
    async fn append_entry(
        &mut self,
        entry: SessionTreeEntry,
    ) -> std::result::Result<(), SessionError>;
    async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry>;
    async fn find_entries(&self, entry_type: &str) -> Vec<SessionTreeEntry>;
    async fn get_label(&self, id: &str) -> Option<String>;
    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> std::result::Result<Vec<SessionTreeEntry>, SessionError>;
    async fn get_entries(&self) -> Vec<SessionTreeEntry>;
}

#[async_trait]
pub trait SessionRepo<M: Clone + Send + Sync = SessionMetadata>: Send + Sync {
    async fn create(
        &mut self,
        options: SessionCreateOptions,
    ) -> std::result::Result<Session<M>, SessionError>;
    async fn open(&self, metadata: &M) -> std::result::Result<Session<M>, SessionError>;
    async fn list(&self) -> std::result::Result<Vec<M>, SessionError>;
    async fn delete(&mut self, metadata: &M) -> std::result::Result<(), SessionError>;
    async fn fork(
        &mut self,
        source_metadata: &M,
        options: ForkOptions,
    ) -> std::result::Result<Session<M>, SessionError>;
}

#[derive(Debug, Clone)]
pub struct SessionCreateOptions {
    pub id: Option<String>,
    pub cwd: String,
    pub parent_session_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ForkOptions {
    pub entry_id: Option<String>,
    pub position: Option<String>,
    pub id: Option<String>,
    pub cwd: String,
    pub parent_session_path: Option<String>,
}

pub struct Session<M: Clone + Send + Sync = SessionMetadata> {
    storage: Arc<tokio::sync::RwLock<Box<dyn SessionStorage<M>>>>,
}

impl<M: Clone + Send + Sync + 'static> Clone for Session<M> {
    fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
        }
    }
}

impl<M: Clone + Send + Sync + 'static> Session<M> {
    pub fn new(storage: Box<dyn SessionStorage<M>>) -> Self {
        Self {
            storage: Arc::new(tokio::sync::RwLock::new(storage)),
        }
    }

    pub async fn get_metadata(&self) -> M {
        self.storage.read().await.get_metadata().await
    }

    pub async fn get_storage(
        &self,
    ) -> tokio::sync::RwLockReadGuard<'_, Box<dyn SessionStorage<M>>> {
        self.storage.read().await
    }

    pub async fn get_leaf_id(&self) -> Option<String> {
        self.storage.read().await.get_leaf_id().await
    }

    pub async fn set_leaf_id(
        &mut self,
        leaf_id: Option<String>,
    ) -> std::result::Result<(), SessionError> {
        self.storage.write().await.set_leaf_id(leaf_id).await
    }

    pub async fn create_entry_id(&self) -> String {
        self.storage.read().await.create_entry_id().await
    }

    pub async fn append_entry(
        &mut self,
        entry: SessionTreeEntry,
    ) -> std::result::Result<(), SessionError> {
        self.storage.write().await.append_entry(entry).await
    }

    pub async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry> {
        self.storage.read().await.get_entry(id).await
    }

    pub async fn find_entries(&self, entry_type: &str) -> Vec<SessionTreeEntry> {
        self.storage.read().await.find_entries(entry_type).await
    }

    pub async fn get_label(&self, id: &str) -> Option<String> {
        self.storage.read().await.get_label(id).await
    }

    pub async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> std::result::Result<Vec<SessionTreeEntry>, SessionError> {
        self.storage.read().await.get_path_to_root(leaf_id).await
    }

    pub async fn get_entries(&self) -> Vec<SessionTreeEntry> {
        self.storage.read().await.get_entries().await
    }

    pub async fn get_branch(
        &self,
        from_id: Option<&str>,
    ) -> std::result::Result<Vec<SessionTreeEntry>, SessionError> {
        let leaf_id = match from_id {
            Some(id) => Some(id.to_string()),
            None => self.storage.read().await.get_leaf_id().await,
        };
        self.storage
            .read()
            .await
            .get_path_to_root(leaf_id.as_deref())
            .await
    }

    pub async fn build_context(&self) -> std::result::Result<SessionContext, SessionError> {
        let branch = self.get_branch(None).await?;
        Ok(build_session_context(&branch))
    }

    pub async fn append_message(
        &mut self,
        message: AgentMessage,
    ) -> std::result::Result<String, SessionError> {
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::Message {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            message,
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn append_thinking_level_change(
        &mut self,
        thinking_level: String,
    ) -> std::result::Result<String, SessionError> {
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::ThinkingLevelChange {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            thinking_level,
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn append_model_change(
        &mut self,
        provider: String,
        model_id: String,
    ) -> std::result::Result<String, SessionError> {
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::ModelChange {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            provider,
            model_id,
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn append_active_tools_change(
        &mut self,
        active_tool_names: Vec<String>,
    ) -> std::result::Result<String, SessionError> {
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::ActiveToolsChange {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            active_tool_names,
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn append_compaction(
        &mut self,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u64,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> std::result::Result<String, SessionError> {
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::Compaction {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn append_custom_entry(
        &mut self,
        custom_type: String,
        data: Option<serde_json::Value>,
    ) -> std::result::Result<String, SessionError> {
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::Custom {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            custom_type,
            data,
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn append_label(
        &mut self,
        target_id: String,
        label: Option<String>,
    ) -> std::result::Result<String, SessionError> {
        if self
            .storage
            .read()
            .await
            .get_entry(&target_id)
            .await
            .is_none()
        {
            return Err(SessionError::NotFound(format!(
                "Entry {} not found",
                target_id
            )));
        }
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::Label {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            target_id,
            label,
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn append_session_name(
        &mut self,
        name: String,
    ) -> std::result::Result<String, SessionError> {
        let id = self.storage.read().await.create_entry_id().await;
        let leaf_id = self.storage.read().await.get_leaf_id().await;
        let entry = SessionTreeEntry::SessionInfo {
            id: id.clone(),
            parent_id: leaf_id,
            timestamp: chrono::Utc::now().to_rfc3339(),
            name: name.trim().to_string(),
        };
        self.storage.write().await.append_entry(entry).await?;
        Ok(id)
    }

    pub async fn move_to(
        &mut self,
        entry_id: Option<&str>,
        summary: Option<MoveToSummary>,
    ) -> std::result::Result<Option<String>, SessionError> {
        if let Some(id) = entry_id {
            if self.storage.read().await.get_entry(id).await.is_none() {
                return Err(SessionError::NotFound(format!("Entry {} not found", id)));
            }
        }
        self.storage
            .write()
            .await
            .set_leaf_id(entry_id.map(|s| s.to_string()))
            .await?;

        if let Some(s) = summary {
            let id = self.storage.read().await.create_entry_id().await;
            let entry = SessionTreeEntry::BranchSummary {
                id: id.clone(),
                parent_id: entry_id.map(|s| s.to_string()),
                timestamp: chrono::Utc::now().to_rfc3339(),
                from_id: entry_id.unwrap_or("root").to_string(),
                summary: s.summary,
                details: s.details,
                from_hook: s.from_hook,
            };
            self.storage.write().await.append_entry(entry).await?;
            return Ok(Some(id));
        }
        Ok(None)
    }
}

#[derive(Debug, Clone)]
pub struct MoveToSummary {
    pub summary: String,
    pub details: Option<serde_json::Value>,
    pub from_hook: Option<bool>,
}

pub fn build_session_context(entries: &[SessionTreeEntry]) -> SessionContext {
    let mut messages = Vec::new();
    let mut active_tool_names = Vec::new();
    let mut thinking_level = None;
    let mut model = None;

    for entry in entries {
        match entry {
            SessionTreeEntry::Message { message, .. } => {
                messages.push(message.clone());
            }
            SessionTreeEntry::ActiveToolsChange {
                active_tool_names: names,
                ..
            } => {
                active_tool_names = names.clone();
            }
            SessionTreeEntry::ThinkingLevelChange {
                thinking_level: tl, ..
            } => {
                thinking_level = Some(tl.clone());
            }
            SessionTreeEntry::ModelChange {
                provider, model_id, ..
            } => {
                model = Some(ModelInfo {
                    provider: provider.clone(),
                    model_id: model_id.clone(),
                });
            }
            SessionTreeEntry::Compaction { summary, .. } => {
                messages.push(AgentMessage::CompactionSummary {
                    summary: summary.clone(),
                    tokens_before: 0,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                });
            }
            SessionTreeEntry::BranchSummary {
                summary, from_id, ..
            } => {
                messages.push(AgentMessage::BranchSummary {
                    summary: summary.clone(),
                    from_id: from_id.clone(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                });
            }
            _ => {}
        }
    }

    SessionContext {
        messages,
        active_tool_names,
        thinking_level,
        model,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShellCaptureResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    pub full_output_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GenerateBranchSummaryOptions {
    pub model: Model,
    pub reserve_tokens: Option<u64>,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchSummaryResult {
    pub summary: String,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}
