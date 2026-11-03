//! OAuth discovery: protected resource metadata, authorization server
//! metadata, and `WWW-Authenticate` parsing.
//!
//! Port of `packages/mcp/src/oauth/discovery.ts`.

use reqwest::Client;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::oauth::errors::OAuthError;
use crate::oauth::types::{
    parse_authorization_server_metadata, parse_protected_resource_metadata,
    AuthorizationServerMetadata, OAuthChallenge, OAuthProtectedResourceMetadata, OAuthServerInfo,
};
use crate::protocol::types::LATEST_PROTOCOL_VERSION;

/// Options for discovery requests.
#[derive(Clone, Default)]
pub struct DiscoveryOptions {
    /// Explicit protected-resource metadata URL.
    pub resource_metadata_url: Option<Url>,
    /// Explicit authorization-server metadata URL (trusted as configured).
    pub authorization_server_metadata_url: Option<Url>,
    /// Protocol version header.
    pub protocol_version: Option<String>,
    /// Skip issuer validation.
    pub skip_issuer_validation: bool,
    /// Cancellation token.
    pub cancel: Option<CancellationToken>,
}

/// 4xx and 502 mean "not here", so discovery tries the next candidate URL.
fn is_discovery_miss(status: u16) -> bool {
    (400..500).contains(&status) || status == 502
}

/// Path suffix for `/.well-known/<kind><path>`; empty for the root path.
fn path_suffix(pathname: &str) -> String {
    pathname.strip_suffix('/').unwrap_or(pathname).to_string()
}

fn field(header: &str, name: &str) -> Option<String> {
    // Equivalent to TS `(?:^|[,\s])name=(?:"([^"]*)"|([^\s,]+))`.
    let lower = header.to_ascii_lowercase();
    let needle = format!("{name}=");
    let mut search_from = 0;
    while let Some(index) = lower[search_from..].find(&needle) {
        let start = search_from + index;
        let boundary = start == 0
            || header[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c == ',' || c.is_whitespace());
        if !boundary {
            search_from = start + needle.len();
            continue;
        }
        let value_start = start + needle.len();
        let rest = &header[value_start..];
        if let Some(quoted) = rest.strip_prefix('"') {
            if let Some(end) = quoted.find('"') {
                let value = &quoted[..end];
                return if value.is_empty() { None } else { Some(value.to_string()) };
            }
            return None;
        }
        let value = rest
            .split(|c: char| c == ',' || c.is_whitespace())
            .next()
            .unwrap_or_default();
        return if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        };
    }
    None
}

/// Parse a `WWW-Authenticate` header.
pub fn parse_www_authenticate(header: Option<&str>) -> OAuthChallenge {
    let Some(header) = header else {
        return OAuthChallenge::default();
    };
    let scheme = header
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if scheme != "bearer" && scheme != "dpop" {
        return OAuthChallenge::default();
    }
    let resource_metadata = field(header, "resource_metadata");
    OAuthChallenge {
        resource_metadata_url: resource_metadata.and_then(|value| Url::parse(&value).ok()),
        scope: field(header, "scope"),
        error: field(header, "error"),
        error_description: field(header, "error_description"),
    }
}

async fn fetch_metadata(
    url: &Url,
    client: &Client,
    protocol_version: &str,
    cancel: Option<&CancellationToken>,
) -> Result<reqwest::Response, OAuthError> {
    let request = client
        .get(url.clone())
        .header("Accept", "application/json")
        .header("MCP-Protocol-Version", protocol_version);
    if let Some(token) = cancel {
        let response = tokio::select! {
            () = token.cancelled() => return Err(OAuthError::Network("aborted".to_string())),
            response = request.send() => response?,
        };
        return Ok(response);
    }
    Ok(request.send().await?)
}

