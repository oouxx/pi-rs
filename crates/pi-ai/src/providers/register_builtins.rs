//! Register built-in API providers.
//!
//! Ported from `packages/ai/src/providers/register-builtins.ts`.

use crate::api_registry::{clear_api_providers, register_api_provider, ApiProvider};
use crate::providers::anthropic::{stream_anthropic, stream_simple_anthropic};
use crate::providers::openai::{stream_openai, stream_simple_openai};

/// Register all built-in API providers.
///
/// Currently registered:
/// - `anthropic-messages` — Anthropic Messages API
/// - `openai-completions` — OpenAI Chat Completions API
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
        let provider = get_api_provider("anthropic-messages");
        // get_api_provider returns a provider (though stream fn's are not cloneable)
        // We mainly verify that the registration doesn't panic
        assert!(provider.is_some());
    }

    #[test]
    fn test_register_builtins_registers_openai() {
        register_built_in_api_providers();
        let provider = get_api_provider("openai-completions");
        assert!(provider.is_some());
    }

    #[test]
    fn test_reset_api_providers() {
        // Should not panic
        reset_api_providers();
        let anthropic = get_api_provider("anthropic-messages");
        let openai = get_api_provider("openai-completions");
        assert!(anthropic.is_some());
        assert!(openai.is_some());
    }

    #[test]
    fn test_unknown_provider_returns_none() {
        register_built_in_api_providers();
        let provider = get_api_provider("unknown-api");
        assert!(provider.is_none());
    }
}
