//! MCP protocol types.
//!
//! Port of `packages/mcp/src/protocol/types.ts`. Only the shapes the client
//! uses are modeled; unknown fields are ignored, matching the TS structural
//! validation.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::content::ContentAnnotations;

/// Newest MCP protocol revision this client speaks.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

/// Protocol revisions this client accepts from a server.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// `Implementation` from `initialize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Implementation {
    /// Client/server name.
    pub name: String,
    /// Version string.
    pub version: String,
    /// Optional display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A root the client exposes to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    /// File URI.
    pub uri: String,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Capabilities the client announces.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    /// Experimental capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Map<String, Value>>,
    /// Roots capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    /// Sampling capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<Map<String, Value>>,
    /// Elicitation capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<Map<String, Value>>,
}

/// `roots` capability object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootsCapability {
    /// Whether the client emits `notifications/roots/list_changed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Capabilities a server announces.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    /// Experimental capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Map<String, Value>>,
    /// Logging capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<Map<String, Value>>,
    /// Prompts capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<ListChangedCapability>,
    /// Resources capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    /// Tools capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ListChangedCapability>,
    /// Completions capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completions: Option<Map<String, Value>>,
}

/// A capability object with an optional `listChanged` flag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChangedCapability {
    /// Whether the server emits list change notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// `resources` capability object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
    /// Whether the server supports resource subscriptions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscribe: Option<bool>,
    /// Whether the server emits list change notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// Parameters of `initialize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Requested protocol version.
    pub protocol_version: String,
    /// Client capabilities.
    pub capabilities: ClientCapabilities,
    /// Client implementation info.
    pub client_info: Implementation,
}

/// Result of `initialize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Negotiated protocol version.
    pub protocol_version: String,
    /// Server capabilities.
    pub capabilities: ServerCapabilities,
    /// Server implementation info.
    pub server_info: Implementation,
    /// Optional instructions for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// `notifications/progress` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressNotification {
    /// Token that ties the progress to a request.
    pub progress_token: crate::protocol::jsonrpc::JsonRpcId,
    /// Current progress.
    pub progress: f64,
    /// Optional total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    /// Optional message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `notifications/cancelled` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledNotification {
    /// Request being cancelled.
    pub request_id: crate::protocol::jsonrpc::JsonRpcId,
    /// Optional reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Tool behavior hints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    /// Display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Tool does not modify state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// Tool may perform destructive updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// Repeated calls have no additional effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    /// Tool interacts with external entities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

/// Task execution hints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecution {
    /// Whether the tool supports tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_support: Option<String>,
}

/// A tool offered by a server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    /// Tool name.
    pub name: String,
    /// Optional title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for arguments.
    pub input_schema: Map<String, Value>,
    /// Optional JSON schema for structured output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Map<String, Value>>,
    /// Behavior hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    /// Task execution hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ToolExecution>,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// Result of `tools/list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    /// Tools in this page.
    #[serde(default)]
    pub tools: Vec<Tool>,
    /// Cursor for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// A resource a server lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    /// Resource URI.
    pub uri: String,
    /// Resource name (client fills in the URI when a server omits it).
    pub name: String,
    /// Optional title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Optional size in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ContentAnnotations>,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
    /// Icons (preserved for pass-through, filtered by the resources tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<Value>,
}

/// A resource template addressed by an RFC 6570 URI template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplate {
    /// URI template.
    pub uri_template: String,
    /// Template name.
    pub name: String,
    /// Optional title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional MIME type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Annotations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ContentAnnotations>,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
    /// Icons (preserved for pass-through, filtered by the resources tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<Value>,
}

/// Result of `resources/list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesResult {
    /// Resources in this page.
    #[serde(default)]
    pub resources: Vec<Resource>,
    /// Cursor for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// Result of `resources/templates/list`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourceTemplatesResult {
    /// Templates in this page.
    #[serde(default)]
    pub resource_templates: Vec<ResourceTemplate>,
    /// Cursor for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// Result of `resources/read`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResourceResult {
    /// Resource contents.
    pub contents: Vec<crate::protocol::content::ResourceContents>,
    /// Extension metadata.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}
