//! OAuth authorization-code flow with PKCE, DCR, and refresh.
//!
//! Port of `packages/mcp/src/oauth/flow.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::auth::{AuthProvider, UnauthorizedContext};
use crate::error::{McpError, Result};
use crate::oauth::discovery::{
    discover_authorization_server_metadata, discover_oauth_server_info, parse_www_authenticate,
    select_resource, DiscoveryOptions,
};
use crate::oauth::errors::OAuthError;
use crate::oauth::types::{
    parse_oauth_tokens, AuthorizationServerMetadata, OAuthClientInformation, OAuthClientMetadata,
    OAuthDiscoveryState, OAuthTokens,
};

const BASE64_URL: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;
const BASE64_STD: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// A Client ID Metadata Document: an https URL used as `client_id`, plus the
/// redirect URI it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientMetadataDocument {
    /// Document URL.
    pub url: String,
    /// Redirect URI from the document.
    pub redirect_url: String,
}

/// What to invalidate after an OAuth failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidateKind {
    /// Everything.
    All,
    /// Client registration.
    Client,
    /// Tokens.
    Tokens,
    /// PKCE verifier.
    Verifier,
    /// Cached discovery state.
    Discovery,
}

/// Provider state required by the OAuth flow.
#[async_trait]
pub trait OAuthClientProvider: Send + Sync {
    /// Redirect URI.
    fn redirect_url(&self) -> String;
    /// Client metadata for dynamic registration.
    fn client_metadata(&self) -> OAuthClientMetadata;
    /// Client metadata document, when the server supports CIMD.
    fn client_metadata_document(
        &self,
        _metadata: Option<&AuthorizationServerMetadata>,
    ) -> Option<OAuthClientMetadataDocument> {
        None
    }
    /// `state` parameter for authorization requests.
    async fn state(&self) -> Option<String> {
        None
    }
    /// Stored client information.
    async fn client_information(&self) -> Option<OAuthClientInformation>;
    /// Store client information.
    async fn save_client_information(&self, _information: OAuthClientInformation) {}
    /// Stored tokens.
    async fn tokens(&self) -> Option<OAuthTokens>;
    /// Store tokens.
    async fn save_tokens(&self, tokens: OAuthTokens);
    /// Called with the authorization URL.
    async fn redirect_to_authorization(&self, url: Url);
    /// Store the PKCE verifier.
    async fn save_code_verifier(&self, verifier: String);
    /// Read the PKCE verifier.
    async fn code_verifier(&self) -> std::result::Result<String, OAuthError>;
    /// Invalidate stored state.
    async fn invalidate_credentials(&self, _kind: InvalidateKind) {}
    /// Store discovery state.
    async fn save_discovery_state(&self, _state: OAuthDiscoveryState) {}
    /// Read discovery state.
    async fn discovery_state(&self) -> Option<OAuthDiscoveryState> {
        None
    }
    /// Custom client authentication. Returns true when it handled the request.
    async fn add_client_authentication(
        &self,
        _headers: &mut HeaderMap,
        _params: &mut Vec<(String, String)>,
        _url: &Url,
        _metadata: Option<&AuthorizationServerMetadata>,
    ) -> std::result::Result<bool, OAuthError> {
        Ok(false)
    }
}

/// Options for one OAuth flow.
#[derive(Clone)]
pub struct OAuthFlowOptions {
    /// MCP server URL.
    pub server_url: Url,
    /// Authorization code to exchange.
    pub authorization_code: Option<String>,
    /// `iss` from the authorization response (RFC 9207).
    pub iss: Option<String>,
    /// Requested scope.
    pub scope: Option<String>,
    /// `resource_metadata` URL from a challenge.
    pub resource_metadata_url: Option<Url>,
    /// Explicit authorization server metadata URL.
    pub authorization_server_metadata_url: Option<Url>,
    /// HTTP client.
    pub http: Client,
    /// Cancellation token.
    pub cancel: Option<CancellationToken>,
    /// Skip issuer validation.
    pub skip_issuer_validation: bool,
    /// Skip refresh and go straight to authorization.
    pub skip_refresh: bool,
}

