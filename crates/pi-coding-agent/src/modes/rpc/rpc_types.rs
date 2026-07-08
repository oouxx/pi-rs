//! RPC protocol types for headless operation.
//!
//! Commands are sent as JSON lines on stdin.
//! Responses and events are emitted as JSON lines on stdout.
//!
//! Mirrors packages/coding-agent/src/modes/rpc/rpc-types.ts

use serde::{Deserialize, Serialize};

// ============================================================================
// RPC Commands (stdin)
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RpcCommand {
    /// Send a prompt to the agent
    Prompt {
        #[serde(default)]
        id: Option<String>,
        message: String,
        #[serde(default)]
        images: Option<Vec<ImageRef>>,
        #[serde(default)]
        streaming_behavior: Option<String>,
    },

    /// Queue a steering message (interrupts current stream)
    Steer {
        #[serde(default)]
        id: Option<String>,
        message: String,
        #[serde(default)]
        images: Option<Vec<ImageRef>>,
    },

    /// Queue a follow-up message (waits for current stream)
    FollowUp {
        #[serde(default)]
        id: Option<String>,
        message: String,
        #[serde(default)]
        images: Option<Vec<ImageRef>>,
    },

    /// Abort current agent operation
    Abort {
        #[serde(default)]
        id: Option<String>,
    },

    /// Abort a running bash command
    AbortBash {
        #[serde(default)]
        id: Option<String>,
    },

    /// Execute a bash command via the agent
    Bash {
        #[serde(default)]
        id: Option<String>,
        command: String,
        #[serde(default)]
        exclude_from_context: Option<bool>,
    },

    /// Start a new session
    NewSession {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        parent_session: Option<String>,
    },

    /// Get current session state
    GetState {
        #[serde(default)]
        id: Option<String>,
    },

    /// Change model
    SetModel {
        #[serde(default)]
        id: Option<String>,
        provider: String,
        #[serde(rename = "modelId")]
        model_id: String,
    },

    /// Cycle to next available model
    CycleModel {
        #[serde(default)]
        id: Option<String>,
    },

    /// List available models
    GetAvailableModels {
        #[serde(default)]
        id: Option<String>,
    },

    /// Change thinking level
    SetThinkingLevel {
        #[serde(default)]
        id: Option<String>,
        level: String,
    },

    /// Cycle thinking level
    CycleThinkingLevel {
        #[serde(default)]
        id: Option<String>,
    },

    /// Set steering queue mode
    SetSteeringMode {
        #[serde(default)]
        id: Option<String>,
        mode: String,
    },

    /// Set follow-up queue mode
    SetFollowUpMode {
        #[serde(default)]
        id: Option<String>,
        mode: String,
    },

    /// Get all messages
    GetMessages {
        #[serde(default)]
        id: Option<String>,
    },

    /// Get session entries
    GetEntries {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        since: Option<String>,
    },

    /// Get session tree
    GetTree {
        #[serde(default)]
        id: Option<String>,
    },

    /// Set session name
    SetSessionName {
        #[serde(default)]
        id: Option<String>,
        name: String,
    },

    /// Set auto compaction
    SetAutoCompaction {
        #[serde(default)]
        id: Option<String>,
        enabled: bool,
    },

    /// Compact the session
    Compact {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        custom_instructions: Option<String>,
    },

    /// Set auto retry
    SetAutoRetry {
        #[serde(default)]
        id: Option<String>,
        enabled: bool,
    },

    /// Abort retry
    AbortRetry {
        #[serde(default)]
        id: Option<String>,
    },

    /// Get session statistics
    GetSessionStats {
        #[serde(default)]
        id: Option<String>,
    },

    /// Switch to another session file
    SwitchSession {
        #[serde(default)]
        id: Option<String>,
        session_path: String,
    },

    /// Fork the session at an entry
    Fork {
        #[serde(default)]
        id: Option<String>,
        entry_id: String,
    },

    /// Clone the session (fork at current leaf)
    Clone {
        #[serde(default)]
        id: Option<String>,
    },

    /// Get messages for forking
    GetForkMessages {
        #[serde(default)]
        id: Option<String>,
    },

    /// Get last assistant text
    GetLastAssistantText {
        #[serde(default)]
        id: Option<String>,
    },

    /// Get available commands (slash commands, skills, prompt templates)
    GetCommands {
        #[serde(default)]
        id: Option<String>,
    },

    /// Graceful shutdown
    Shutdown {
        #[serde(default)]
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRef {
    #[serde(rename = "type")]
    pub image_type: String,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
}

// ============================================================================
// RPC Responses (stdout)
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RpcOutput {
    /// Command response
    Response {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        command: String,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// Agent event streamed to the client
    Event {
        #[serde(flatten)]
        event: AgentEvent,
    },

    /// Extension UI request forwarded to the client
    ExtensionUiRequest {
        #[serde(flatten)]
        request: ExtensionUiRequestData,
    },

    /// Error from extension execution
    ExtensionError {
        extension_path: String,
        event: String,
        error: String,
    },
}

#[derive(Debug, Serialize)]
pub struct ExtensionUiRequestData {
    pub r#type: String,
    pub id: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notify_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
}

/// Serializable agent event for streaming to RPC clients.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    MessageStart,
    MessageUpdate {
        delta: String,
    },
    MessageEnd,
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: serde_json::Value,
        is_error: bool,
    },
    AgentEnd,
}

#[derive(Debug, Serialize)]
pub struct RpcSessionState {
    pub model: String,
    pub thinking_level: String,
    pub is_streaming: bool,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub message_count: usize,
}

/// Helper to create a success response.
pub fn rpc_success(id: Option<String>, command: &str, data: Option<serde_json::Value>) -> RpcOutput {
    RpcOutput::Response {
        id,
        command: command.to_string(),
        success: true,
        data,
        error: None,
    }
}

/// Helper to create an error response.
pub fn rpc_error(id: Option<String>, command: &str, error: String) -> RpcOutput {
    RpcOutput::Response {
        id,
        command: command.to_string(),
        success: false,
        data: None,
        error: Some(error),
    }
}
