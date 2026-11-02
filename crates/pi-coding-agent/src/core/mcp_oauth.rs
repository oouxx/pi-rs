//! MCP OAuth credentials and the auth provider used by HTTP transports.
//!
//! Port of `packages/coding-agent/src/extensions/mcp/oauth.ts`. Credentials
//! live in `{agent_dir}/mcp-auth.json`, keyed by server name and URL.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use fs2::FileExt;
use pi_mcp::auth::{AuthProvider, UnauthorizedContext};
use pi_mcp::oauth::{
    adapt_oauth_provider, authorize_mcp, parse_www_authenticate,
    McpOAuthProvider, McpOAuthProviderOptions, McpOAuthState, McpOAuthStateStore,
    OAuthClientMetadata, OAuthClientProvider, OAuthFlowOptions, OAuthFlowResult,
};
pub use pi_mcp::oauth::OAuthError;
use pi_mcp::{McpError, Result as McpResult};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::config::get_agent_dir;
use crate::core::mcp_servers::{mcp_namespace, McpOAuthConfig};
use crate::core::resolve_config_value::resolve_config_value_or_throw;

const REFRESH_SKEW_MS: i64 = 30_000;
const OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// OAuth client settings, with config references already resolved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpOAuthSettings {
    /// Pre-registered client id.
    pub client_id: Option<String>,
    /// Resolved client secret.
    pub client_secret: Option<String>,
    /// Fixed callback port.
    pub callback_port: Option<u16>,
    /// Registered loopback redirect URI.
    pub callback_url: Option<String>,
    /// Requested scopes.
    pub scope: Option<String>,
    /// Client name for dynamic registration.
    pub client_name: Option<String>,
    /// `auto`, `dcr`, or `cimd`.
    pub client_registration: Option<String>,
    /// Explicit authorization-server metadata URL.
    pub auth_server_metadata_url: Option<String>,
}

impl McpOAuthSettings {
    /// Build settings from a validated config entry.
    pub fn from_config(config: Option<&McpOAuthConfig>, server_name: &str) -> Result<Self, String> {
        let Some(config) = config else {
            return Ok(Self::default());
        };
        let client_secret = match &config.client_secret {
            Some(secret) => Some(resolve_config_value_or_throw(
                secret,
                &format!("MCP server \"{server_name}\" oauth.clientSecret"),
            )?),
            None => None,
        };
        Ok(Self {
            client_id: config.client_id.clone(),
            client_secret,
            callback_port: config.callback_port,
            callback_url: config.callback_url.clone(),
            scope: config.scope.clone(),
            client_name: config.client_name.clone(),
            client_registration: config.client_registration.clone(),
            auth_server_metadata_url: config.auth_server_metadata_url.clone(),
        })
    }

    /// Redirect URI used for refreshes and registration.
    pub fn redirect_url(&self) -> String {
        if let Some(url) = &self.callback_url {
            return url.clone();
        }
        match self.callback_port {
            Some(port) => format!("http://127.0.0.1:{port}/callback"),
            None => "http://127.0.0.1/callback".to_string(),
        }
    }
}

struct StoreState {
    path: PathBuf,
    lock_path: PathBuf,
    key: String,
    legacy_key: String,
    process_lock: Mutex<()>,
}

/// Per-server credential store in `mcp-auth.json`.
#[derive(Clone)]
pub struct McpOAuthServerStore {
    state: Arc<StoreState>,
}

impl std::fmt::Debug for McpOAuthServerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthServerStore")
            .field("path", &self.state.path)
            .field("key", &self.state.key)
            .finish()
    }
}

impl McpOAuthServerStore {
    fn read_states(&self) -> Map<String, Value> {
        let Ok(text) = std::fs::read_to_string(&self.state.path) else {
            return Map::new();
        };
        serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    }