impl OAuthFlowOptions {
    /// Options for a server URL with a default HTTP client.
    pub fn new(server_url: Url, http: Client) -> Self {
        Self {
            server_url,
            authorization_code: None,
            iss: None,
            scope: None,
            resource_metadata_url: None,
            authorization_server_metadata_url: None,
            http,
            cancel: None,
            skip_issuer_validation: false,
            skip_refresh: false,
        }
    }
}

/// Result of [`authorize_mcp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthFlowResult {
    /// Tokens are stored.
    Authorized,
    /// The user must visit the authorization URL.
    Redirect,
}

/// Options for a token request.
#[derive(Clone)]
pub struct TokenRequestOptions {
    /// Authorization server metadata.
    pub metadata: Option<AuthorizationServerMetadata>,
    /// Client information.
    pub client_information: OAuthClientInformation,
    /// Resource indicator.
    pub resource: Option<String>,
    /// Provider for custom client authentication.
    pub provider: Option<Arc<dyn OAuthClientProvider>>,
    /// HTTP client.
    pub http: Client,
    /// Cancellation token.
    pub cancel: Option<CancellationToken>,
}

fn loopback(hostname: &str) -> bool {
    matches!(hostname, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn application_type(redirect_uris: &[String]) -> &'static str {
    let native = redirect_uris.iter().any(|uri| {
        Url::parse(uri).is_ok_and(|url| {
            (url.scheme() != "http" && url.scheme() != "https")
                || url.host_str().is_some_and(loopback)
        })
    });
    if native {
        "native"
    } else {
        "web"
    }
}

fn secure_endpoint(value: &Url) -> std::result::Result<Url, OAuthError> {
    if value.scheme() != "https" && !value.host_str().is_some_and(loopback) {
        return Err(OAuthError::InsecureEndpoint {
            endpoint: value.to_string(),
        });
    }
    Ok(value.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientAuthMethod {
    Basic,
    Post,
    None,
}

fn select_client_auth_method(
    information: &OAuthClientInformation,
    supported: &[String],
) -> ClientAuthMethod {
    if let Some(hinted) = information
        .token_endpoint_auth_method
        .as_deref()
        .filter(|hint| matches!(*hint, "client_secret_basic" | "client_secret_post" | "none"))
    {
        if supported.is_empty() || supported.iter().any(|method| method == hinted) {
            return match hinted {
                "client_secret_basic" => ClientAuthMethod::Basic,
                "client_secret_post" => ClientAuthMethod::Post,
                _ => ClientAuthMethod::None,
            };
        }
    }
    if supported.is_empty() {
        return if information.client_secret.is_some() {
            ClientAuthMethod::Basic
        } else {
            ClientAuthMethod::None
        };
    }
    if information.client_secret.is_some()
        && supported.iter().any(|method| method == "client_secret_basic")
    {
        return ClientAuthMethod::Basic;
    }
    if information.client_secret.is_some()
        && supported.iter().any(|method| method == "client_secret_post")
    {
        return ClientAuthMethod::Post;
    }
    if supported.iter().any(|method| method == "none") {
        return ClientAuthMethod::None;
    }
    if information.client_secret.is_some() {
        ClientAuthMethod::Post
    } else {
        ClientAuthMethod::None
    }
}

fn apply_client_authentication(
    method: ClientAuthMethod,
    information: &OAuthClientInformation,
    headers: &mut HeaderMap,
    params: &mut Vec<(String, String)>,
) -> std::result::Result<(), OAuthError> {
    match method {
        ClientAuthMethod::Basic => {
            let Some(secret) = &information.client_secret else {
                return Err(OAuthError::Message(
                    "client_secret_basic requires a client secret".to_string(),
                ));
            };
            let raw = format!("{}:{secret}", information.client_id);
            let encoded = BASE64_STD.encode(raw.as_bytes());
            let value = HeaderValue::from_str(&format!("Basic {encoded}"))
                .map_err(|error| OAuthError::Message(error.to_string()))?;
            headers.insert(AUTHORIZATION, value);
        }
        ClientAuthMethod::Post | ClientAuthMethod::None => {
            params.push(("client_id".to_string(), information.client_id.clone()));
            if method == ClientAuthMethod::Post {
                if let Some(secret) = &information.client_secret {
                    params.push(("client_secret".to_string(), secret.clone()));
                }
            }
        }
    }
    Ok(())
}

fn pkce() -> (String, String) {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let verifier = BASE64_URL.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = BASE64_URL.encode(digest);
    (verifier, challenge)
}

/// Build the authorization URL and PKCE verifier.
pub fn start_authorization(
    authorization_server_url: &Url,
    metadata: Option<&AuthorizationServerMetadata>,
    client_information: &OAuthClientInformation,
    redirect_url: &str,
    scope: Option<&str>,
    state: Option<&str>,
    resource: Option<&str>,
) -> std::result::Result<(Url, String), OAuthError> {
    if let Some(metadata) = metadata {
        if !metadata.response_types_supported.iter().any(|value| value == "code") {
            return Err(OAuthError::Message(
                "Authorization server does not support authorization codes".to_string(),
            ));
        }
        if let Some(methods) = &metadata.code_challenge_methods_supported {
            if !methods.iter().any(|value| value == "S256") {
                return Err(OAuthError::Message(
                    "Authorization server does not support PKCE S256".to_string(),
                ));
            }
        }
    }
    let endpoint = metadata
        .map(|metadata| metadata.authorization_endpoint.clone())
        .unwrap_or_else(|| {
            let mut url = authorization_server_url.clone();
            url.set_path("/authorize");
            url.to_string()
        });
    let mut url = Url::parse(&endpoint)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(OAuthError::Message(format!(
            "Invalid authorization endpoint {url}"
        )));
    }
    let (verifier, challenge) = pkce();
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", &client_information.client_id);
        query.append_pair("code_challenge", &challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("redirect_uri", redirect_url);
        if let Some(state) = state {
            query.append_pair("state", state);
        }
        if let Some(scope) = scope {
            query.append_pair("scope", scope);
        }
        if scope.is_some_and(|scope| scope.split_whitespace().any(|value| value == "offline_access")) {
            query.append_pair("prompt", "consent");
        }
        if let Some(resource) = resource {
            query.append_pair("resource", resource);
        }
    }
    Ok((url, verifier))
}

async fn token_request(
    authorization_server_url: &Url,
    options: &TokenRequestOptions,
    mut params: Vec<(String, String)>,
) -> std::result::Result<OAuthTokens, OAuthError> {
    let endpoint = options
        .metadata
        .as_ref()
        .map(|metadata| metadata.token_endpoint.clone())
        .unwrap_or_else(|| {
            let mut url = authorization_server_url.clone();
            url.set_path("/token");
            url.to_string()
        });
    let url = secure_endpoint(&Url::parse(&endpoint)?)?;
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    if let Some(resource) = &options.resource {
        params.push(("resource".to_string(), resource.clone()));
    }
    let mut handled = false;
    if let Some(provider) = &options.provider {
        handled = provider
            .add_client_authentication(
                &mut headers,
                &mut params,
                &url,
                options.metadata.as_ref(),
            )
            .await?;
    }
    if !handled {
        let supported = options
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.token_endpoint_auth_methods_supported.clone())
            .unwrap_or_default();
        apply_client_authentication(
            select_client_auth_method(&options.client_information, &supported),
            &options.client_information,
            &mut headers,
            &mut params,
        )?;
    }
    let request = options
        .http
        .post(url)
        .headers(headers)
        .form(&params);
    let response = match &options.cancel {
        Some(token) => tokio::select! {
            () = token.cancelled() => return Err(OAuthError::Network("aborted".to_string())),
            response = request.send() => response?,
        },
        None => request.send().await?,
    };
    let status = response.status();
    let text = response.text().await?;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if let Some(object) = value.as_object() {
        if let Some(error) = object.get("error").and_then(serde_json::Value::as_str) {
            return Err(OAuthError::oauth(
                error,
                object
                    .get("error_description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(error),
                object
                    .get("error_uri")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            ));
        }
    }
    if !status.is_success() {
        return Err(OAuthError::oauth(
            "server_error",
            format!("HTTP {}: {text}", status.as_u16()),
            None,
        ));
    }
    parse_oauth_tokens(&value)
}

/// Register a client dynamically.
pub async fn register_client(
    authorization_server_url: &Url,
    metadata: Option<&AuthorizationServerMetadata>,
    client_metadata: &OAuthClientMetadata,
    scope: Option<&str>,
    http: &Client,
    cancel: Option<&CancellationToken>,
) -> std::result::Result<OAuthClientInformation, OAuthError> {
    let endpoint = match (metadata, metadata.and_then(|metadata| metadata.registration_endpoint.as_ref())) {
        (Some(_), None) => {
            return Err(OAuthError::Message(
                "Authorization server does not support dynamic client registration".to_string(),
            ))
        }
        (_, Some(endpoint)) => endpoint.clone(),
        (None, None) => {
            let mut url = authorization_server_url.clone();
            url.set_path("/register");
            url.to_string()
        }
    };
    let mut body = serde_json::to_value(client_metadata)?;
    if let Some(object) = body.as_object_mut() {
        if !object.contains_key("application_type") {
            object.insert(
                "application_type".to_string(),
                serde_json::Value::String(application_type(&client_metadata.redirect_uris).to_string()),
            );
        }
        if let Some(scope) = scope {
            object.insert(
                "scope".to_string(),
                serde_json::Value::String(scope.to_string()),
            );
        }
    }
    let request = http
        .post(Url::parse(&endpoint)?)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(&body)?);
    let response = match cancel {
        Some(token) => tokio::select! {
            () = token.cancelled() => return Err(OAuthError::Network("aborted".to_string())),
            response = request.send() => response?,
        },
        None => request.send().await?,
    };
    if !response.status().is_success() {
        return Err(OAuthError::Registration {
            status: response.status().as_u16(),
            body: response.text().await.unwrap_or_default(),
        });
    }
    let value: serde_json::Value = response.json().await?;
    crate::oauth::types::parse_client_information(&value)
}

/// Exchange an authorization code for tokens.
pub async fn exchange_authorization_code(
    authorization_server_url: &Url,
    options: &TokenRequestOptions,
    code: &str,
    code_verifier: &str,
    redirect_url: &str,
) -> std::result::Result<OAuthTokens, OAuthError> {
    token_request(
        authorization_server_url,
        options,
        vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), code.to_string()),
            ("code_verifier".to_string(), code_verifier.to_string()),
            ("redirect_uri".to_string(), redirect_url.to_string()),
        ],
    )
    .await
}

