//! Streaming entry points for pi-ai.
//!
//! Provides `stream()`, `complete()`, `stream_simple()`, and `complete_simple()`
//! as the main API for calling LLM providers.

use crate::api_registry::get_api_provider;
use crate::types::{AssistantMessage, Context, Model, SimpleStreamOptions, StreamOptions};
use crate::utils::event_stream::AssistantMessageEventStream;

/// Resolve the API provider for a given API, panicking if none registered.
fn resolve_api_provider(api: &str) -> crate::api_registry::ApiProvider {
    get_api_provider(api).unwrap_or_else(|| panic!("No API provider registered for api: {api}"))
}

/// Stream a completion from the given model.
#[must_use] 
pub fn stream(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> AssistantMessageEventStream {
    let provider = resolve_api_provider(&model.api);
    // Treat a blank explicit key as unset so it falls through to the
    // provider's env fallback (TS auth resolution treats an empty string as
    // unset). `with_env_api_key` used to do this normalization before it was
    // removed in favour of provider-side resolution.
    let options = options.map(|mut opts| {
        if opts.api_key.as_deref().is_some_and(|key| key.trim().is_empty()) {
            opts.api_key = None;
        }
        opts
    });
    (provider.stream)(model, context, options.as_ref())
}

/// Complete a request and return the final `AssistantMessage`.
pub async fn complete(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> Result<AssistantMessage, String> {
    stream(model, context, options).result().await
}

/// Stream a completion using simplified options (with reasoning support).
#[must_use] 
pub fn stream_simple(
    model: &Model,
    context: &Context,
    options: Option<SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    let provider = resolve_api_provider(&model.api);
    let opts = options.unwrap_or_default();
    (provider.stream_simple)(model, context, Some(&opts))
}

/// Complete a request using simplified options.
pub async fn complete_simple(
    model: &Model,
    context: &Context,
    options: Option<SimpleStreamOptions>,
) -> Result<AssistantMessage, String> {
    stream_simple(model, context, options).result().await
}

