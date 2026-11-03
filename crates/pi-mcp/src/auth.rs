//! Bearer-token provider for HTTP transports.
//!
//! Port of `packages/mcp/src/auth-provider.ts`.

use async_trait::async_trait;
use url::Url;

use crate::error::Result;

/// Context for an unauthorized response.
///
/// The TS `AuthProvider.onUnauthorized` receives the response, server URL,
/// fetch function, and the rejected token. Rust passes the status and the
/// `WWW-Authenticate` header instead of the response body because the
/// transport has already consumed it; `http` is the same client so a refresh
/// can make the same requests.
pub struct UnauthorizedContext {
    /// HTTP status (401, or 403 with `insufficient_scope`).
    pub status: u16,
    /// `WWW-Authenticate` header.
    pub www_authenticate: Option<String>,
    /// MCP server URL.
    pub server_url: Url,
    /// Access token the rejected request carried, when any.
    pub token: Option<String>,
    /// HTTP client the transport uses.
    pub http: reqwest::Client,
}

/// Supplies bearer tokens to an MCP HTTP transport and may refresh them after
/// an unauthorized response.
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Token for the next request.
    async fn token(&self) -> Option<String>;

    /// Called once after a 401 (or a 403 asking for `insufficient_scope`).
    async fn on_unauthorized(&self, context: UnauthorizedContext) -> Result<()>;
}

/// An auth provider with no credentials.
pub struct NoAuth;

#[async_trait]
impl AuthProvider for NoAuth {
    async fn token(&self) -> Option<String> {
        None
    }

    async fn on_unauthorized(&self, _context: UnauthorizedContext) -> Result<()> {
        Err(crate::error::McpError::AuthRequired {
            www_authenticate: None,
            body: String::new(),
        })
    }
}