/// Refresh tokens.
pub async fn refresh_authorization(
    authorization_server_url: &Url,
    options: &TokenRequestOptions,
    refresh_token: &str,
) -> std::result::Result<OAuthTokens, OAuthError> {
    let mut tokens = token_request(
        authorization_server_url,
        options,
        vec![
            ("grant_type".to_string(), "refresh_token".to_string()),
            ("refresh_token".to_string(), refresh_token.to_string()),
        ],
    )
    .await?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    Ok(tokens)
}

fn with_scope(mut tokens: OAuthTokens, scope: Option<&str>) -> OAuthTokens {
    if tokens.scope.is_none() {
        if let Some(scope) = scope {
            tokens.scope = Some(scope.to_string());
        }
    }
    tokens
}

/// Scopes for a step-up authorization: the challenged scopes plus the granted
/// ones, so a token with only the missing scopes does not lose access.
pub fn step_up_scope(granted: Option<&str>, challenged: Option<&str>) -> Option<String> {
    let challenged = challenged?;
    let mut scopes: Vec<&str> = Vec::new();
    for scope in [granted, Some(challenged)].into_iter().flatten() {
        for value in scope.split_whitespace().filter(|value| !value.is_empty()) {
            if !scopes.contains(&value) {
                scopes.push(value);
            }
        }
    }
    Some(scopes.join(" "))
}

