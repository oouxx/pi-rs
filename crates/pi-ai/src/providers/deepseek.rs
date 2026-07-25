//! `DeepSeek` API provider (OpenAI-compatible).
//!
//! `DeepSeek` uses the `OpenAI` Chat Completions API format. This is a thin
//! wrapper that delegates to the OpenAI-compatible streaming logic.

use crate::types::{Context, Model, SimpleStreamOptions, StreamOptions};
use crate::utils::event_stream::AssistantMessageEventStream;

/// Stream a completion from `DeepSeek` (OpenAI-compatible API).
#[must_use] 
pub fn stream_deepseek(
    model: &Model,
    context: &Context,
    options: Option<&StreamOptions>,
) -> AssistantMessageEventStream {
    crate::providers::openai::stream_openai(model, context, options)
}

/// Stream a completion from `DeepSeek` with simplified options.
#[must_use] 
pub fn stream_simple_deepseek(
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
    fn test_deepseek_module_functions_exist() {
        // Verify the module functions compile and have correct signatures.
        // Actual streaming requires a tokio runtime and valid API key,
        // so we just verify compilation here.
        let _: fn(&Model, &Context, Option<&StreamOptions>) -> AssistantMessageEventStream =
            stream_deepseek;
        let _: fn(&Model, &Context, Option<&SimpleStreamOptions>) -> AssistantMessageEventStream =
            stream_simple_deepseek;
    }
}
