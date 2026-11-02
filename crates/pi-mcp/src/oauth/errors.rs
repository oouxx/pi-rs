//! OAuth error types, ported from `packages/mcp/src/oauth/errors.ts`.

/// Errors produced by the MCP OAuth flow.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// An OAuth error response from the authorization server.
    #[error("{message}")]
    OAuth {
        /// OAuth error code (`invalid_grant`, ...).
        code: String,
        /// Human-readable message.
        message: String,
        /// Optional `error_uri`.
        error_uri: Option<String>,
    },
    /// The discovered issuer does not match the authorization server URL.
    #[error("OAuth issuer mismatch: expected {}, received {}", format_issuer(expected), format_issuer(received))]
    IssuerMismatch {
        /// Expected issuer.
        expected: Option<String>,
        /// Received issuer.
        received: Option<String>,
    },
    /// An endpoint that receives credentials is not HTTPS.
    #[error("Refusing to send OAuth credentials to non-HTTPS endpoint {endpoint}")]
    InsecureEndpoint {
        /// The offending endpoint.
        endpoint: String,
    },
    /// Dynamic client registration failed.
    #[error("OAuth dynamic client registration failed with status {status}: {body}")]
    Registration {
        /// HTTP status.
        status: u16,
        /// Response body.
        body: String,
    },
    /// The user has to authorize in a browser.
    #[error("MCP OAuth authorization requires user interaction")]
    AuthorizationRequired,
    /// The user cancelled sign-in.
    #[error("Sign-in cancelled")]
    SignInCancelled,
    /// Network failure (connection, timeout).
    #[error("OAuth network error: {0}")]
    Network(String),
    /// Any other OAuth failure (validation, HTTP, JSON).
    #[error("OAuth error: {0}")]
    Message(String),
}

fn format_issuer(issuer: &Option<String>) -> String {
    match issuer {
        Some(value) => format!("{value:?}"),
        None => "none".to_string(),
    }
}

impl OAuthError {
    /// Build an OAuth error response.
    pub fn oauth(code: impl Into<String>, message: impl Into<String>, error_uri: Option<String>) -> Self {
        Self::OAuth {
            code: code.into(),
            message: message.into(),
            error_uri,
        }
    }

    /// The OAuth error code, when this is an OAuth error response.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::OAuth { code, .. } => Some(code),
            _ => None,
        }
    }

    /// Whether this is `invalid_client`.
    pub fn is_invalid_client(&self) -> bool {
        self.code() == Some("invalid_client")
    }

    /// Whether this is `unauthorized_client`.
    pub fn is_unauthorized_client(&self) -> bool {
        self.code() == Some("unauthorized_client")
    }

    /// Whether this is `invalid_grant`.
    pub fn is_invalid_grant(&self) -> bool {
        self.code() == Some("invalid_grant")
    }
}

impl From<reqwest::Error> for OAuthError {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error.to_string())
    }
}

impl From<serde_json::Error> for OAuthError {
    fn from(error: serde_json::Error) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<url::ParseError> for OAuthError {
    fn from(error: url::ParseError) -> Self {
        Self::Message(error.to_string())
    }
}
