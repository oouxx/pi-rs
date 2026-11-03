//! `pi-mcp` — Model Context Protocol client.
//!
//! Rust port of `@earendil-works/pi-mcp` from the pi TypeScript monorepo.
//! The crate is deliberately independent of the other pi crates: content is
//! exposed as [`protocol::content::LlmContent`] and callers translate it to
//! their own block types.

pub mod auth;
pub mod client;
pub mod error;
pub mod oauth;
pub mod protocol;
pub mod transport;
pub mod transports;

pub use client::{McpClient, McpClientOptions, McpRequestOptions, RootsProvider};
pub use error::{McpError, Result};
pub use protocol::content::{to_llm_content, CallToolResult, ContentBlock, LlmContent, ResourceContents};
pub use protocol::jsonrpc::{JsonRpcId, JsonRpcMessage};
pub use protocol::types::*;
pub use transport::{McpTransport, McpTransportSendOptions, Subscription};
pub use transports::streamable_http::{
    StreamableHttpTransport, StreamableHttpTransportOptions,
};