async fn run_flow(
    provider: &Arc<dyn OAuthClientProvider>,
    options: &OAuthFlowOptions,
) -> std::result::Result<OAuthFlowResult, OAuthError> {
    let metadata_url = match &options.authorization_server_metadata_url {
        Some(url) => Some(secure_endpoint(url)?),
        None => None,
    };
    let cached = if metadata_url.is_none() {
        provider.discovery_state().await
    } else {
        None
    };
    let discovery_options = DiscoveryOptions {
        resource_metadata_url: options.resource_metadata_url.clone(),
        authorization_server_metadata_url: metadata_url.clone(),
        protocol_version: None,
        skip_issuer_validation: options.skip_issuer_validation,
        cancel: options.cancel.clone(),
    };
    let discovered = if let Some(cached) = cached.filter(|cached| !cached.authorization_server_url.is_empty()) {
        let metadata = match cached.authorization_server_metadata {
            Some(metadata) => Some(metadata),
            None => {
                discover_authorization_server_metadata(
                    &Url::parse(&cached.authorization_server_url)?,
                    &options.http,
                    &discovery_options,
                )
                .await?
            }
        };
        crate::oauth::types::OAuthServerInfo {
            authorization_server_url: cached.authorization_server_url,
            authorization_server_metadata: metadata,
            resource_metadata: cached.resource_metadata,
        }
    } else {
        discover_oauth_server_info(&options.server_url, &options.http, &discovery_options).await?
    };
    if metadata_url.is_none() {
        provider
            .save_discovery_state(OAuthDiscoveryState {
                authorization_server_url: discovered.authorization_server_url.clone(),
                authorization_server_metadata: discovered.authorization_server_metadata.clone(),
                resource_metadata: discovered.resource_metadata.clone(),
                resource_metadata_url: options
                    .resource_metadata_url
                    .as_ref()
                    .map(ToString::to_string),
            })
            .await;
    }
    let metadata = discovered.authorization_server_metadata;
    let authorization_server_url = Url::parse(&discovered.authorization_server_url)?;
    let resource = select_resource(&options.server_url, discovered.resource_metadata.as_ref())?;
    let client_metadata = provider.client_metadata();
    let scope = options
        .scope
        .clone()
        .filter(|scope| !scope.is_empty())
        .or_else(|| {
            discovered
                .resource_metadata
                .as_ref()
                .and_then(|metadata| metadata.scopes_supported.as_ref())
                .filter(|scopes| !scopes.is_empty())
                .map(|scopes| scopes.join(" "))
        })
        .or_else(|| client_metadata.scope.clone());
    let stored = provider.client_information().await;
    let client_document = if stored.is_none() {
        provider.client_metadata_document(metadata.as_ref())
    } else {
        None
    };
    if let Some(document) = &client_document {
        let url = Url::parse(&document.url)?;
        if url.scheme() != "https" || url.path() == "/" {
            return Err(OAuthError::Message(
                "Invalid OAuth client metadata URL".to_string(),
            ));
        }
    }
    let mut client = stored.or_else(|| {
        client_document.as_ref().map(|document| OAuthClientInformation {
            client_id: document.url.clone(),
            client_secret: None,
            client_id_issued_at: None,
            client_secret_expires_at: None,
            redirect_uris: vec![document.redirect_url.clone()],
            token_endpoint_auth_method: None,
            extra: Default::default(),
        })
    });
    if client.is_none() {
        if options.authorization_code.is_some() {
            return Err(OAuthError::Message(
                "OAuth client information is missing during code exchange".to_string(),
            ));
        }
        let registered = register_client(
            &authorization_server_url,
            metadata.as_ref(),
            &client_metadata,
            scope.as_deref(),
            &options.http,
            options.cancel.as_ref(),
        )
        .await?;
        provider.save_client_information(registered.clone()).await;
        client = Some(registered);
    }
    let client = match client {
        Some(client) => client,
        None => return Err(OAuthError::Message("Missing OAuth client".to_string())),
    };
    let redirect_url = client_document
        .as_ref()
        .map(|document| document.redirect_url.clone())
        .unwrap_or_else(|| provider.redirect_url());
    let token_options = TokenRequestOptions {
        metadata: metadata.clone(),
        client_information: client.clone(),
        resource: resource.clone(),
        provider: Some(Arc::clone(provider)),
        http: options.http.clone(),
        cancel: options.cancel.clone(),
    };

    if let Some(code) = &options.authorization_code {
        let iss = options.iss.as_deref();
        if (iss.is_some()
            || metadata
                .as_ref()
                .is_some_and(|metadata| metadata.authorization_response_iss_parameter_supported == Some(true)))
            && iss != metadata.as_ref().map(|metadata| metadata.issuer.as_str())
        {
            return Err(OAuthError::IssuerMismatch {
                expected: metadata.as_ref().map(|metadata| metadata.issuer.clone()),
                received: iss.map(str::to_string),
            });
        }
        let verifier = provider.code_verifier().await?;
        let tokens = exchange_authorization_code(
            &authorization_server_url,
            &token_options,
            code,
            &verifier,
            &redirect_url,
        )
        .await?;
        provider.save_tokens(with_scope(tokens, scope.as_deref())).await;
        return Ok(OAuthFlowResult::Authorized);
    }

    let existing = if options.skip_refresh {
        None
    } else {
        provider.tokens().await
    };
    if let Some(existing) = existing.filter(|tokens| tokens.refresh_token.is_some()) {
        let refresh_token = existing.refresh_token.clone().unwrap_or_default();
        match refresh_authorization(&authorization_server_url, &token_options, &refresh_token).await {
            Ok(tokens) => {
                provider
                    .save_tokens(with_scope(tokens, existing.scope.as_deref()))
                    .await;
                return Ok(OAuthFlowResult::Authorized);
            }
            Err(error) => {
                let abort = options
                    .cancel
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled);
                if abort || matches!(error, OAuthError::InsecureEndpoint { .. }) {
                    return Err(error);
                }
                if let OAuthError::OAuth { code, .. } = &error {
                    if code != "server_error" {
                        return Err(error);
                    }
                }
            }
        }
    }

    let state = provider.state().await;
    let (authorization_url, verifier) = start_authorization(
        &authorization_server_url,
        metadata.as_ref(),
        &client,
        &redirect_url,
        scope.as_deref(),
        state.as_deref(),
        resource.as_deref(),
    )?;
    provider.save_code_verifier(verifier).await;
    provider.redirect_to_authorization(authorization_url).await;
    Ok(OAuthFlowResult::Redirect)
}