    fn write_states(&self, states: &Map<String, Value>) -> Result<(), String> {
        if let Some(parent) = self.state.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let text = format!("{}\n", serde_json::to_string_pretty(states).unwrap_or_default());
        let temp = self.state.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .map_err(|error| format!("{}: {error}", temp.display()))?;
        std::io::Write::write_all(&mut file, text.as_bytes())
            .map_err(|error| format!("{}: {error}", temp.display()))?;
        std::fs::rename(&temp, &self.state.path)
            .map_err(|error| format!("{}: {error}", self.state.path.display()))
    }

    /// Run `work` while no other process refreshes this server's tokens.
    pub async fn with_refresh_lock<T: Send>(
        &self,
        work: impl Future<Output = T> + Send,
    ) -> T {
        let _process = self.state.process_lock.lock().await;
        let lock_path = self.state.lock_path.clone();
        let file = tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(lock_path)
                .ok()?;
            file.lock_exclusive().ok()?;
            Some(file)
        })
        .await
        .ok()
        .flatten();
        let result = work.await;
        if let Some(file) = file {
            let _ = file.unlock();
        }
        result
    }

    /// Stored tokens, read without migrating legacy state.
    pub fn tokens(&self) -> Option<pi_mcp::oauth::OAuthTokens> {
        let states = self.read_states();
        let value = states
            .get(&self.state.key)
            .or_else(|| states.get(&self.state.legacy_key))?;
        serde_json::from_value::<McpOAuthState>(value.clone())
            .ok()
            .and_then(|state| state.tokens)
    }

    /// Remove credentials. Returns whether anything was stored.
    pub fn remove(&self) -> bool {
        let mut states = self.read_states();
        let key = if states.contains_key(&self.state.key) {
            Some(self.state.key.clone())
        } else if states.contains_key(&self.state.legacy_key) {
            Some(self.state.legacy_key.clone())
        } else {
            None
        };
        let Some(key) = key else {
            return false;
        };
        states.remove(&key);
        let _ = self.write_states(&states);
        true
    }
}

#[async_trait]
impl McpOAuthStateStore for McpOAuthServerStore {
    async fn load(&self) -> Option<McpOAuthState> {
        let mut states = self.read_states();
        if let Some(value) = states.get(&self.state.key) {
            return serde_json::from_value(value.clone()).ok();
        }
        let value = states.get(&self.state.legacy_key)?.clone();
        let state: McpOAuthState = serde_json::from_value(value).ok()?;
        // Take over legacy state written by older versions.
        states.insert(self.state.key.clone(), serde_json::to_value(&state).ok()?);
        states.remove(&self.state.legacy_key);
        let _ = self.write_states(&states);
        Some(state)
    }

    async fn save(&self, state: McpOAuthState) {
        let mut states = self.read_states();
        if let Ok(value) = serde_json::to_value(&state) {
            states.insert(self.state.key.clone(), value);
            let _ = self.write_states(&states);
        }
    }
}

/// Store for every server in one agent directory.
#[derive(Clone, Debug)]
pub struct McpOAuthCredentialStore {
    path: PathBuf,
    lock_dir: PathBuf,
}

impl McpOAuthCredentialStore {
    /// Create a store at `{agent_dir}/mcp-auth.json`.
    pub fn new(agent_dir: &Path) -> Self {
        Self {
            path: agent_dir.join("mcp-auth.json"),
            lock_dir: agent_dir.to_path_buf(),
        }
    }

    /// Create a store in the default agent directory.
    pub fn default_agent_dir() -> Self {
        Self::new(&get_agent_dir())
    }

