use std::collections::HashMap;
use std::sync::RwLock;

use crate::types::{AssistantMessageEvent, Context, Model, SimpleStreamOptions, StreamOptions};

/// The function signature for a streaming provider implementation.
pub type ApiStreamFn = Box<
    dyn Fn(&Model, &Context, Option<&StreamOptions>) -> Box<dyn futures::Stream<Item = AssistantMessageEvent> + Unpin + Send>
        + Send
        + Sync,
>;

/// The function signature for a simple streaming provider implementation.
pub type ApiStreamSimpleFn = Box<
    dyn Fn(&Model, &Context, Option<&SimpleStreamOptions>) -> Box<dyn futures::Stream<Item = AssistantMessageEvent> + Unpin + Send>
        + Send
        + Sync,
>;

/// An API provider that can handle streaming completions.
pub struct ApiProvider {
    pub api: String,
    pub stream: ApiStreamFn,
    pub stream_simple: ApiStreamSimpleFn,
}

struct RegisteredProvider {
    provider: ApiProvider,
    source_id: Option<String>,
}

static REGISTRY: std::sync::LazyLock<RwLock<HashMap<String, RegisteredProvider>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register an API provider implementation.
pub fn register_api_provider(provider: ApiProvider, source_id: Option<&str>) {
    let mut registry = REGISTRY.write().unwrap();
    registry.insert(
        provider.api.clone(),
        RegisteredProvider {
            provider,
            source_id: source_id.map(|s| s.to_string()),
        },
    );
}

/// Look up a registered API provider by API name.
pub fn get_api_provider(api: &str) -> Option<ApiProvider> {
    let registry = REGISTRY.read().unwrap();
    registry.get(api).map(|r| ApiProvider {
        api: r.provider.api.clone(),
        stream: Box::new(|_, _, _| {
            panic!("Provider streams cannot be cloned — use the original")
        }),
        stream_simple: Box::new(|_, _, _| {
            panic!("Provider streams cannot be cloned — use the original")
        }),
    })
}

/// Get all registered API providers.
pub fn get_api_providers() -> Vec<ApiProvider> {
    let registry = REGISTRY.read().unwrap();
    registry
        .values()
        .map(|r| ApiProvider {
            api: r.provider.api.clone(),
            stream: Box::new(|_, _, _| {
                panic!("Provider streams cannot be cloned — use get_api_provider")
            }),
            stream_simple: Box::new(|_, _, _| {
                panic!("Provider streams cannot be cloned — use get_api_provider")
            }),
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
