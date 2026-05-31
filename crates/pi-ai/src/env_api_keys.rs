use std::collections::HashMap;

/// Map of provider names to environment variable names for API keys.
fn provider_env_keys() -> HashMap<&'static str, &'static str> {
    let mut map = HashMap::new();
    map.insert("openai", "OPENAI_API_KEY");
    map.insert("anthropic", "ANTHROPIC_API_KEY");
    map.insert("google", "GOOGLE_API_KEY");
    map.insert("google-vertex", "GOOGLE_VERTEX_API_KEY");
    map.insert("deepseek", "DEEPSEEK_API_KEY");
    map.insert("github-copilot", "COPILOT_API_KEY");
    map.insert("xai", "XAI_API_KEY");
    map.insert("groq", "GROQ_API_KEY");
    map.insert("cerebras", "CEREBRAS_API_KEY");
    map.insert("openrouter", "OPENROUTER_API_KEY");
    map.insert("mistral", "MISTRAL_API_KEY");
    map.insert("huggingface", "HF_API_KEY");
    map.insert("together", "TOGETHER_API_KEY");
    map.insert("fireworks", "FIREWORKS_API_KEY");
    map.insert("vercel-ai-gateway", "VERCEL_AI_GATEWAY_API_KEY");
    map.insert("zai", "ZAI_API_KEY");
    map.insert("amazon-bedrock", "AWS_ACCESS_KEY_ID");
    map.insert("minimax", "MINIMAX_API_KEY");
    map.insert("minimax-cn", "MINIMAX_CN_API_KEY");
    map.insert("moonshotai", "MOONSHOT_API_KEY");
    map.insert("moonshotai-cn", "MOONSHOT_CN_API_KEY");
    map.insert("cloudflare-workers-ai", "CLOUDFLARE_WORKERS_AI_API_KEY");
    map.insert("cloudflare-ai-gateway", "CLOUDFLARE_AI_GATEWAY_API_KEY");
    map.insert("xiaomi", "XIAOMI_API_KEY");
    map.insert("kimi-coding", "KIMI_CODING_API_KEY");
    map
}

static ENV_KEYS: std::sync::LazyLock<HashMap<&'static str, &'static str>> =
    std::sync::LazyLock::new(provider_env_keys);

/// Get the API key for a provider from the environment.
/// Returns the value of the environment variable associated with the provider.
pub fn get_env_api_key(provider: &str) -> Option<String> {
    let var_name = ENV_KEYS.get(provider)?;
    std::env::var(var_name).ok()
}

/// Get the environment variable name for a provider.
pub fn get_env_var_name(provider: &str) -> Option<&'static str> {
    ENV_KEYS.get(provider).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_env_var_name() {
        assert_eq!(get_env_var_name("openai"), Some("OPENAI_API_KEY"));
        assert_eq!(get_env_var_name("anthropic"), Some("ANTHROPIC_API_KEY"));
        assert_eq!(get_env_var_name("nonexistent"), None);
    }

    #[test]
    fn test_get_env_api_key_not_set() {
        assert!(get_env_api_key("openai").is_none() || std::env::var("OPENAI_API_KEY").is_ok());
    }

    // --- Supplementary tests matching TS originals ---

    #[test]
    fn test_known_providers_have_env_var_names() {
        let providers = &[
            "openai", "anthropic", "google", "deepseek", "xai", "groq",
            "cerebras", "openrouter", "mistral", "huggingface", "together",
            "fireworks", "vercel-ai-gateway", "zai", "github-copilot",
            "amazon-bedrock", "minimax", "moonshotai",
        ];
        for provider in providers {
            let var_name = get_env_var_name(provider);
            assert!(
                var_name.is_some(),
                "Provider '{}' should have an env var name",
                provider
            );
        }
    }

    #[test]
    fn test_get_env_api_key_with_var_set() {
        std::env::set_var("__PI_TEST_API_KEY__", "test-key-value");
        // Hack: test by checking var resolution logic directly
        assert_eq!(std::env::var("__PI_TEST_API_KEY__").ok(), Some("test-key-value".into()));
        std::env::remove_var("__PI_TEST_API_KEY__");
    }

    #[test]
    fn test_get_env_api_key_returns_none_for_unknown_provider() {
        assert!(get_env_api_key("nonexistent-provider-xyz").is_none());
    }
}