    /// Per-server view.
    pub fn for_server(&self, name: &str, server_url: &str) -> McpOAuthServerStore {
        let legacy_key = Url::parse(server_url)
            .map(|url| url.to_string())
            .unwrap_or_else(|_| server_url.to_string());
        let key = format!("{}|{legacy_key}", mcp_namespace(name));
        let lock_hash = {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(key.as_bytes());
            digest[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        McpOAuthServerStore {
            state: Arc::new(StoreState {
                path: self.path.clone(),
                lock_path: self.lock_dir.join(format!("mcp-auth-refresh-{lock_hash}")),
                key,
                legacy_key,
                process_lock: Mutex::new(()),
            }),
        }
    }
}

impl Default for McpOAuthCredentialStore {
    fn default() -> Self {
        Self::default_agent_dir()
    }
}

/// OAuth auth provider with proactive and 401-triggered refresh.
pub struct McpAuthProvider {
    provider: Arc<McpOAuthProvider>,
    store: McpOAuthServerStore,
    settings: McpOAuthSettings,
    server_url: Url,
    http: reqwest::Client,
    refreshing: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl std::fmt::Debug for McpAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAuthProvider")
            .field("server_url", &self.server_url)
            .finish()
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

impl McpAuthProvider {
    async fn refresh(
        &self,
        stale_token: Option<String>,
        resource_metadata_url: Option<Url>,
        scope: Option<String>,
        skip_refresh: bool,
    ) -> McpResult<()> {
        let (leader, gate) = {
            let mut slot = self.refreshing.lock().await;
            match slot.as_ref() {
                Some(gate) => (false, Arc::clone(gate)),
                None => {
                    let gate = Arc::new(tokio::sync::Notify::new());
                    *slot = Some(Arc::clone(&gate));
                    (true, gate)
                }
            }
        };
        if !leader {
            gate.notified().await;
            return Ok(());
        }
        let result = self
            .store
            .with_refresh_lock(async {
                let state = self.store.load().await;
                if stale_token.is_some()
                    && state.as_ref().and_then(|state| state.tokens.as_ref())
                        .map(|tokens| &tokens.access_token)
                        != stale_token.as_ref()
                {
                    return Ok(());
                }
                let Some(refresh_token) = state
                    .as_ref()
                    .and_then(|state| state.tokens.as_ref())
                    .and_then(|tokens| tokens.refresh_token.clone())
                else {
                    return Err(McpError::OAuth(Box::new(OAuthError::AuthorizationRequired)));
                };
                let options = OAuthFlowOptions {
                    server_url: self.server_url.clone(),
                    authorization_code: None,
                    iss: None,
                    scope,
                    resource_metadata_url,
                    authorization_server_metadata_url: self
                        .settings
                        .auth_server_metadata_url
                        .as_deref()
                        .and_then(|url| Url::parse(url).ok()),
                    http: self.http.clone(),
                    cancel: None,
                    skip_issuer_validation: false,
                    skip_refresh,
                };
                let provider: Arc<dyn pi_mcp::oauth::OAuthClientProvider> = self.provider.clone();
                match authorize_mcp(&provider, &options).await {
                    Ok(OAuthFlowResult::Authorized) => {
                        let _ = refresh_token;
                        Ok(())
                    }
                    Ok(OAuthFlowResult::Redirect) => {
                        Err(McpError::OAuth(Box::new(OAuthError::AuthorizationRequired)))
                    }
                    Err(error) => Err(McpError::OAuth(Box::new(error))),
                }
            })
            .await;
        {
            let mut slot = self.refreshing.lock().await;
            *slot = None;
        }
        gate.notify_waiters();
        result
    }

    /// Wait until no refresh is running.
    pub async fn settled(&self) {
        let gate = self.refreshing.lock().await.clone();
        if let Some(gate) = gate {
            gate.notified().await;
        }
    }
}

#[async_trait]
impl AuthProvider for McpAuthProvider {
    async fn token(&self) -> Option<String> {
        let state = self.store.load().await?;
        let token = state.tokens.as_ref()?.access_token.clone();
        let expired = state
            .tokens_expire_at
            .is_some_and(|expires_at| expires_at - REFRESH_SKEW_MS <= now_millis());
        if expired && state.tokens.as_ref().is_some_and(|tokens| tokens.refresh_token.is_some()) {
            let _ = self.refresh(Some(token.clone()), None, None, false).await;
            return self
                .store
                .load()
                .await
                .and_then(|state| state.tokens.map(|tokens| tokens.access_token));
        }
        Some(token)
    }

    async fn on_unauthorized(&self, context: UnauthorizedContext) -> McpResult<()> {
        let challenge = parse_www_authenticate(context.www_authenticate.as_deref());
        // A refresh keeps the granted scope, so more scope needs a new sign-in.
        if challenge.error.as_deref() == Some("insufficient_scope") {
            return Err(McpError::OAuth(Box::new(OAuthError::AuthorizationRequired)));
        }
        self.refresh(
            context.token.clone(),
            challenge.resource_metadata_url,
            challenge.scope.clone(),
            false,
        )
        .await
    }
}

/// Create an auth provider for a server that uses OAuth.
pub fn create_mcp_auth_provider(
    name: &str,
    server_url: &Url,
    settings: McpOAuthSettings,
    store: McpOAuthServerStore,
) -> Arc<dyn AuthProvider> {
    let _ = name;
    let provider = Arc::new(McpOAuthProvider::new(McpOAuthProviderOptions {
        server_url: server_url.clone(),
        redirect_url: settings.redirect_url(),
        client_metadata: OAuthClientMetadata {
            client_name: settings.client_name.clone(),
            ..Default::default()
        },
        client_metadata_document: None,
        client_id: settings.client_id.clone(),
        client_secret: settings.client_secret.clone(),
        store: Some(Arc::new(store.clone())),
        on_redirect: Arc::new(|_| {}),
    }));
    let http = reqwest::Client::builder()
        .timeout(OAUTH_REQUEST_TIMEOUT)
        .build()
        .unwrap_or_default();
    Arc::new(McpAuthProvider {
        provider,
        store,
        settings,
        server_url: server_url.clone(),
        http,
        refreshing: Mutex::new(None),
    })
}

/// Whether a server uses OAuth: HTTP without an Authorization header and
/// without `auth.provider`.
pub fn server_uses_oauth(
    config: &crate::core::mcp_servers::McpServerConfig,
) -> bool {
    if config.url.is_none() || config.auth.is_some() {
        return false;
    }
    !config.headers.as_ref().is_some_and(|headers| {
        headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("authorization"))
    })
}

/// Translate an OAuth error into a user-facing sign-in hint.
pub fn sign_in_hint(name: &str) -> String {
    format!(
        "MCP server \"{name}\" requires sign-in. Run /mcp or `pi-rs mcp login {name}` to sign in."
    )
}

/// Adapter used by tests and external callers.
pub fn adapt_provider(provider: Arc<McpOAuthProvider>) -> Arc<dyn AuthProvider> {
    adapt_oauth_provider(provider)
}

/// Prompt used by the browser sign-in flow.
#[derive(Clone)]
pub struct McpSignInPrompt {
    /// Show the authorization URL and open it in a browser.
    pub show_authorization_url: Arc<dyn Fn(Url) + Send + Sync>,
    /// Ask for the redirect URL from the browser address bar.
    pub prompt_for_redirect_url:
        Arc<dyn Fn(CancellationToken) -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync>,
}

impl std::fmt::Debug for McpSignInPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpSignInPrompt")
    }
}

