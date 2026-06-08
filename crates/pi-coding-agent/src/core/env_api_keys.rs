use std::collections::HashMap;
use std::sync::LazyLock;

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

static ENV_KEYS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(provider_env_keys);

pub fn get_env_api_key(provider: &str) -> Option<String> {
    let var_name = ENV_KEYS.get(provider)?;
    std::env::var(var_name).ok()
}

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
}
