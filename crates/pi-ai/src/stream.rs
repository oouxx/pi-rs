use crate::api_registry::get_api_provider;
use crate::env_api_keys::get_env_api_key;
use crate::types::{
    AssistantMessage, Context, Model, SimpleStreamOptions, StreamOptions,
};
use crate::utils::event_stream::AssistantMessageEventStream;

/// Check if an explicit API key was provided.
fn has_explicit_api_key(api_key: Option<&str>) -> bool {
    api_key.map_or(false, |k| !k.trim().is_empty())
}

/// Resolve API key from options or environment, returning updated options.
fn with_env_api_key(
    model: &Model,
    options: Option<StreamOptions>,
) -> Option<StreamOptions> {
    let mut opts = options.unwrap_or_default();
    if has_explicit_api_key(opts.api_key.as_deref()) {
        return Some(opts);
    }
    if let Some(env_key) = get_env_api_key(&model.provider) {
        opts.api_key = Some(env_key);
    }
    Some(opts)
}

/// Resolve the API provider for a given API, throwing if none registered.
fn resolve_api_provider(api: &str) -> crate::api_registry::ApiProvider {
    get_api_provider(api).unwrap_or_else(|| {
        panic!("No API provider registered for api: {}", api)
    })
}

/// Stream a completion from the given model.
///
/// Returns an `AssistantMessageEventStream` that emits events as the model
/// generates content. Call `.result()` on the stream to await the final
/// `AssistantMessage`.
pub fn stream(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> AssistantMessageEventStream {
    let provider = resolve_api_provider(&model.api);
    let opts = with_env_api_key(model, options);
    let boxed_stream = (provider.stream)(model, context, opts.as_ref());
    AssistantMessageEventStream::new(boxed_stream)
}

/// Complete a request and return the final `AssistantMessage`.
///
/// Convenience wrapper around `stream()` that calls `.result()` internally.
pub async fn complete(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> Result<AssistantMessage, String> {
    stream(model, context, options).result().await
}

/// Stream a completion using simplified options (with reasoning support).
pub fn stream_simple(
    model: &Model,
    context: &Context,
    options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let provider = resolve_api_provider(&model.api);
    let opts = options.unwrap_or_default();
    let boxed_stream = (provider.stream_simple)(model, context, Some(&opts));
    AssistantMessageEventStream::new(boxed_stream)
}

/// Complete a request using simplified options.
///
/// Convenience wrapper around `stream_simple()` that calls `.result()` internally.
pub async fn complete_simple(
    model: &Model,
    context: &Context,
    options: Option<SimpleStreamOptions>,
) -> Result<AssistantMessage, String> {
    stream_simple(model, context, options).result().await
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            signal: None,
            api_key: None,
            transport: None,
            cache_retention: None,
            session_id: None,
            headers: None,
            timeout_ms: None,
            websocket_connect_timeout_ms: None,
            max_retries: None,
            max_retry_delay_ms: None,
            metadata: None,
        }
    }
}