/// Run the OAuth flow, handling invalid client/token state by retrying once.
pub async fn authorize_mcp(
    provider: &Arc<dyn OAuthClientProvider>,
    options: &OAuthFlowOptions,
) -> std::result::Result<OAuthFlowResult, OAuthError> {
    match run_flow(provider, options).await {
        Ok(result) => Ok(result),
        Err(error) => {
            if error.is_invalid_client() || error.is_unauthorized_client() {
                provider.invalidate_credentials(InvalidateKind::All).await;
                return run_flow(provider, options).await;
            }
            if error.is_invalid_grant() {
                provider.invalidate_credentials(InvalidateKind::Tokens).await;
                return run_flow(provider, options).await;
            }
            Err(error)
        }
    }
}

struct OAuthAdapter {
    provider: Arc<dyn OAuthClientProvider>,
    in_flight: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

#[async_trait]
impl AuthProvider for OAuthAdapter {
    async fn token(&self) -> Option<String> {
        self.provider.tokens().await.map(|tokens| tokens.access_token)
    }

    async fn on_unauthorized(&self, context: UnauthorizedContext) -> Result<()> {
        let challenge = parse_www_authenticate(context.www_authenticate.as_deref());
        let insufficient_scope = challenge.error.as_deref() == Some("insufficient_scope");
        if !insufficient_scope && context.token.is_some() {
            let current = self.provider.tokens().await.map(|tokens| tokens.access_token);
            if current.is_some() && current != context.token {
                return Ok(());
            }
        }

        let (leader, gate) = {
            let mut slot = self.in_flight.lock().await;
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

        let result = async {
            let granted = if insufficient_scope {
                self.provider.tokens().await.and_then(|tokens| tokens.scope)
            } else {
                None
            };
            let scope = if insufficient_scope {
                step_up_scope(granted.as_deref(), challenge.scope.as_deref())
            } else {
                challenge.scope.clone()
            };
            let options = OAuthFlowOptions {
                server_url: context.server_url.clone(),
                authorization_code: None,
                iss: None,
                scope,
                resource_metadata_url: challenge.resource_metadata_url.clone(),
                authorization_server_metadata_url: None,
                http: context.http.clone(),
                cancel: None,
                skip_issuer_validation: false,
                skip_refresh: insufficient_scope,
            };
            match authorize_mcp(&self.provider, &options).await {
                Ok(OAuthFlowResult::Redirect) => Err(McpError::OAuth(Box::new(
                    OAuthError::AuthorizationRequired,
                ))),
                Ok(OAuthFlowResult::Authorized) => Ok(()),
                Err(error) => Err(McpError::OAuth(Box::new(error))),
            }
        }
        .await;

        {
            let mut slot = self.in_flight.lock().await;
            *slot = None;
        }
        gate.notify_waiters();
        result
    }
}

/// Wrap an OAuth client provider as an [`AuthProvider`] for HTTP transports.
pub fn adapt_oauth_provider(provider: Arc<dyn OAuthClientProvider>) -> Arc<dyn AuthProvider> {
    Arc::new(OAuthAdapter {
        provider,
        in_flight: Mutex::new(None),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn step_up_scope_keeps_granted_scopes() {
        assert_eq!(
            step_up_scope(Some("read write"), Some("write admin")),
            Some("read write admin".to_string())
        );
        assert_eq!(step_up_scope(Some("read"), None), None);
    }

    #[test]
    fn application_type_derives_native_for_loopback() {
        assert_eq!(
            application_type(&["http://127.0.0.1:1234/callback".to_string()]),
            "native"
        );
        assert_eq!(
            application_type(&["https://example.com/callback".to_string()]),
            "web"
        );
    }

    #[test]
    fn insecure_endpoints_are_rejected_except_loopback() {
        assert!(secure_endpoint(&Url::parse("https://example.com/token").unwrap()).is_ok());
        assert!(secure_endpoint(&Url::parse("http://127.0.0.1:1234/token").unwrap()).is_ok());
        assert!(secure_endpoint(&Url::parse("http://example.com/token").unwrap()).is_err());
    }

    #[test]
    fn client_auth_method_prefers_secret_basic() {
        let information = OAuthClientInformation {
            client_id: "id".to_string(),
            client_secret: Some("secret".to_string()),
            client_id_issued_at: None,
            client_secret_expires_at: None,
            redirect_uris: Vec::new(),
            token_endpoint_auth_method: None,
            extra: Default::default(),
        };
        assert_eq!(
            select_client_auth_method(&information, &["client_secret_basic".to_string()]),
            ClientAuthMethod::Basic
        );
    }
}