struct CallbackSettings {
    host: String,
    redirect_host: String,
    port: Option<u16>,
    path: String,
}

fn callback_settings(settings: &McpOAuthSettings) -> Result<CallbackSettings, OAuthError> {
    let raw = settings
        .callback_url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1/callback".to_string());
    let url = Url::parse(&raw)?;
    let host = url.host_str().unwrap_or("127.0.0.1").to_string();
    let listen_host = if host == "localhost" {
        "127.0.0.1".to_string()
    } else {
        host.clone()
    };
    Ok(CallbackSettings {
        host: listen_host,
        redirect_host: host,
        port: url.port().or(settings.callback_port),
        path: url.path().to_string(),
    })
}

fn response_from_redirect_url(
    input: &str,
    state: &str,
    redirect_url: &Url,
) -> Result<(String, Option<String>), OAuthError> {
    let url = Url::parse(input.trim())
        .map_err(|_| OAuthError::Message("Expected the full redirect URL from the browser address bar".to_string()))?;
    if url.origin() != redirect_url.origin() || url.path() != redirect_url.path() {
        return Err(OAuthError::Message(
            "The redirect URL does not match this sign-in's redirect URI".to_string(),
        ));
    }
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    if let Some(error) = params.get("error") {
        return Err(OAuthError::Message(
            params
                .get("error_description")
                .cloned()
                .unwrap_or_else(|| error.clone()),
        ));
    }
    if params.get("state").map(String::as_str) != Some(state) {
        return Err(OAuthError::Message(
            "The redirect URL belongs to a different sign-in".to_string(),
        ));
    }
    let code = params
        .get("code")
        .cloned()
        .ok_or_else(|| OAuthError::Message("The redirect URL does not contain an authorization code".to_string()))?;
    Ok((code, params.get("iss").cloned()))
}

