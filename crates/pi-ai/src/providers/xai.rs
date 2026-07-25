//! xAI Grok API provider (OpenAI-compatible).
//!
//! xAI's Grok models use the `OpenAI` Chat Completions API format.
//! This is a thin wrapper that delegates to the OpenAI-compatible streaming logic.

use crate::types::{Context, Model, SimpleStreamOptions, StreamOptions};
use crate::utils::event_stream::AssistantMessageEventStream;

/// Stream a completion from xAI Grok (OpenAI-compatible API).
#[must_use] 
pub fn stream_xai(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> AssistantMessageEventStream {
    crate::providers::openai::stream_openai(model, context, options)
}

/// Stream a completion from xAI Grok with simplified options.
#[must_use] 
pub fn stream_simple_xai(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> AssistantMessageEventStream {
    crate::providers::openai::stream_simple_openai(model, context, options)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn test_xai_module_functions_exist() {
        let _: fn(&Model, &Context, Option<&StreamOptions>) -> AssistantMessageEventStream =
            stream_xai;
        let _: fn(&Model, &Context, Option<&SimpleStreamOptions>) -> AssistantMessageEventStream =
            stream_simple_xai;
    }
}
