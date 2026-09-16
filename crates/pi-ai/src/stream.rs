//! Streaming entry points for pi-ai.
//!
//! Provides `stream()`, `complete()`, `stream_simple()`, and `complete_simple()`
//! as the main API for calling LLM providers.

use crate::api_registry::get_api_provider;
use crate::types::{
    AssistantMessage, AssistantMessageEvent, Context, Model, SimpleStreamOptions, StopReason,
    StreamOptions, Usage,
};
use crate::utils::event_stream::AssistantMessageEventStream;

/// Error stream returned when the model's `api` has no registered backend.
/// Models can be present in the catalog while their streaming backend is not
/// ported yet (e.g. `bedrock-converse-stream`, `google-vertex`); match TS by
/// surfacing an error message instead of panicking.
fn missing_api_provider_stream(model: &Model) -> AssistantMessageEventStream {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let error = AssistantMessage {
        content: vec![],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Error,
        error_message: Some(format!(
            "No API provider registered for api \"{}\" (provider \"{}\") — this backend is not implemented in pi-rs",
            model.api, model.provider
        )),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    };
    let _ = tx.send(AssistantMessageEvent::Error {
        reason: StopReason::Error,
        error,
    });
    AssistantMessageEventStream::from_receiver(rx)
}

/// Stream a completion from the given model.
#[must_use] 
pub fn stream(
    model: &Model,
    context: &Context,
    options: Option<StreamOptions>,
) -> AssistantMessageEventStream {
    let Some(provider) = get_api_provider(&model.api) else {
        return missing_api_provider_stream(model);
    };
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
    let Some(provider) = get_api_provider(&model.api) else {
        return missing_api_provider_stream(model);
    };
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


#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::types::ModelCost;

    fn model(api: &str) -> Model {
        Model {
            id: "test-model".into(),
            name: "Test Model".into(),
            api: api.into(),
            provider: "test-provider".into(),
            base_url: "https://example.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: ModelCost::default(),
            context_window: 4096,
            max_tokens: 1024,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }

    /// A catalog model whose streaming backend is not ported (e.g.
    /// `bedrock-converse-stream`, `google-vertex`) must surface an error
    /// message instead of panicking.
    #[tokio::test]
    async fn test_unregistered_api_returns_error_stream() {
        let m = model("bedrock-converse-stream");
        let context = Context {
            system_prompt: None,
            messages: vec![],
            tools: None,
        };
        let message = stream(&m, &context, None).result().await.unwrap();
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.api, "bedrock-converse-stream");
        assert!(message
            .error_message
            .unwrap()
            .contains("No API provider registered"));
    }

    /// `stream_simple` has the same graceful behavior.
    #[tokio::test]
    async fn test_unregistered_api_simple_returns_error_stream() {
        let m = model("google-vertex");
        let context = Context {
            system_prompt: None,
            messages: vec![],
            tools: None,
        };
        let message = stream_simple(&m, &context, None).result().await.unwrap();
        assert_eq!(message.stop_reason, StopReason::Error);
    }
}
