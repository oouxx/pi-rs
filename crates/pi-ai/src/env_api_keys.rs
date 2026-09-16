//! Provider API-key environment discovery.
//!
//! Port of `packages/ai/src/env-api-keys.ts`. These functions only report
//! *configured API-key environment variables* for discovery/status; they are
//! intentionally unaware of ambient credential sources (AWS profiles, Google
//! ADC). Request-time auth resolution lives in the provider implementations.
//!
//! Deliberately not ported yet (tracked in `DEVIATIONS.md`): `getEnvApiKey`
//! returns the `"<authenticated>"` sentinel for `google-vertex` / `amazon-bedrock`
//! ambient credentials.

use std::collections::HashMap;

/// Provider-scoped environment overlay (match TS `ProviderEnv`).
pub type ProviderEnv = HashMap<String, String>;

/// Anthropic bearer token env var (match TS `ANTHROPIC_AUTH_TOKEN_ENV`).
/// Participates in env discovery/status, but `get_env_api_key` skips it because
/// requests must pass it as `Authorization: Bearer`.
pub const ANTHROPIC_AUTH_TOKEN_ENV: &str = "ANTHROPIC_AUTH_TOKEN";
/// Anthropic OAuth token env var (match TS `ANTHROPIC_OAUTH_TOKEN_ENV`).
pub const ANTHROPIC_OAUTH_TOKEN_ENV: &str = "ANTHROPIC_OAUTH_TOKEN";
/// Anthropic API key env var (match TS `ANTHROPIC_API_KEY_ENV`).
pub const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// Static provider → env var map (match TS `getApiKeyEnvVars` envMap).
/// `anthropic` and `github-copilot` are handled as special cases below.
const ENV_MAP: &[(&str, &str)] = &[
    ("ant-ling", "ANT_LING_API_KEY"),
    ("qwen-token-plan", "QWEN_TOKEN_PLAN_API_KEY"),
    ("qwen-token-plan-cn", "QWEN_TOKEN_PLAN_CN_API_KEY"),
    ("qwen-token-plan-individual", "QWEN_TOKEN_PLAN_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("azure-openai-responses", "AZURE_OPENAI_API_KEY"),
    ("nvidia", "NVIDIA_API_KEY"),
    ("deepseek", "DEEPSEEK_API_KEY"),
    ("google", "GEMINI_API_KEY"),
    ("google-vertex", "GOOGLE_CLOUD_API_KEY"),
    ("groq", "GROQ_API_KEY"),
    ("cerebras", "CEREBRAS_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("radius", "RADIUS_API_KEY"),
    ("openrouter", "OPENROUTER_API_KEY"),
    ("vercel-ai-gateway", "AI_GATEWAY_API_KEY"),
    ("zai", "ZAI_API_KEY"),
    ("zai-coding-cn", "ZAI_CODING_CN_API_KEY"),
    ("mistral", "MISTRAL_API_KEY"),
    ("minimax", "MINIMAX_API_KEY"),
    ("minimax-cn", "MINIMAX_CN_API_KEY"),
    ("moonshotai", "MOONSHOT_API_KEY"),
    ("moonshotai-cn", "MOONSHOT_API_KEY"),
    ("huggingface", "HF_TOKEN"),
    ("fireworks", "FIREWORKS_API_KEY"),
    ("together", "TOGETHER_API_KEY"),
    ("baseten", "BASETEN_API_KEY"),
    ("opencode", "OPENCODE_API_KEY"),
    ("opencode-go", "OPENCODE_API_KEY"),
    ("kimi-coding", "KIMI_API_KEY"),
    ("cloudflare-workers-ai", "CLOUDFLARE_API_KEY"),
    ("cloudflare-ai-gateway", "CLOUDFLARE_API_KEY"),
    ("xiaomi", "XIAOMI_API_KEY"),
    ("xiaomi-token-plan-cn", "XIAOMI_TOKEN_PLAN_CN_API_KEY"),
    ("xiaomi-token-plan-ams", "XIAOMI_TOKEN_PLAN_AMS_API_KEY"),
    ("xiaomi-token-plan-sgp", "XIAOMI_TOKEN_PLAN_SGP_API_KEY"),
    // pi-rs extension (not in TS): local Ollama auto-discovery reads this as an
    // optional bearer token. See DEVIATIONS.md #4.
    ("ollama", "OLLAMA_API_KEY"),
];

/// All candidate env var names for a provider, in precedence order
/// (match TS `getApiKeyEnvVars`).
///
/// Returns `None` for providers without any API-key env var.
#[must_use]
pub fn get_api_key_env_vars(provider: &str) -> Option<Vec<&'static str>> {
    match provider {
        "github-copilot" => Some(vec!["COPILOT_GITHUB_TOKEN"]),
        // ANTHROPIC_AUTH_TOKEN participates in env discovery/status, but
        // `get_env_api_key` skips it because requests must pass it as a Bearer header.
        "anthropic" => Some(vec![
            ANTHROPIC_AUTH_TOKEN_ENV,
            ANTHROPIC_OAUTH_TOKEN_ENV,
            ANTHROPIC_API_KEY_ENV,
        ]),
        other => ENV_MAP
            .iter()
            .find(|(name, _)| *name == other)
            .map(|(_, var)| vec![*var]),
    }
}

/// Resolve a provider env value from a scoped overlay, then the process env
/// (match TS `getProviderEnvValue`). Empty values are treated as unset.
#[must_use]
pub fn get_provider_env_value(name: &str, env: Option<&ProviderEnv>) -> Option<String> {
    if let Some(value) = env.and_then(|e| e.get(name)).filter(|v| !v.is_empty()) {
        return Some(value.clone());
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Which of a provider's candidate env vars are actually configured, in
/// precedence order (match TS `findEnvKeys`). Returns `None` when none are set.
#[must_use]
pub fn find_env_keys(provider: &str, env: Option<&ProviderEnv>) -> Option<Vec<String>> {
    let env_vars = get_api_key_env_vars(provider)?;
    let found: Vec<String> = env_vars
        .into_iter()
        .filter(|var| get_provider_env_value(var, env).is_some())
        .map(std::string::ToString::to_string)
        .collect();
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// Get the API key for a provider from known environment variables
/// (match TS `getEnvApiKey`).
///
/// Anthropic is special: `ANTHROPIC_AUTH_TOKEN` is skipped because it is a
/// gateway bearer token, not an API key. Providers that authenticate through
/// ambient credentials (AWS, Google ADC) return `None` here until the
/// `"<authenticated>"` sentinel is ported.
#[must_use]
pub fn get_env_api_key(provider: &str, env: Option<&ProviderEnv>) -> Option<String> {
    let env_keys = find_env_keys(provider, env)?;
    let api_key_env = if provider == "anthropic" {
        env_keys
            .iter()
            .find(|key| key.as_str() != ANTHROPIC_AUTH_TOKEN_ENV)?
    } else {
        env_keys.first()?
    };
    get_provider_env_value(api_key_env, env)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn overlay(pairs: &[(&str, &str)]) -> ProviderEnv {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn test_env_var_names_match_ts_catalog() {
        // Regression guard: these names were previously wrong (e.g. GEMINI vs
        // GOOGLE, HF_TOKEN vs HF_API_KEY) and silently broke real API-key auth.
        let cases = [
            ("openai", "OPENAI_API_KEY"),
            ("google", "GEMINI_API_KEY"),
            ("google-vertex", "GOOGLE_CLOUD_API_KEY"),
            ("deepseek", "DEEPSEEK_API_KEY"),
            ("github-copilot", "COPILOT_GITHUB_TOKEN"),
            ("xai", "XAI_API_KEY"),
            ("groq", "GROQ_API_KEY"),
            ("cerebras", "CEREBRAS_API_KEY"),
            ("openrouter", "OPENROUTER_API_KEY"),
            ("huggingface", "HF_TOKEN"),
            ("together", "TOGETHER_API_KEY"),
            ("fireworks", "FIREWORKS_API_KEY"),
            ("vercel-ai-gateway", "AI_GATEWAY_API_KEY"),
            ("zai", "ZAI_API_KEY"),
            ("zai-coding-cn", "ZAI_CODING_CN_API_KEY"),
            ("minimax", "MINIMAX_API_KEY"),
            ("minimax-cn", "MINIMAX_CN_API_KEY"),
            ("moonshotai", "MOONSHOT_API_KEY"),
            ("moonshotai-cn", "MOONSHOT_API_KEY"),
            ("cloudflare-workers-ai", "CLOUDFLARE_API_KEY"),
            ("cloudflare-ai-gateway", "CLOUDFLARE_API_KEY"),
            ("kimi-coding", "KIMI_API_KEY"),
            ("opencode", "OPENCODE_API_KEY"),
            ("opencode-go", "OPENCODE_API_KEY"),
            ("ant-ling", "ANT_LING_API_KEY"),
            ("azure-openai-responses", "AZURE_OPENAI_API_KEY"),
            ("nvidia", "NVIDIA_API_KEY"),
            ("mistral", "MISTRAL_API_KEY"),
            ("baseten", "BASETEN_API_KEY"),
            ("radius", "RADIUS_API_KEY"),
            ("qwen-token-plan", "QWEN_TOKEN_PLAN_API_KEY"),
            ("qwen-token-plan-cn", "QWEN_TOKEN_PLAN_CN_API_KEY"),
            ("qwen-token-plan-individual", "QWEN_TOKEN_PLAN_API_KEY"),
            ("xiaomi", "XIAOMI_API_KEY"),
            ("xiaomi-token-plan-cn", "XIAOMI_TOKEN_PLAN_CN_API_KEY"),
            ("xiaomi-token-plan-ams", "XIAOMI_TOKEN_PLAN_AMS_API_KEY"),
            ("xiaomi-token-plan-sgp", "XIAOMI_TOKEN_PLAN_SGP_API_KEY"),
        ];
        for (provider, expected) in cases {
            let vars = get_api_key_env_vars(provider).unwrap();
            assert_eq!(vars, vec![expected], "provider {provider}");
        }
    }

    #[test]
    fn test_anthropic_env_var_list() {
        assert_eq!(
            get_api_key_env_vars("anthropic").unwrap(),
            vec![
                ANTHROPIC_AUTH_TOKEN_ENV,
                ANTHROPIC_OAUTH_TOKEN_ENV,
                ANTHROPIC_API_KEY_ENV
            ]
        );
    }

    #[test]
    fn test_unknown_provider_has_no_env_vars() {
        assert!(get_api_key_env_vars("nonexistent-provider-xyz").is_none());
        assert!(find_env_keys("nonexistent-provider-xyz", None).is_none());
        assert!(get_env_api_key("nonexistent-provider-xyz", None).is_none());
    }

    #[test]
    fn test_provider_env_overlay_wins_over_process_env() {
        // TS `getProviderEnvValue` prefers the scoped overlay. Using the
        // overlay keeps this test independent of the real process env.
        let env = overlay(&[("OPENAI_API_KEY", "from-overlay")]);
        assert_eq!(
            get_provider_env_value("OPENAI_API_KEY", Some(&env)).as_deref(),
            Some("from-overlay")
        );
    }

    #[test]
    fn test_empty_overlay_value_is_unset() {
        let env = overlay(&[("OPENAI_API_KEY", "")]);
        assert!(get_provider_env_value("__PI_DEFINITELY_UNSET__", Some(&env)).is_none());
    }

    #[test]
    fn test_find_env_keys_returns_all_set_vars_in_order() {
        let env = overlay(&[
            (ANTHROPIC_AUTH_TOKEN_ENV, "bearer"),
            (ANTHROPIC_API_KEY_ENV, "key"),
        ]);
        // OAUTH unset, AUTH + API_KEY set -> both reported, in list order.
        assert_eq!(
            find_env_keys("anthropic", Some(&env)).unwrap(),
            vec![
                ANTHROPIC_AUTH_TOKEN_ENV.to_string(),
                ANTHROPIC_API_KEY_ENV.to_string()
            ]
        );
    }

    #[test]
    fn test_get_env_api_key_skips_anthropic_auth_token() {
        // Only the gateway bearer token is set: it is not an API key, so
        // discovery must not return it (match TS `getEnvApiKey`).
        let auth_only = overlay(&[(ANTHROPIC_AUTH_TOKEN_ENV, "bearer")]);
        assert!(get_env_api_key("anthropic", Some(&auth_only)).is_none());

        // API key set alongside the bearer token: the API key wins.
        let both = overlay(&[
            (ANTHROPIC_AUTH_TOKEN_ENV, "bearer"),
            (ANTHROPIC_API_KEY_ENV, "key"),
        ]);
        assert_eq!(
            get_env_api_key("anthropic", Some(&both)).as_deref(),
            Some("key")
        );
    }

    #[test]
    fn test_get_env_api_key_prefers_anthropic_oauth_over_api_key() {
        let env = overlay(&[
            (ANTHROPIC_OAUTH_TOKEN_ENV, "oauth"),
            (ANTHROPIC_API_KEY_ENV, "key"),
        ]);
        assert_eq!(
            get_env_api_key("anthropic", Some(&env)).as_deref(),
            Some("oauth")
        );
    }

    #[test]
    fn test_get_env_api_key_uses_first_configured_var() {
        let env = overlay(&[("GEMINI_API_KEY", "gemini")]);
        assert_eq!(
            get_env_api_key("google", Some(&env)).as_deref(),
            Some("gemini")
        );
    }
}
