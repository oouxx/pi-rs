//! OAuth support for remote MCP servers.
//!
//! Port of `packages/mcp/src/oauth/*`. The flow follows MCP's OAuth 2.1
//! profile: protected-resource metadata discovery, authorization-server
//! discovery, PKCE, dynamic client registration, and a loopback callback.

pub mod callback;
pub mod discovery;
pub mod errors;
pub mod flow;
pub mod provider;
pub mod types;

pub use discovery::{
    build_authorization_server_discovery_urls, discover_authorization_server_metadata,
    discover_oauth_server_info, discover_protected_resource_metadata, parse_www_authenticate,
    resource_url_from_server_url, select_resource,
};
pub use errors::OAuthError;
pub use flow::{
    adapt_oauth_provider, authorize_mcp, exchange_authorization_code, refresh_authorization,
    register_client, start_authorization, step_up_scope, OAuthClientMetadataDocument,
    OAuthClientProvider, OAuthFlowOptions, OAuthFlowResult,
};
pub use provider::{
    MemoryOAuthStateStore, McpOAuthProvider, McpOAuthProviderOptions, McpOAuthState,
    McpOAuthStateStore,
};
pub use types::*;
