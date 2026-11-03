//! OAuth metadata types and structural validation.
//!
//! Port of `packages/mcp/src/oauth/types.ts`. Validation is dependency-free
//! and mirrors the TS `parse*` functions: absent (`null`/`""`) optional
//! values become `None`, URLs must be `http`/`https`, and unknown fields are
//! preserved.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::oauth::errors::OAuthError;

fn object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>, OAuthError> {
    value
        .as_object()
        .ok_or_else(|| OAuthError::Message(format!("Invalid {name}")))
}

fn required_string(value: Option<&Value>, name: &str) -> Result<String, OAuthError> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        _ => Err(OAuthError::Message(format!("Invalid {name}"))),
    }
}

fn absent(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        _ => false,
    }
}

fn optional_string(value: Option<&Value>, name: &str) -> Result<Option<String>, OAuthError> {
    if absent(value) {
        return Ok(None);
    }
    required_string(value, name).map(Some)
}

fn optional_strings(value: Option<&Value>, name: &str) -> Result<Option<Vec<String>>, OAuthError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(text) => result.push(text.clone()),
                    _ => return Err(OAuthError::Message(format!("Invalid {name}"))),
                }
            }
            Ok(Some(result))
        }
        _ => Err(OAuthError::Message(format!("Invalid {name}"))),
    }
}

fn is_http_url(url: &Url) -> bool {
    url.scheme() == "http" || url.scheme() == "https"
}

fn safe_url(value: Option<&Value>, name: &str) -> Result<String, OAuthError> {
    let text = required_string(value, name)?;
    let url = Url::parse(&text).map_err(|_| OAuthError::Message(format!("Invalid {name}")))?;
    if !is_http_url(&url) {
        return Err(OAuthError::Message(format!("Invalid {name}")));
    }
    Ok(text)
}

fn optional_url(value: Option<&Value>, name: &str) -> Result<Option<String>, OAuthError> {
    if absent(value) {
        return Ok(None);
    }
    safe_url(value, name).map(Some)
}

fn optional_bool(value: Option<&Value>) -> Option<bool> {
    value.and_then(Value::as_bool)
}

/// Protected resource metadata (`/.well-known/oauth-protected-resource`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthProtectedResourceMetadata {
    /// Canonical resource URI.
    pub resource: String,
    /// Authorization servers that can issue tokens for the resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_servers: Option<Vec<String>>,
    /// Scopes the resource supports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,
    /// Unknown fields preserved from the document.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Authorization server metadata (RFC 8414 or OIDC discovery).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationServerMetadata {
    /// Issuer.
    pub issuer: String,
    /// Authorization endpoint.
    pub authorization_endpoint: String,
    /// Token endpoint.
    pub token_endpoint: String,
    /// Dynamic client registration endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    /// Supported scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,
    /// Supported response types.
    pub response_types_supported: Vec<String>,
    /// Supported grant types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_types_supported: Option<Vec<String>>,
    /// Token endpoint auth methods.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
    /// PKCE challenge methods.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_challenge_methods_supported: Option<Vec<String>>,
    /// Whether CIMD is supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_metadata_document_supported: Option<bool>,
    /// Whether authorization responses carry `iss` (RFC 9207).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_response_iss_parameter_supported: Option<bool>,
    /// Unknown fields preserved from the document.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Tokens returned by the token endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// Bearer access token.
    pub access_token: String,
    /// Token type (usually `Bearer`).
    pub token_type: String,
    /// Lifetime in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<f64>,
    /// Granted scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Refresh token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// OpenID Connect ID token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}