/// Sign in to an MCP server through a loopback OAuth callback.
///
/// Uses the stored refresh token when possible; otherwise runs the browser
/// authorization-code flow. Aborting `cancel` stops the sign-in.
pub async fn sign_in_mcp_server(
    server_url: &Url,
    store: McpOAuthServerStore,
    settings: &McpOAuthSettings,
    challenge: Option<&pi_mcp::oauth::OAuthChallenge>,
    prompt: McpSignInPrompt,
    cancel: Option<CancellationToken>,
) -> Result<(), OAuthError> {
    if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
        return Err(OAuthError::SignInCancelled);
    }
    let callbacks = callback_settings(settings)?;
    let callback = tokio::time::timeout(
        Duration::from_secs(10),
        pi_mcp::oauth::callback::OAuthCallbackServer::listen(
            pi_mcp::oauth::callback::OAuthCallbackServerOptions {
                host: Some(callbacks.host.clone()),
                redirect_host: Some(callbacks.redirect_host.clone()),
                port: callbacks.port,
                path: Some(callbacks.path.clone()),
                ..Default::default()
            },
        ),
    )
    .await
    .map_err(|_| OAuthError::Message("OAuth callback server timed out".to_string()))??;
    let redirect_url = callback.redirect_url.clone();
    let redirect = Url::parse(&redirect_url)?;
    let authorization_url: Arc<Mutex<Option<Url>>> = Arc::new(Mutex::new(None));
    // `McpOAuthProvider` takes a synchronous redirect callback; capture the URL
    // through the provider's `on_redirect`.
    let provider = Arc::new(McpOAuthProvider::new(McpOAuthProviderOptions {
        server_url: server_url.clone(),
        redirect_url: redirect_url.clone(),
        client_metadata: OAuthClientMetadata {
            client_name: settings.client_name.clone(),
            ..Default::default()
        },
        client_metadata_document: None,
        client_id: settings.client_id.clone(),
        client_secret: settings.client_secret.clone(),
        store: Some(Arc::new(store.clone())),
        on_redirect: {
            let authorization_url = Arc::clone(&authorization_url);
            Arc::new(move |url| {
                if let Ok(mut slot) = authorization_url.try_lock() {
                    *slot = Some(url);
                }
            })
        },
    }));
    let step_up = challenge.and_then(|challenge| challenge.error.as_deref()) == Some("insufficient_scope");
    let flow = OAuthFlowOptions {
        server_url: server_url.clone(),
        authorization_code: None,
        iss: None,
        scope: settings.scope.clone(),
        resource_metadata_url: challenge.and_then(|challenge| challenge.resource_metadata_url.clone()),
        authorization_server_metadata_url: settings
            .auth_server_metadata_url
            .as_deref()
            .and_then(|url| Url::parse(url).ok()),
        http: reqwest::Client::builder()
            .timeout(OAUTH_REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default(),
        cancel: cancel.clone(),
        skip_issuer_validation: false,
        skip_refresh: step_up,
    };
    let result = async {
        let provider_trait: Arc<dyn pi_mcp::oauth::OAuthClientProvider> = provider.clone();
        match authorize_mcp(&provider_trait, &flow).await? {
            OAuthFlowResult::Authorized => return Ok(()),
            OAuthFlowResult::Redirect => {}
        }
        let authorization_url = authorization_url
            .lock()
            .await
            .clone()
            .ok_or_else(|| OAuthError::Message("OAuth flow did not produce an authorization URL".to_string()))?;
        let state = provider
            .state()
            .await
            .ok_or_else(|| OAuthError::Message("OAuth provider did not produce a state".to_string()))?;
        let authorization_redirect = authorization_url
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .map(|(_, value)| value.to_string())
            .and_then(|value| Url::parse(&value).ok())
            .unwrap_or_else(|| redirect.clone());
        (prompt.show_authorization_url)(authorization_url);
        let path = authorization_redirect.path().to_string();
        let browser = callback.wait_for_callback(&state, Some(&path));
        let pasted = (prompt.prompt_for_redirect_url)(
            cancel.clone().unwrap_or_default(),
        );
        let (code, iss) = tokio::select! {
            result = browser => {
                let callback = result.map_err(|error| OAuthError::Message(error.to_string()))?;
                (callback.code, callback.iss)
            }
            value = pasted => {
                let Some(value) = value else {
                    return Err(OAuthError::SignInCancelled);
                };
                response_from_redirect_url(&value, &state, &redirect)?
            }
        };
        let provider_trait: Arc<dyn pi_mcp::oauth::OAuthClientProvider> = provider.clone();
        let exchange = OAuthFlowOptions {
            authorization_code: Some(code),
            iss,
            ..flow
        };
        match authorize_mcp(&provider_trait, &exchange).await? {
            OAuthFlowResult::Authorized => Ok(()),
            OAuthFlowResult::Redirect => Err(OAuthError::Message(
                "OAuth flow redirected again during code exchange".to_string(),
            )),
        }
    }
    .await;
    callback.close().await;
    result
}

