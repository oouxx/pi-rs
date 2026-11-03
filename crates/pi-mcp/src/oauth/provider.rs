//! OAuth provider state and the default stateful provider.
//!
//! Port of `packages/mcp/src/oauth/provider.ts`.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use url::Url;

use crate::oauth::errors::OAuthError;
use crate::oauth::flow::{
    InvalidateKind, OAuthClientMetadataDocument, OAuthClientProvider,
};
use crate::oauth::types::{
    AuthorizationServerMetadata, OAuthClientInformation, OAuthClientMetadata, OAuthDiscoveryState,
    OAuthTokens,
};

/// Stored OAuth state for one MCP server URL.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthState {
    /// Server URL this state belongs to.
    pub server_url: String,
    /// Registered client information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_information: Option<OAuthClientInformation>,
    /// Stored tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<OAuthTokens>,
    /// When the access token expires (milliseconds since the epoch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_expire_at: Option<i64>,
    /// Pending PKCE verifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_verifier: Option<String>,
    /// Pending `state` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_state: Option<String>,
    /// Cached discovery state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery: Option<OAuthDiscoveryState>,
}

/// Durable storage for one server's OAuth state.
#[async_trait]
pub trait McpOAuthStateStore: Send + Sync {
    /// Load the state.
    async fn load(&self) -> Option<McpOAuthState>;
    /// Save the state.
    async fn save(&self, state: McpOAuthState);
}

/// In-memory store.
#[derive(Default)]
pub struct MemoryOAuthStateStore {
    value: Mutex<Option<McpOAuthState>>,
}

impl MemoryOAuthStateStore {
    /// Create an empty store.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl McpOAuthStateStore for MemoryOAuthStateStore {
    async fn load(&self) -> Option<McpOAuthState> {
        self.value.lock().await.clone()
    }

    async fn save(&self, state: McpOAuthState) {
        *self.value.lock().await = Some(state);
    }
}

type ClientMetadataDocumentFn =
    Arc<dyn Fn(Option<&AuthorizationServerMetadata>) -> Option<OAuthClientMetadataDocument> + Send + Sync>;

/// Options for [`McpOAuthProvider`].
#[derive(Clone)]
pub struct McpOAuthProviderOptions {
    /// MCP server URL.
    pub server_url: Url,
    /// Redirect URI.
    pub redirect_url: String,
    /// Client metadata.
    pub client_metadata: OAuthClientMetadata,
    /// Optional Client ID Metadata Document selector.
    pub client_metadata_document: Option<ClientMetadataDocumentFn>,
    /// Pre-registered client id.
    pub client_id: Option<String>,
    /// Pre-registered client secret.
    pub client_secret: Option<String>,
    /// State store.
    pub store: Option<Arc<dyn McpOAuthStateStore>>,
    /// Called with the authorization URL.
    pub on_redirect: Arc<dyn Fn(Url) + Send + Sync>,
}

impl std::fmt::Debug for McpOAuthProviderOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthProviderOptions")
            .field("server_url", &self.server_url)
            .field("redirect_url", &self.redirect_url)
            .field("client_id", &self.client_id)
            .field("has_client_secret", &self.client_secret.is_some())
            .finish()
    }
}

/// Default stateful OAuth provider for one exact MCP server URL.
pub struct McpOAuthProvider {
    redirect_url: String,
    client_metadata: OAuthClientMetadata,
    client_metadata_document: Option<ClientMetadataDocumentFn>,
    server_url: String,
    configured_client: Option<OAuthClientInformation>,
    store: Arc<dyn McpOAuthStateStore>,
    on_redirect: Arc<dyn Fn(Url) + Send + Sync>,
    writes: Mutex<()>,
}

impl McpOAuthProvider {
    /// Create a provider.
    pub fn new(options: McpOAuthProviderOptions) -> Self {
        let mut client_metadata = options.client_metadata;
        if client_metadata.redirect_uris.is_empty() {
            client_metadata.redirect_uris = vec![options.redirect_url.clone()];
        }
        if client_metadata.grant_types.is_none() {
            client_metadata.grant_types =
                Some(vec!["authorization_code".to_string(), "refresh_token".to_string()]);
        }
        if client_metadata.response_types.is_none() {
            client_metadata.response_types = Some(vec!["code".to_string()]);
        }
        if client_metadata.token_endpoint_auth_method.is_none() {
            client_metadata.token_endpoint_auth_method = Some(
                if options.client_secret.is_some() {
                    "client_secret_post"
                } else {
                    "none"
                }
                .to_string(),
            );
        }
        let configured_client = options.client_id.map(|client_id| OAuthClientInformation {
            client_id,
            client_secret: options.client_secret,
            client_id_issued_at: None,
            client_secret_expires_at: None,
            redirect_uris: Vec::new(),
            token_endpoint_auth_method: None,
            extra: Default::default(),
        });
        Self {
            redirect_url: options.redirect_url,
            client_metadata,
            client_metadata_document: options.client_metadata_document,
            server_url: options.server_url.to_string(),
            configured_client,
            store: options
                .store
                .unwrap_or_else(|| MemoryOAuthStateStore::new()),
            on_redirect: options.on_redirect,
            writes: Mutex::new(()),
        }
    }

