//! API provider registry — maps API names to streaming functions.
//!
//! Providers are registered by API format (e.g., "openai-completions", "anthropic-messages").
//! Different backends within the same API format are distinguished by their Model configuration.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::types::{Context, Model, SimpleStreamOptions, StreamOptions};
use crate::utils::event_stream::AssistantMessageEventStream;

/// Streaming function signature (full options).
pub type ApiStreamFn = Arc<
    dyn Fn(&Model, &Context, Option<&StreamOptions>) -> AssistantMessageEventStream + Send + Sync,
>;

/// Streaming function signature (simple options).
pub type ApiStreamSimpleFn = Arc<
    dyn Fn(&Model, &Context, Option<&SimpleStreamOptions>) -> AssistantMessageEventStream
        + Send
        + Sync,
>;

/// An API provider registered for a specific API format.
#[derive(Clone)]
pub struct ApiProvider {
    pub api: String,
    pub stream: ApiStreamFn,
    pub stream_simple: ApiStreamSimpleFn,
}

struct RegisteredProvider {
    provider: Arc<ApiProvider>,
    source_id: Option<String>,
}

static REGISTRY: std::sync::LazyLock<RwLock<HashMap<String, RegisteredProvider>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register an API provider. If a provider with the same API already exists, it is replaced.
pub fn register_api_provider(provider: ApiProvider, source_id: Option<&str>) {
    let mut registry = REGISTRY.write().unwrap();
    registry.insert(
        provider.api.clone(),
        RegisteredProvider {
            provider: Arc::new(provider),
            source_id: source_id.map(|s| s.to_string()),
        },
    );
}

/// Look up a registered API provider by API name.
/// Returns a cloneable handle — each call copies the Arc, so it's cheap.
pub fn get_api_provider(api: &str) -> Option<ApiProvider> {
    let registry = REGISTRY.read().unwrap();
    registry.get(api).map(|r| ApiProvider {
        api: r.provider.api.clone(),
        stream: Arc::clone(&r.provider.stream),
        stream_simple: Arc::clone(&r.provider.stream_simple),
    })
}

/// Get all registered API providers.
pub fn get_api_providers() -> Vec<ApiProvider> {
    let registry = REGISTRY.read().unwrap();
    registry
        .values()
        .map(|r| ApiProvider {
            api: r.provider.api.clone(),
            stream: Arc::clone(&r.provider.stream),
            stream_simple: Arc::clone(&r.provider.stream_simple),
        })
        .collect()
}

/// Unregister all API providers registered under the given source ID.
pub fn unregister_api_providers(source_id: &str) {
    let mut registry = REGISTRY.write().unwrap();
    registry.retain(|_, r| r.source_id.as_deref() != Some(source_id));
}

/// Clear all registered API providers.
pub fn clear_api_providers() {
    let mut registry = REGISTRY.write().unwrap();
    registry.clear();
}
