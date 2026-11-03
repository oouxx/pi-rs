//! Error types for the MCP client.
//!
//! Mirrors the TypeScript error classes in `protocol/jsonrpc.ts`,
//! `transports/streamable-http.ts`, and `oauth/errors.ts`, collapsed into one
//! enum so transports, the client, and the coding-agent runtime can match on
//! the failure kind.

use std::time::Duration;

use serde_json::Value;

/// Result alias used throughout `pi-mcp`.
pub type Result<T> = std::result::Result<T, McpError>;

/// A JSON-RPC error reported by the MCP server.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcError {
    /// JSON-RPC error code.
    pub code: i64,
    /// Human-readable message.
    pub message: String,
    /// Optional structured data.
    pub data: Option<Value>,
}

/// Errors produced by the MCP client, transports, and OAuth flows.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// A request exceeded its timeout.
    #[error("MCP request timed out after {0:?}")]
    Timeout(Duration),
    /// A request was cancelled by the caller.
    #[error("MCP request aborted")]
    Aborted,
    /// The connection is closed or not ready.
    #[error("MCP connection closed: {0}")]
    ConnectionClosed(String),
    /// The server answered with a JSON-RPC error.
    #[error("MCP JSON-RPC error {code}: {message}")]
    JsonRpc {
        /// JSON-RPC code.
        code: i64,
        /// Human-readable message.
        message: String,
        /// Optional structured data.
        data: Option<Value>,
    },
    /// The HTTP transport got a non-success status.
    #[error("MCP HTTP request failed with status {status}{display}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Response body (bounded by the transport).
        body: String,
        /// Formatted suffix for `Display` (`": snippet"` or empty).
        display: String,
    },
    /// The server requires authentication.
    #[error("MCP server requires authentication")]
    AuthRequired {
        /// `WWW-Authenticate` header, when present.
        www_authenticate: Option<String>,
        /// Response body.
        body: String,
    },
    /// The server no longer knows the session (404 with a session id).
    #[error("MCP session expired")]
    SessionExpired {
        /// Response body.
        body: String,
    },
    /// A response did not match the MCP schema.
    #[error("Invalid MCP response: {0}")]
    InvalidResponse(String),
    /// Transport-level failure (spawn failure, broken pipe, ...).
    #[error("MCP transport error: {0}")]
    Transport(String),
    /// Network failure (connection reset, DNS, ...).
    #[error("MCP network error: {0}")]
    Network(String),
    /// Filesystem or process I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON serialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// OAuth failure.
    #[error(transparent)]
    OAuth(Box<crate::oauth::errors::OAuthError>),
}

impl From<crate::oauth::errors::OAuthError> for McpError {
    fn from(error: crate::oauth::errors::OAuthError) -> Self {
        Self::OAuth(Box::new(error))
    }
}

impl McpError {
    /// A JSON-RPC error with the given code.
    pub fn json_rpc(code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self::JsonRpc {
            code,
            message: message.into(),
            data,
        }
    }

    /// `-32600`, used for malformed messages and invalid results.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::json_rpc(crate::protocol::jsonrpc::JSON_RPC_ERROR_CODES.invalid_request, message, None)
    }

    /// Whether this is a JSON-RPC `method not found` error.
    pub fn is_method_not_found(&self) -> bool {
        matches!(
            self,
            Self::JsonRpc { code, .. }
                if *code == crate::protocol::jsonrpc::JSON_RPC_ERROR_CODES.method_not_found
        )
    }

    /// Whether this is a transient HTTP failure worth retrying.
    pub fn is_transient_http(&self) -> bool {
        matches!(self, Self::Http { status, .. } if *status == 408 || *status == 429 || (*status >= 500 && *status != 501))
    }
}

impl From<reqwest::Error> for McpError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout(Duration::ZERO)
        } else {
            Self::Network(error.to_string())
        }
    }
}