    async fn load(&self) -> McpOAuthState {
        let _guard = self.writes.lock().await;
        self.own(self.store.load().await)
    }

    async fn update(&self, update: impl FnOnce(McpOAuthState) -> McpOAuthState) {
        let _guard = self.writes.lock().await;
        let state = update(self.own(self.store.load().await));
        self.store.save(state).await;
    }

    /// Stored state for another server URL is ignored so credentials never
    /// leak across servers.
    fn own(&self, state: Option<McpOAuthState>) -> McpOAuthState {
        match state {
            Some(state) if state.server_url == self.server_url => state,
            _ => McpOAuthState {
                server_url: self.server_url.clone(),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl OAuthClientProvider for McpOAuthProvider {
    fn redirect_url(&self) -> String {
        self.redirect_url.clone()
    }

    fn client_metadata(&self) -> OAuthClientMetadata {
        self.client_metadata.clone()
    }

    fn client_metadata_document(
        &self,
        metadata: Option<&AuthorizationServerMetadata>,
    ) -> Option<OAuthClientMetadataDocument> {
        self.client_metadata_document
            .as_ref()
            .and_then(|selector| selector(metadata))
    }

    async fn state(&self) -> Option<String> {
        let existing = self.load().await.oauth_state;
        if existing.is_some() {
            return existing;
        }
        let state = random_hex();
        self.update(|mut current| {
            current.oauth_state = Some(state.clone());
            current
        })
        .await;
        Some(state)
    }

    async fn client_information(&self) -> Option<OAuthClientInformation> {
        if let Some(client) = &self.configured_client {
            return Some(client.clone());
        }
        self.load().await.client_information
    }

    async fn save_client_information(&self, information: OAuthClientInformation) {
        if self.configured_client.is_some() {
            return;
        }
        self.update(|mut state| {
            state.client_information = Some(information);
            state
        })
        .await;
    }

    async fn tokens(&self) -> Option<OAuthTokens> {
        self.load().await.tokens
    }

    async fn save_tokens(&self, tokens: OAuthTokens) {
        let expires_at = tokens
            .expires_in
            .map(|seconds| now_millis() + (seconds * 1000.0) as i64);
        self.update(|mut state| {
            state.tokens = Some(tokens);
            state.tokens_expire_at = expires_at;
            state
        })
        .await;
    }

    async fn redirect_to_authorization(&self, url: Url) {
        (self.on_redirect)(url);
    }

    async fn save_code_verifier(&self, verifier: String) {
        self.update(|mut state| {
            state.code_verifier = Some(verifier);
            state
        })
        .await;
    }

    async fn code_verifier(&self) -> std::result::Result<String, OAuthError> {
        self.load()
            .await
            .code_verifier
            .ok_or_else(|| OAuthError::Message("No OAuth PKCE code verifier is stored".to_string()))
    }

    async fn invalidate_credentials(&self, kind: InvalidateKind) {
        self.update(|mut state| {
            if matches!(kind, InvalidateKind::All | InvalidateKind::Client) {
                state.client_information = None;
            }
            if matches!(kind, InvalidateKind::All | InvalidateKind::Tokens) {
                state.tokens = None;
                state.tokens_expire_at = None;
            }
            if matches!(kind, InvalidateKind::All | InvalidateKind::Verifier) {
                state.code_verifier = None;
            }
            if matches!(kind, InvalidateKind::All | InvalidateKind::Discovery) {
                state.discovery = None;
            }
            if matches!(kind, InvalidateKind::All) {
                state.oauth_state = None;
            }
            state
        })
        .await;
    }

    async fn save_discovery_state(&self, discovery: OAuthDiscoveryState) {
        self.update(|mut state| {
            state.discovery = Some(discovery);
            state
        })
        .await;
    }

    async fn discovery_state(&self) -> Option<OAuthDiscoveryState> {
        self.load().await.discovery
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn random_hex() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use tokio::sync::Mutex as TokioMutex;

    struct TestStore {
        value: TokioMutex<Option<McpOAuthState>>,
    }

    #[async_trait]
    impl McpOAuthStateStore for TestStore {
        async fn load(&self) -> Option<McpOAuthState> {
            self.value.lock().await.clone()
        }
        async fn save(&self, state: McpOAuthState) {
            *self.value.lock().await = Some(state);
        }
    }

    fn provider(server: &str) -> McpOAuthProvider {
        McpOAuthProvider::new(McpOAuthProviderOptions {
            server_url: Url::parse(server).unwrap(),
            redirect_url: "http://127.0.0.1:1234/callback".to_string(),
            client_metadata: OAuthClientMetadata::default(),
            client_metadata_document: None,
            client_id: None,
            client_secret: None,
            store: Some(Arc::new(TestStore {
                value: TokioMutex::new(None),
            })),
            on_redirect: Arc::new(|_| {}),
        })
    }

    #[tokio::test]
    async fn state_is_scoped_to_one_server_url() {
        let first = provider("https://example.com/mcp");
        let other = provider("https://other.example.com/mcp");
        first
            .save_tokens(OAuthTokens {
                access_token: "a".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: None,
                scope: None,
                refresh_token: None,
                id_token: None,
            })
            .await;
        assert!(first.tokens().await.is_some());
        // The other provider's store is fresh in this test, so it sees nothing.
        assert!(other.tokens().await.is_none());
    }
}
