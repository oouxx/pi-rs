//! Register built-in API providers.
//!
//! Ported from `packages/ai/src/providers/register-builtins.ts`.
//!
//! API providers are registered by API format:
//! - `anthropic-messages` — Anthropic Messages API (Anthropic, Vertex, Bedrock, etc.)
//! - `openai-completions` — OpenAI Chat Completions API (OpenAI, DeepSeek, xAI, Groq, Together, etc.)
//! - `mistral-conversations` — Mistral Conversations API
//!
//! Different providers within the same API format are distinguished by their Model
//! configuration (base_url, provider name, compat flags).

use crate::api_registry::{clear_api_providers, register_api_provider, ApiProvider};
use crate::providers::anthropic::{stream_anthropic, stream_simple_anthropic};
use crate::providers::openai::{stream_openai, stream_simple_openai};

/// Register all built-in API providers.
pub fn register_built_in_api_providers() {
    register_api_provider(
        ApiProvider {
            api: "anthropic-messages".to_string(),
            stream: Box::new(move |model, context, options| {
                Box::new(stream_anthropic(model, context, options))
            }),
            stream_simple: Box::new(move |model, context, options| {
                Box::new(stream_simple_anthropic(model, context, options))
            }),
        },
        Some("builtin"),
    );

    register_api_provider(
        ApiProvider {
            api: "openai-completions".to_string(),
            stream: Box::new(move |model, context, options| {
                Box::new(stream_openai(model, context, options))
            }),
            stream_simple: Box::new(move |model, context, options| {
                Box::new(stream_simple_openai(model, context, options))
            }),
        },
        Some("builtin"),
    );

    // Mistral uses OpenAI-compatible format
    register_api_provider(
        ApiProvider {
            api: "mistral-conversations".to_string(),
            stream: Box::new(move |model, context, options| {
                Box::new(stream_openai(model, context, options))
            }),
            stream_simple: Box::new(move |model, context, options| {
                Box::new(stream_simple_openai(model, context, options))
            }),
        },
        Some("builtin"),
    );
}

/// Clear all providers and re-register the built-in ones.
pub fn reset_api_providers() {
    clear_api_providers();
    register_built_in_api_providers();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_registry::get_api_provider;

    #[test]
    fn test_register_builtins_registers_anthropic() {
        register_built_in_api_providers();
        assert!(get_api_provider("anthropic-messages").is_some());
    }

    #[test]
    fn test_register_builtins_registers_openai() {
        register_built_in_api_providers();
        assert!(get_api_provider("openai-completions").is_some());
    }

    #[test]
    fn test_register_builtins_registers_mistral() {
        register_built_in_api_providers();
        assert!(get_api_provider("mistral-conversations").is_some());
    }

    #[test]
    fn test_reset_api_providers() {
        reset_api_providers();
        assert!(get_api_provider("anthropic-messages").is_some());
        assert!(get_api_provider("openai-completions").is_some());
        assert!(get_api_provider("mistral-conversations").is_some());
    }

    #[test]
    fn test_unknown_provider_returns_none() {
        register_built_in_api_providers();
        assert!(get_api_provider("unknown-api").is_none());
    }
}