/// Client metadata sent with dynamic registration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthClientMetadata {
    /// Redirect URIs.
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    /// Token endpoint auth method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<String>,
    /// Grant types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_types: Option<Vec<String>>,
    /// Response types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_types: Option<Vec<String>>,
    /// OIDC application type (`native` or `web`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_type: Option<String>,
    /// Client name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    /// Client homepage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_uri: Option<String>,
    /// Logo URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
    /// Requested scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Contact addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contacts: Option<Vec<String>>,
    /// Unknown metadata fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Registered client information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthClientInformation {
    /// Client id.
    pub client_id: String,
    /// Optional client secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// When the client id was issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id_issued_at: Option<f64>,
    /// When the client secret expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<f64>,
    /// Redirect URIs registered for the client.
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    /// Token endpoint auth method hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<String>,
    /// Unknown fields preserved from the registration response.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Cached discovery result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthDiscoveryState {
    /// Authorization server URL.
    pub authorization_server_url: String,
    /// Discovered metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_server_metadata: Option<AuthorizationServerMetadata>,
    /// Protected resource metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_metadata: Option<OAuthProtectedResourceMetadata>,
    /// URL the resource metadata came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_metadata_url: Option<String>,
}

/// Result of OAuth discovery.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OAuthServerInfo {
    /// Authorization server URL.
    pub authorization_server_url: String,
    /// Discovered metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_server_metadata: Option<AuthorizationServerMetadata>,
    /// Protected resource metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_metadata: Option<OAuthProtectedResourceMetadata>,
}

/// Parsed `WWW-Authenticate` challenge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthChallenge {
    /// `resource_metadata` URL.
    pub resource_metadata_url: Option<Url>,
    /// Requested scope.
    pub scope: Option<String>,
    /// Error code.
    pub error: Option<String>,
    /// Error description.
    pub error_description: Option<String>,
}

