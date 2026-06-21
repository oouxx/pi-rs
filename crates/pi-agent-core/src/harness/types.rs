use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::pi_ai_types::{ContentBlock, Model, ThinkingLevel};
use crate::types::AgentMessage;

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
    pub transport: Option<String>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub cache_retention: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentHarnessStreamOptionsPatch {
    pub temperature: Option<Option<f64>>,
    pub top_p: Option<Option<f64>>,
    pub max_tokens: Option<Option<u64>>,
    pub transport: Option<Option<String>>,
    pub timeout_ms: Option<Option<u64>>,
    pub max_retries: Option<Option<u32>>,
    pub max_retry_delay_ms: Option<Option<u64>>,
    pub cache_retention: Option<Option<String>>,
    pub headers: Option<Option<HashMap<String, Option<String>>>>,
    pub metadata: Option<Option<HashMap<String, Option<String>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentHarnessPhase {
    Idle,
    Turn,
    Compaction,
    BranchSummary,
}

/// A deferred session write that is queued during an active turn
/// and flushed when the turn ends.
#[derive(Debug, Clone)]
pub enum PendingSessionWrite {
    Message { message: AgentMessage },
    ModelChange { provider: String, model_id: String },
    ThinkingLevelChange { thinking_level: String },
    ActiveToolsChange { active_tool_names: Vec<String> },
    Custom { custom_type: String, data: Option<serde_json::Value> },
    CustomMessage { custom_type: String, content: serde_json::Value, display: bool, details: Option<serde_json::Value> },
    Label { target_id: String, label: Option<String> },
    SessionInfo { name: String },
    Leaf { target_id: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentHarnessOwnEvent<S: Clone = Skill, P: Clone = PromptTemplate> {
    QueueUpdate {
        steer_queue: Vec<AgentMessage>,
        follow_up_queue: Vec<AgentMessage>,
        next_turn_queue: Vec<AgentMessage>,
    },
    SavePoint {
        had_pending_mutations: bool,
    },
    Abort {
        cleared_steer: Vec<AgentMessage>,
        cleared_follow_up: Vec<AgentMessage>,
    },
    Settled {
        next_turn_count: usize,
    },
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
        session_id: String,
        stream_options: AgentHarnessStreamOptions,
    },
    BeforeProviderPayload {
        payload: serde_json::Value,
    },
    AfterProviderResponse {
        model: Model,
        thinking_level: ThinkingLevel,
        status: u16,
        headers: HashMap<String, String>,
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
        from_hook: bool,
    },
    SessionBeforeTree {
        preparation: TreePreparation,
    },
    SessionTree {
        new_leaf_id: Option<String>,
        old_leaf_id: Option<String>,
        summary_entry: Option<BranchSummaryEntry>,
        from_hook: Option<bool>,
    },
    ModelUpdate {
        model: Model,
        previous_model: Model,
        source: String,
    },
    ThinkingLevelUpdate {
        level: ThinkingLevel,
        previous_level: ThinkingLevel,
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

// ============================================================
// AgentHarnessEventResultMap — typed hook return values
// ============================================================

#[derive(Debug, Clone, Default)]
pub struct ContextHookResult {
    pub messages: Vec<AgentMessage>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallHookResult {
    pub block: Option<bool>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolResultHookResult {
    pub content: Option<Vec<ContentBlock>>,
    pub details: Option<serde_json::Value>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BeforeAgentStartHookResult {
    pub system_prompt: Option<String>,
    pub messages: Option<Vec<AgentMessage>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionBeforeCompactHookResult {
    pub cancel: Option<bool>,
    pub compaction: Option<CompactResult>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionBeforeTreeHookResult {
    pub cancel: Option<bool>,
    pub summary: Option<BranchSummaryHookResult>,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BranchSummaryHookResult {
    pub summary: String,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BeforeProviderRequestHookResult {
    pub stream_options: Option<AgentHarnessStreamOptionsPatch>,
}

// ============================================================
// End of EventResultMap types
// ============================================================

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
    #[error("busy: {0}")]
    Busy(String),
    #[error("auth: {0}")]
    Auth(String),
    #[error("invalid_state: {0}")]
    InvalidState(String),
    #[error("branch_summary: {0}")]
    BranchSummary(#[from] BranchSummaryError),
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
    async fn read_binary_file(
        &self,
        path: &str,
    ) -> std::result::Result<Vec<u8>, FileError> {
        // Default: read as text and convert. Override for proper binary reading.
        let content = self.read_text_file(path, None).await?;
        Ok(content.into_bytes())
    }
    async fn read_text_lines(
        &self,
        path: &str,
        options: Option<ReadTextFileOptions>,
    ) -> std::result::Result<Vec<String>, FileError> {
        let content = self.read_text_file(path, options).await?;
        Ok(content.lines().map(|l| l.to_string()).collect())
    }
    async fn join_path(&self, parts: &[&str]) -> std::result::Result<String, FileError> {
        let mut path = std::path::PathBuf::new();
        for part in parts {
            path.push(part);
        }
        Ok(path.to_string_lossy().to_string())
    }
    async fn absolute_path(&self, path: &str) -> std::result::Result<String, FileError> {
        // Default: prepend cwd if path is relative, then canonicalize
        let p = std::path::Path::new(path);
        if p.is_relative() {
            let mut abs = std::path::PathBuf::from(self.cwd());
            abs.push(path);
            Ok(abs.to_string_lossy().to_string())
        } else {
            Ok(path.to_string())
        }
    }
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

// ============================================================
// Helper functions
// ============================================================

pub fn create_user_message(text: &str, images: Option<Vec<ContentBlock>>) -> AgentMessage {
    let mut content = vec![ContentBlock::Text {
        text: text.to_string(),
        text_signature: None,
    }];
    if let Some(imgs) = images {
        content.extend(imgs);
    }
    AgentMessage::User {
        content,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn create_failure_message(model: &Model, error: &str, aborted: bool) -> AgentMessage {
    AgentMessage::Assistant {
        content: vec![ContentBlock::Text {
            text: String::new(),
            text_signature: None,
        }],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        usage: crate::pi_ai_types::Usage::default(),
        stop_reason: Some(if aborted {
            crate::pi_ai_types::StopReason::Aborted
        } else {
            crate::pi_ai_types::StopReason::Error
        }),
        error_message: Some(error.to_string()),
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

pub fn clone_stream_options(options: &AgentHarnessStreamOptions) -> AgentHarnessStreamOptions {
    AgentHarnessStreamOptions {
        temperature: options.temperature,
        top_p: options.top_p,
        max_tokens: options.max_tokens,
        transport: options.transport.clone(),
        timeout_ms: options.timeout_ms,
        max_retries: options.max_retries,
        max_retry_delay_ms: options.max_retry_delay_ms,
        cache_retention: options.cache_retention.clone(),
        headers: options.headers.clone(),
        metadata: options.metadata.clone(),
    }
}

pub fn merge_headers(
    headers: &[Option<HashMap<String, String>>],
) -> Option<HashMap<String, String>> {
    let mut merged: HashMap<String, String> = HashMap::new();
    let mut has_headers = false;
    for entry in headers {
        if let Some(h) = entry {
            for (k, v) in h {
                merged.insert(k.clone(), v.clone());
            }
            has_headers = true;
        }
    }
    if has_headers {
        Some(merged)
    } else {
        None
    }
}

pub fn find_duplicate_names(names: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut duplicates = Vec::new();
    for name in names {
        if !seen.insert(name.clone()) {
            duplicates.push(name.clone());
        }
    }
    duplicates
}

pub fn apply_stream_options_patch(
    base: &AgentHarnessStreamOptions,
    patch: &AgentHarnessStreamOptionsPatch,
) -> AgentHarnessStreamOptions {
    let mut result = clone_stream_options(base);

    if let Some(v) = &patch.temperature {
        result.temperature = *v;
    }
    if let Some(v) = &patch.top_p {
        result.top_p = *v;
    }
    if let Some(v) = &patch.max_tokens {
        result.max_tokens = *v;
    }
    if let Some(v) = &patch.transport {
        result.transport = v.clone();
    }
    if let Some(v) = &patch.timeout_ms {
        result.timeout_ms = *v;
    }
    if let Some(v) = &patch.max_retries {
        result.max_retries = *v;
    }
    if let Some(v) = &patch.max_retry_delay_ms {
        result.max_retry_delay_ms = *v;
    }
    if let Some(v) = &patch.cache_retention {
        result.cache_retention = v.clone();
    }
    if let Some(patch_headers) = &patch.headers {
        match patch_headers {
            None => result.headers = None,
            Some(changes) => {
                let mut headers = result.headers.clone().unwrap_or_default();
                for (key, value) in changes {
                    match value {
                        None => {
                            headers.remove(key);
                        }
                        Some(v) => {
                            headers.insert(key.clone(), v.clone());
                        }
                    }
                }
                result.headers = if headers.is_empty() { None } else { Some(headers) };
            }
        }
    }
    if let Some(patch_meta) = &patch.metadata {
        match patch_meta {
            None => result.metadata = None,
            Some(changes) => {
                let mut metadata = result.metadata.clone().unwrap_or_default();
                for (key, value) in changes {
                    match value {
                        None => {
                            metadata.remove(key);
                        }
                        Some(v) => {
                            metadata.insert(key.clone(), v.clone());
                        }
                    }
                }
                result.metadata = if metadata.is_empty() { None } else { Some(metadata) };
            }
        }
    }

    result
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn test_create_user_message_basic() {
        let msg = create_user_message("hello", None);
        assert!(matches!(msg, AgentMessage::User { .. }));
        if let AgentMessage::User { content, .. } = &msg {
            assert_eq!(content.len(), 1);
            assert!(matches!(content[0], ContentBlock::Text { .. }));
        }
    }

    #[test]
    fn test_create_user_message_with_images() {
        let img = ContentBlock::Image {
            data: "base64data".into(),
            mime_type: "image/png".into(),
        };
        let msg = create_user_message("hello", Some(vec![img]));
        if let AgentMessage::User { content, .. } = &msg {
            assert_eq!(content.len(), 2);
            assert!(matches!(content[1], ContentBlock::Image { .. }));
        }
    }

    #[test]
    fn test_create_failure_message_aborted() {
        let model = Model {
            provider: "test".into(),
            api: "test-api".into(),
            id: "test-model".into(),
            name: "Test Model".into(),
            base_url: "https://test.com".into(),
            context_window: 0,
            max_tokens: 0,
            cost: crate::pi_ai_types::ModelCost::default(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            headers: None,
            compat: None,
        };
        let msg = create_failure_message(&model, "cancelled", true);
        if let AgentMessage::Assistant { error_message, stop_reason, .. } = &msg {
            assert_eq!(error_message.as_deref(), Some("cancelled"));
            assert_eq!(*stop_reason, Some(crate::pi_ai_types::StopReason::Aborted));
        } else {
            panic!("Expected Assistant message");
        }
    }

    #[test]
    fn test_clone_stream_options() {
        let opts = AgentHarnessStreamOptions {
            temperature: Some(0.7),
            top_p: None,
            max_tokens: Some(4096),
            transport: Some("websocket".into()),
            timeout_ms: None,
            max_retries: Some(3),
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: Some(HashMap::from([("X-Key".into(), "val".into())])),
            metadata: None,
        };
        let cloned = clone_stream_options(&opts);
        assert_eq!(cloned.temperature, Some(0.7));
        assert_eq!(cloned.max_tokens, Some(4096));
        assert_eq!(cloned.transport, Some("websocket".into()));
        assert_eq!(cloned.headers.as_ref().unwrap().get("X-Key"), Some(&"val".into()));
    }

    #[test]
    fn test_merge_headers_empty() {
        let result = merge_headers(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_merge_headers_single() {
        let h1 = Some(HashMap::from([("A".into(), "1".into())]));
        let result = merge_headers(&[h1]);
        assert_eq!(result.unwrap().get("A"), Some(&"1".into()));
    }

    #[test]
    fn test_merge_headers_multiple() {
        let h1 = Some(HashMap::from([("A".into(), "1".into())]));
        let h2 = Some(HashMap::from([("B".into(), "2".into())]));
        let result = merge_headers(&[h1, h2]);
        let map = result.unwrap();
        assert_eq!(map.get("A"), Some(&"1".into()));
        assert_eq!(map.get("B"), Some(&"2".into()));
    }

    #[test]
    fn test_find_duplicate_names() {
        let names = vec!["a".into(), "b".into(), "a".into(), "c".into(), "b".into()];
        let dups = find_duplicate_names(&names);
        assert!(dups.contains(&"a".into()));
        assert!(dups.contains(&"b".into()));
        assert_eq!(dups.len(), 2);
    }

    #[test]
    fn test_find_duplicate_names_no_duplicates() {
        let names = vec!["a".into(), "b".into(), "c".into()];
        let dups = find_duplicate_names(&names);
        assert!(dups.is_empty());
    }

    #[test]
    fn test_apply_stream_options_patch() {
        let base = AgentHarnessStreamOptions {
            temperature: Some(0.5),
            top_p: None,
            max_tokens: Some(2048),
            transport: None,
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: None,
            metadata: None,
        };
        let patch = AgentHarnessStreamOptionsPatch {
            temperature: Some(Some(0.7)),
            top_p: Some(None),
            max_tokens: Some(Some(4096)),
            transport: Some(Some("sse".into())),
            timeout_ms: Some(Some(30000)),
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: None,
            metadata: None,
        };
        let result = apply_stream_options_patch(&base, &patch);
        assert_eq!(result.temperature, Some(0.7));
        assert_eq!(result.top_p, None);
        assert_eq!(result.max_tokens, Some(4096));
        assert_eq!(result.transport, Some("sse".into()));
        assert_eq!(result.timeout_ms, Some(30000));
    }

    #[test]
    fn test_apply_stream_options_patch_clear_value() {
        let base = AgentHarnessStreamOptions {
            temperature: Some(0.5),
            top_p: Some(0.9),
            max_tokens: None,
            transport: None,
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: None,
            metadata: None,
        };
        let patch = AgentHarnessStreamOptionsPatch {
            temperature: Some(None),
            top_p: None,
            max_tokens: None,
            transport: None,
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: None,
            metadata: None,
        };
        let result = apply_stream_options_patch(&base, &patch);
        assert_eq!(result.temperature, None);
        assert_eq!(result.top_p, Some(0.9));
    }

    #[test]
    fn test_apply_stream_options_patch_headers() {
        let base = AgentHarnessStreamOptions {
            temperature: None,
            top_p: None,
            max_tokens: None,
            transport: None,
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: Some(HashMap::from([("X-Keep".into(), "val".into())])),
            metadata: None,
        };
        let patch = AgentHarnessStreamOptionsPatch {
            temperature: None,
            top_p: None,
            max_tokens: None,
            transport: None,
            timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            cache_retention: None,
            headers: Some(Some(HashMap::from([
                ("X-Add".into(), Some("new".into())),
                ("X-Keep".into(), None),
            ]))),
            metadata: None,
        };
        let result = apply_stream_options_patch(&base, &patch);
        let headers = result.headers.unwrap();
        assert_eq!(headers.get("X-Add"), Some(&"new".into()));
        assert!(headers.get("X-Keep").is_none());
    }
}