/// Make a cancellable OAuth flow future.
pub async fn run_flow_with_cancel<T>(
    cancel: Option<CancellationToken>,
    future: impl Future<Output = T>,
) -> Option<T> {
    match cancel {
        Some(token) => tokio::select! {
            () = token.cancelled() => None,
            value = future => Some(value),
        },
        None => Some(future.await),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use pi_mcp::oauth::OAuthTokens;

    #[test]
    fn store_keys_and_legacy_migration() {
        let dir = tempfile::tempdir().unwrap();
        let store = McpOAuthCredentialStore::new(dir.path());
        let server = store.for_server("docs", "https://example.com/mcp");
        let legacy_key = "https://example.com/mcp".to_string();
        let state = McpOAuthState {
            server_url: "https://example.com/mcp".to_string(),
            tokens: Some(OAuthTokens {
                access_token: "token".to_string(),
                token_type: "Bearer".to_string(),
                expires_in: None,
                scope: None,
                refresh_token: None,
                id_token: None,
            }),
            ..Default::default()
        };
        std::fs::write(
            dir.path().join("mcp-auth.json"),
            serde_json::to_string(&serde_json::json!({legacy_key: state})).unwrap(),
        )
        .unwrap();
        // The first load migrates the legacy key.
        let loaded = futures::executor::block_on(server.load()).unwrap();
        assert_eq!(loaded.tokens.unwrap().access_token, "token");
        let text = std::fs::read_to_string(dir.path().join("mcp-auth.json")).unwrap();
        assert!(text.contains("mcp__docs|https://example.com/mcp"));
    }

    #[test]
    fn server_uses_oauth_rules() {
        let mut config = crate::core::mcp_servers::McpServerConfig {
            url: Some("https://example.com/mcp".to_string()),
            ..Default::default()
        };
        assert!(server_uses_oauth(&config));
        config.headers = Some(indexmap::IndexMap::from([(
            "Authorization".to_string(),
            "Bearer x".to_string(),
        )]));
        assert!(!server_uses_oauth(&config));
    }
}
