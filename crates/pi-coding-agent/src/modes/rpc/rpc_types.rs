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
    #[serde(rename_all = "camelCase")]
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
    #[serde(rename_all = "camelCase")]
    Bash {
        #[serde(default)]
        id: Option<String>,
        command: String,
        #[serde(default)]
        exclude_from_context: Option<bool>,
    },

    /// Start a new session
    #[serde(rename_all = "camelCase")]
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
    #[serde(rename_all = "camelCase")]
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
    #[serde(rename_all = "camelCase")]
    SwitchSession {
        #[serde(default)]
        id: Option<String>,
        session_path: String,
    },

    /// Fork the session at an entry
    #[serde(rename_all = "camelCase")]
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

    /// Export session to HTML
    #[serde(rename_all = "camelCase")]
    ExportHtml {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        output_path: Option<String>,
    },

    /// Graceful shutdown
    Shutdown {
        #[serde(default)]
        id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    #[serde(rename = "type")]
    pub image_type: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub data: Option<String>,
}

// ============================================================================
// RPC Responses (stdout)
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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


}

/// Session state returned by get_state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcSessionState {
    pub model: pi_agent_core::pi_ai_types::Model,
    pub thinking_level: String,
    pub is_streaming: bool,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    pub message_count: usize,
    /// Whether the session is currently being compacted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_compacting: Option<bool>,
    /// Steering queue mode: "all" or "one-at-a-time".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering_mode: Option<String>,
    /// Follow-up queue mode: "all" or "one-at-a-time".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_up_mode: Option<String>,
    /// Path to the current session file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    /// Whether auto-compaction is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_compaction_enabled: Option<bool>,
    /// Number of pending (queued) messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_message_count: Option<usize>,
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