fn well_known_url(server: &Url, kind: &str, path: &str) -> Result<Url, OAuthError> {
    let mut url = server.clone();
    url.set_path(&format!("/.well-known/{kind}{path}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

/// Discover protected resource metadata.
pub async fn discover_protected_resource_metadata(
    server_url: &Url,
    client: &Client,
    options: &DiscoveryOptions,
) -> Result<OAuthProtectedResourceMetadata, OAuthError> {
    let protocol_version = options
        .protocol_version
        .clone()
        .unwrap_or_else(|| LATEST_PROTOCOL_VERSION.to_string());
    let path = path_suffix(server_url.path());
    let first = match &options.resource_metadata_url {
        Some(url) => url.clone(),
        None => well_known_url(server_url, "oauth-protected-resource", &path)?,
    };
    let mut response = fetch_metadata(&first, client, &protocol_version, options.cancel.as_ref()).await?;
    if options.resource_metadata_url.is_none()
        && server_url.path() != "/"
        && is_discovery_miss(response.status().as_u16())
    {
        let root = well_known_url(server_url, "oauth-protected-resource", "")?;
        response = fetch_metadata(&root, client, &protocol_version, options.cancel.as_ref()).await?;
    }
    if !response.status().is_success() {
        return Err(OAuthError::Message(format!(
            "HTTP {} loading OAuth protected resource metadata",
            response.status().as_u16()
        )));
    }
    let value: Value = response.json().await?;
    parse_protected_resource_metadata(&value)
}

/// Candidate authorization server metadata URLs, in the order TS tries them.
pub fn build_authorization_server_discovery_urls(
    authorization_server_url: &Url,
) -> Result<Vec<Url>, OAuthError> {
    let path = path_suffix(authorization_server_url.path());
    let mut urls = vec![
        well_known_url(authorization_server_url, "oauth-authorization-server", &path)?,
        well_known_url(authorization_server_url, "openid-configuration", &path)?,
    ];
    if !path.is_empty() {
        let mut url = authorization_server_url.clone();
        url.set_path(&format!("{path}/.well-known/openid-configuration"));
        url.set_query(None);
        url.set_fragment(None);
        urls.push(url);
    }
    Ok(urls)
}

/// Discover authorization server metadata.
pub async fn discover_authorization_server_metadata(
    authorization_server_url: &Url,
    client: &Client,
    options: &DiscoveryOptions,
) -> Result<Option<AuthorizationServerMetadata>, OAuthError> {
    let protocol_version = options
        .protocol_version
        .clone()
        .unwrap_or_else(|| LATEST_PROTOCOL_VERSION.to_string());
    for url in build_authorization_server_discovery_urls(authorization_server_url)? {
        let response = fetch_metadata(&url, client, &protocol_version, options.cancel.as_ref()).await?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            if is_discovery_miss(status) {
                continue;
            }
            return Err(OAuthError::Message(format!(
                "HTTP {status} loading authorization server metadata from {url}"
            )));
        }
        let value: Value = response.json().await?;
        let metadata = parse_authorization_server_metadata(&value)?;
        if !options.skip_issuer_validation {
            let expected = trim_trailing_slash(authorization_server_url.as_str());
            let received = trim_trailing_slash(&metadata.issuer);
            if expected != received {
                return Err(OAuthError::IssuerMismatch {
                    expected: Some(expected.to_string()),
                    received: Some(received.to_string()),
                });
            }
        }
        return Ok(Some(metadata));
    }
    Ok(None)
}

fn trim_trailing_slash(value: &str) -> &str {
    value.strip_suffix('/').unwrap_or(value)
}

/// Discover protected resource metadata and authorization server metadata.
///
/// A protected-resource failure is ignored unless it is a network failure,
/// matching TS (`if (error instanceof TypeError) throw error`).
pub async fn discover_oauth_server_info(
    server_url: &Url,
    client: &Client,
    options: &DiscoveryOptions,
) -> Result<OAuthServerInfo, OAuthError> {
    let resource_metadata = match discover_protected_resource_metadata(server_url, client, options).await {
        Ok(metadata) => Some(metadata),
        Err(error @ OAuthError::Network(_)) => return Err(error),
        Err(_) => None,
    };

    if let Some(metadata_url) = &options.authorization_server_metadata_url {
        let protocol_version = options
            .protocol_version
            .clone()
            .unwrap_or_else(|| LATEST_PROTOCOL_VERSION.to_string());
        let response = fetch_metadata(
            metadata_url,
            client,
            &protocol_version,
            options.cancel.as_ref(),
        )
        .await?;
        if !response.status().is_success() {
            return Err(OAuthError::Message(format!(
                "HTTP {} loading authorization server metadata from {metadata_url}",
                response.status().as_u16()
            )));
        }
        let value: Value = response.json().await?;
        let metadata = parse_authorization_server_metadata(&value)?;
        return Ok(OAuthServerInfo {
            authorization_server_url: metadata.issuer.clone(),
            authorization_server_metadata: Some(metadata),
            resource_metadata,
        });
    }

    let authorization_server_url = resource_metadata
        .as_ref()
        .and_then(|metadata| metadata.authorization_servers.as_ref())
        .and_then(|servers| servers.first())
        .map(|url| Url::parse(url))
        .transpose()?
        .unwrap_or_else(|| {
            let mut root = server_url.clone();
            root.set_path("/");
            root.set_query(None);
            root.set_fragment(None);
            root
        });
    let metadata = discover_authorization_server_metadata(&authorization_server_url, client, options).await?;
    Ok(OAuthServerInfo {
        authorization_server_url: authorization_server_url.to_string(),
        authorization_server_metadata: metadata,
        resource_metadata,
    })
}

/// Remove the fragment from a server URL.
pub fn resource_url_from_server_url(value: &Url) -> Url {
    let mut url = value.clone();
    url.set_fragment(None);
    url
}

/// Select the protected resource for the MCP server.
pub fn select_resource(
    server_url: &Url,
    metadata: Option<&OAuthProtectedResourceMetadata>,
) -> Result<Option<String>, OAuthError> {
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let requested = resource_url_from_server_url(server_url);
    let configured = Url::parse(&metadata.resource)?;
    if requested.origin() != configured.origin() {
        return Err(OAuthError::Message(format!(
            "Protected resource {} does not match MCP server {requested}",
            metadata.resource
        )));
    }
    let requested_path = if requested.path().ends_with('/') {
        requested.path().to_string()
    } else {
        format!("{}/", requested.path())
    };
    let configured_path = if configured.path().ends_with('/') {
        configured.path().to_string()
    } else {
        format!("{}/", configured.path())
    };
    if !requested_path.starts_with(&configured_path) {
        return Err(OAuthError::Message(format!(
            "Protected resource {} does not match MCP server {requested}",
            metadata.resource
        )));
    }
    Ok(Some(metadata.resource.clone()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn parses_bearer_challenge() {
        let challenge = parse_www_authenticate(Some(
            "Bearer resource_metadata=\"https://example.com/meta\", scope=\"read write\", error=\"insufficient_scope\"",
        ));
        assert_eq!(
            challenge.resource_metadata_url,
            Some(Url::parse("https://example.com/meta").unwrap())
        );
        assert_eq!(challenge.scope.as_deref(), Some("read write"));
        assert_eq!(challenge.error.as_deref(), Some("insufficient_scope"));
    }

    #[test]
    fn ignores_non_bearer_challenges() {
        assert_eq!(
            parse_www_authenticate(Some("Basic realm=\"x\"")),
            OAuthChallenge::default()
        );
    }

    #[test]
    fn builds_discovery_urls_in_order() {
        let urls = build_authorization_server_discovery_urls(
            &Url::parse("https://example.com/tenant").unwrap(),
        )
        .unwrap();
        assert_eq!(urls[0].as_str(), "https://example.com/.well-known/oauth-authorization-server/tenant");
        assert_eq!(urls[1].as_str(), "https://example.com/.well-known/openid-configuration/tenant");
        assert_eq!(
            urls[2].as_str(),
            "https://example.com/tenant/.well-known/openid-configuration"
        );
    }

    #[test]
    fn select_resource_requires_origin_and_path_prefix() {
        let server = Url::parse("https://example.com/mcp").unwrap();
        let metadata = OAuthProtectedResourceMetadata {
            resource: "https://example.com/mcp".to_string(),
            authorization_servers: None,
            scopes_supported: None,
            extra: Default::default(),
        };
        assert_eq!(
            select_resource(&server, Some(&metadata)).unwrap(),
            Some("https://example.com/mcp".to_string())
        );
        let other = OAuthProtectedResourceMetadata {
            resource: "https://other.example.com/mcp".to_string(),
            ..metadata
        };
        assert!(select_resource(&server, Some(&other)).is_err());
    }
}
