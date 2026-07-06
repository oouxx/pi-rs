use std::collections::HashMap;
use std::sync::LazyLock;

pub static BUILT_IN_PROVIDER_DISPLAY_NAMES: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert("anthropic", "Anthropic");
        m.insert("amazon-bedrock", "Amazon Bedrock");
        m.insert("ant-ling", "Ant Ling");
        m.insert("azure-openai-responses", "Azure OpenAI Responses");
        m.insert("cerebras", "Cerebras");
        m.insert("cloudflare-ai-gateway", "Cloudflare AI Gateway");
        m.insert("cloudflare-workers-ai", "Cloudflare Workers AI");
        m.insert("deepseek", "DeepSeek");
        m.insert("fireworks", "Fireworks");
        m.insert("google", "Google Gemini");
        m.insert("google-vertex", "Google Vertex AI");
        m.insert("groq", "Groq");
        m.insert("huggingface", "Hugging Face");
        m.insert("kimi-coding", "Kimi For Coding");
        m.insert("mistral", "Mistral");
        m.insert("minimax", "MiniMax");
        m.insert("minimax-cn", "MiniMax (China)");
        m.insert("moonshotai", "Moonshot AI");
        m.insert("moonshotai-cn", "Moonshot AI (China)");
        m.insert("nvidia", "NVIDIA NIM");
        m.insert("opencode", "OpenCode Zen");
        m.insert("opencode-go", "OpenCode Go");
        m.insert("openai", "OpenAI");
        m.insert("openrouter", "OpenRouter");
        m.insert("together", "Together AI");
        m.insert("vercel-ai-gateway", "Vercel AI Gateway");
        m.insert("xai", "xAI");
        m.insert("zai", "ZAI");
        m.insert("zai-coding-cn", "ZAI Coding Plan (China)");
        m.insert("xiaomi", "Xiaomi MiMo");
        m.insert("xiaomi-token-plan-cn", "Xiaomi MiMo Token Plan (China)");
        m.insert(
            "xiaomi-token-plan-ams",
            "Xiaomi MiMo Token Plan (Amsterdam)",
        );
        m.insert(
            "xiaomi-token-plan-sgp",
            "Xiaomi MiMo Token Plan (Singapore)",
        );
        m
    });

pub fn get_provider_display_name(provider: &str) -> Option<&'static str> {
    BUILT_IN_PROVIDER_DISPLAY_NAMES.get(provider).copied()
}