/// Parse protected resource metadata.
pub fn parse_protected_resource_metadata(
    value: &Value,
) -> Result<OAuthProtectedResourceMetadata, OAuthError> {
    let input = object(value, "OAuth protected resource metadata")?;
    let mut extra = input.clone();
    extra.remove("resource");
    extra.remove("authorization_servers");
    extra.remove("scopes_supported");
    let resource = safe_url(
        input.get("resource"),
        "OAuth protected resource metadata resource",
    )?;
    let authorization_servers = optional_strings(input.get("authorization_servers"), "authorization_servers")?
        .map(|servers| {
            servers
                .into_iter()
                .map(|server| {
                    let value = Value::String(server);
                    safe_url(Some(&value), "authorization server URL")
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(OAuthProtectedResourceMetadata {
        resource,
        authorization_servers,
        scopes_supported: optional_strings(input.get("scopes_supported"), "scopes_supported")?,
        extra,
    })
}

/// Parse authorization server metadata.
pub fn parse_authorization_server_metadata(
    value: &Value,
) -> Result<AuthorizationServerMetadata, OAuthError> {
    let input = object(value, "authorization server metadata")?;
    let response_types =
        optional_strings(input.get("response_types_supported"), "response_types_supported")?
            .ok_or_else(|| OAuthError::Message("Invalid response_types_supported".to_string()))?;
    let mut extra = input.clone();
    for key in [
        "issuer",
        "authorization_endpoint",
        "token_endpoint",
        "registration_endpoint",
        "scopes_supported",
        "response_types_supported",
        "grant_types_supported",
        "token_endpoint_auth_methods_supported",
        "code_challenge_methods_supported",
        "client_id_metadata_document_supported",
        "authorization_response_iss_parameter_supported",
    ] {
        extra.remove(key);
    }
    Ok(AuthorizationServerMetadata {
        issuer: safe_url(input.get("issuer"), "authorization server issuer")?,
        authorization_endpoint: safe_url(input.get("authorization_endpoint"), "authorization endpoint")?,
        token_endpoint: safe_url(input.get("token_endpoint"), "token endpoint")?,
        registration_endpoint: optional_url(input.get("registration_endpoint"), "registration endpoint")?,
        scopes_supported: optional_strings(input.get("scopes_supported"), "scopes_supported")?,
        response_types_supported: response_types,
        grant_types_supported: optional_strings(
            input.get("grant_types_supported"),
            "grant_types_supported",
        )?,
        token_endpoint_auth_methods_supported: optional_strings(
            input.get("token_endpoint_auth_methods_supported"),
            "token_endpoint_auth_methods_supported",
        )?,
        code_challenge_methods_supported: optional_strings(
            input.get("code_challenge_methods_supported"),
            "code_challenge_methods_supported",
        )?,
        client_id_metadata_document_supported: optional_bool(
            input.get("client_id_metadata_document_supported"),
        ),
        authorization_response_iss_parameter_supported: optional_bool(
            input.get("authorization_response_iss_parameter_supported"),
        ),
        extra,
    })
}

/// Parse a token endpoint response.
pub fn parse_oauth_tokens(value: &Value) -> Result<OAuthTokens, OAuthError> {
    let input = object(value, "OAuth token response")?;
    let expires = match input.get("expires_in") {
        None | Some(Value::Null) => None,
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) if !text.is_empty() => text.parse::<f64>().ok(),
        Some(_) => None,
    };
    if input.get("expires_in").is_some()
        && !absent(input.get("expires_in"))
        && expires.is_none()
    {
        return Err(OAuthError::Message("Invalid expires_in".to_string()));
    }
    Ok(OAuthTokens {
        access_token: required_string(input.get("access_token"), "access_token")?,
        token_type: required_string(input.get("token_type"), "token_type")?,
        expires_in: expires,
        scope: optional_string(input.get("scope"), "scope")?,
        refresh_token: optional_string(input.get("refresh_token"), "refresh_token")?,
        id_token: optional_string(input.get("id_token"), "id_token")?,
    })
}

/// Parse a dynamic client registration response.
pub fn parse_client_information(value: &Value) -> Result<OAuthClientInformation, OAuthError> {
    let input = object(value, "OAuth client registration response")?;
    let mut extra = input.clone();
    for key in [
        "client_id",
        "client_secret",
        "client_id_issued_at",
        "client_secret_expires_at",
        "redirect_uris",
        "token_endpoint_auth_method",
    ] {
        extra.remove(key);
    }
    Ok(OAuthClientInformation {
        client_id: required_string(input.get("client_id"), "client_id")?,
        client_secret: optional_string(input.get("client_secret"), "client_secret")?,
        client_id_issued_at: input.get("client_id_issued_at").and_then(Value::as_f64),
        client_secret_expires_at: input.get("client_secret_expires_at").and_then(Value::as_f64),
        redirect_uris: optional_strings(input.get("redirect_uris"), "redirect_uris")?.unwrap_or_default(),
        token_endpoint_auth_method: optional_string(
            input.get("token_endpoint_auth_method"),
            "token_endpoint_auth_method",
        )?,
        extra,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_urls_must_be_http() {
        assert!(parse_protected_resource_metadata(&json!({"resource": "file:///x"})).is_err());
        assert!(parse_protected_resource_metadata(&json!({"resource": "https://example.com"})).is_ok());
    }

    #[test]
    fn token_expiry_accepts_numbers_and_numeric_strings() {
        let tokens = parse_oauth_tokens(&json!({
            "access_token": "a", "token_type": "Bearer", "expires_in": "3600"
        }))
        .unwrap();
        assert_eq!(tokens.expires_in, Some(3600.0));
        assert!(parse_oauth_tokens(&json!({
            "access_token": "a", "token_type": "Bearer", "expires_in": "soon"
        }))
        .is_err());
    }

    #[test]
    fn unknown_fields_are_preserved() {
        let metadata = parse_authorization_server_metadata(&json!({
            "issuer": "https://example.com",
            "authorization_endpoint": "https://example.com/authorize",
            "token_endpoint": "https://example.com/token",
            "response_types_supported": ["code"],
            "custom": true
        }))
        .unwrap();
        assert_eq!(metadata.extra.get("custom"), Some(&json!(true)));
    }
}
