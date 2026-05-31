use crate::types::{Model, ModelCost, OpenAICompletionsCompat, AnthropicMessagesCompat, ModelCompat};
use std::collections::HashMap;

type ModelMap = HashMap<&'static str, Model>;

pub fn models() -> HashMap<&'static str, ModelMap> {
    let mut map: HashMap<&'static str, ModelMap> = HashMap::new();

    // --- anthropic ---
    let mut anthropic = HashMap::new();
    anthropic.insert("claude-sonnet-4-6", Model {
        id: "claude-sonnet-4-6".into(),
        name: "Claude Sonnet 4.6".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url: "https://api.anthropic.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 6.0 },
        context_window: 200000,
        max_tokens: 8192,
        headers: None,
        compat: Some(ModelCompat::AnthropicMessages(AnthropicMessagesCompat {
            supports_eager_tool_input_streaming: Some(true),
            supports_long_cache_retention: Some(true),
            send_session_affinity_headers: Some(true),
            supports_cache_control_on_tools: Some(true),
            force_adaptive_thinking: Some(false),
            allow_empty_signature: Some(false),
        })),
    });
    anthropic.insert("claude-opus-4-7", Model {
        id: "claude-opus-4-7".into(),
        name: "Claude Opus 4.7".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url: "https://api.anthropic.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 15.0, output: 75.0, cache_read: 1.5, cache_write: 30.0 },
        context_window: 200000,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::AnthropicMessages(AnthropicMessagesCompat {
            supports_eager_tool_input_streaming: Some(true),
            supports_long_cache_retention: Some(true),
            send_session_affinity_headers: Some(true),
            supports_cache_control_on_tools: Some(true),
            force_adaptive_thinking: Some(false),
            allow_empty_signature: Some(false),
        })),
    });
    anthropic.insert("claude-haiku-4-5", Model {
        id: "claude-haiku-4-5".into(),
        name: "Claude Haiku 4.5".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url: "https://api.anthropic.com".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 0.8, output: 4.0, cache_read: 0.08, cache_write: 1.6 },
        context_window: 200000,
        max_tokens: 8192,
        headers: None,
        compat: Some(ModelCompat::AnthropicMessages(AnthropicMessagesCompat {
            supports_eager_tool_input_streaming: Some(false),
            supports_long_cache_retention: Some(false),
            send_session_affinity_headers: Some(false),
            supports_cache_control_on_tools: Some(true),
            force_adaptive_thinking: Some(false),
            allow_empty_signature: Some(false),
        })),
    });
    map.insert("anthropic", anthropic);

    // --- openai ---
    let mut openai = HashMap::new();
    openai.insert("gpt-4o", Model {
        id: "gpt-4o".into(),
        name: "GPT-4o".into(),
        api: "openai-completions".into(),
        provider: "openai".into(),
        base_url: "https://api.openai.com".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 2.5, output: 10.0, cache_read: 1.25, cache_write: 0.0 },
        context_window: 128000,
        max_tokens: 16384,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(true),
            supports_reasoning_effort: Some(false),
            supports_usage_in_streaming: Some(true),
            max_tokens_field: Some("max_completion_tokens".into()),
            ..Default::default()
        })),
    });
    openai.insert("gpt-4.1", Model {
        id: "gpt-4.1".into(),
        name: "GPT-4.1".into(),
        api: "openai-completions".into(),
        provider: "openai".into(),
        base_url: "https://api.openai.com".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 2.0, output: 8.0, cache_read: 0.5, cache_write: 0.0 },
        context_window: 1048576,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(true),
            supports_reasoning_effort: Some(false),
            supports_usage_in_streaming: Some(true),
            max_tokens_field: Some("max_completion_tokens".into()),
            ..Default::default()
        })),
    });
    openai.insert("o4-mini", Model {
        id: "o4-mini".into(),
        name: "o4-mini".into(),
        api: "openai-responses".into(),
        provider: "openai".into(),
        base_url: "https://api.openai.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 1.1, output: 4.4, cache_read: 0.275, cache_write: 0.0 },
        context_window: 200000,
        max_tokens: 100000,
        headers: None,
        compat: None,
    });
    map.insert("openai", openai);

    // --- deepseek ---
    let mut deepseek = HashMap::new();
    deepseek.insert("deepseek-chat", Model {
        id: "deepseek-chat".into(),
        name: "DeepSeek Chat".into(),
        api: "openai-completions".into(),
        provider: "deepseek".into(),
        base_url: "https://api.deepseek.com".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.27, output: 1.10, cache_read: 0.07, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 8192,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            thinking_format: Some("deepseek".into()),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    deepseek.insert("deepseek-reasoner", Model {
        id: "deepseek-reasoner".into(),
        name: "DeepSeek Reasoner".into(),
        api: "openai-completions".into(),
        provider: "deepseek".into(),
        base_url: "https://api.deepseek.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.55, output: 2.19, cache_read: 0.14, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            thinking_format: Some("deepseek".into()),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    map.insert("deepseek", deepseek);

    // --- google ---
    let mut google = HashMap::new();
    google.insert("gemini-2.5-flash", Model {
        id: "gemini-2.5-flash".into(),
        name: "Gemini 2.5 Flash".into(),
        api: "google-generative-ai".into(),
        provider: "google".into(),
        base_url: "https://generativelanguage.googleapis.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 0.15, output: 0.6, cache_read: 0.0, cache_write: 0.0 },
        context_window: 1048576,
        max_tokens: 8192,
        headers: None,
        compat: None,
    });
    google.insert("gemini-2.5-pro", Model {
        id: "gemini-2.5-pro".into(),
        name: "Gemini 2.5 Pro".into(),
        api: "google-generative-ai".into(),
        provider: "google".into(),
        base_url: "https://generativelanguage.googleapis.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 1.25, output: 10.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 1048576,
        max_tokens: 65536,
        headers: None,
        compat: None,
    });
    map.insert("google", google);

    // --- groq ---
    let mut groq = HashMap::new();
    groq.insert("llama-4-maverick", Model {
        id: "llama-4-maverick".into(),
        name: "Llama 4 Maverick".into(),
        api: "openai-completions".into(),
        provider: "groq".into(),
        base_url: "https://api.groq.com/openai".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.2, output: 0.6, cache_read: 0.0, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 8192,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            max_tokens_field: Some("max_completion_tokens".into()),
            supports_usage_in_streaming: Some(true),
            ..Default::default()
        })),
    });
    map.insert("groq", groq);

    // --- xai ---
    let mut xai = HashMap::new();
    xai.insert("grok-4", Model {
        id: "grok-4".into(),
        name: "Grok 4".into(),
        api: "openai-completions".into(),
        provider: "xai".into(),
        base_url: "https://api.x.ai".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 2.0, output: 8.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 8192,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            max_tokens_field: Some("max_tokens".into()),
            supports_usage_in_streaming: Some(true),
            ..Default::default()
        })),
    });
    map.insert("xai", xai);

    // --- openrouter ---
    let mut openrouter = HashMap::new();
    openrouter.insert("openai/gpt-4o", Model {
        id: "openai/gpt-4o".into(),
        name: "OpenAI GPT-4o (OpenRouter)".into(),
        api: "openai-completions".into(),
        provider: "openrouter".into(),
        base_url: "https://openrouter.ai/api/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 2.5, output: 10.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 128000,
        max_tokens: 16384,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            thinking_format: Some("openrouter".into()),
            max_tokens_field: Some("max_tokens".into()),
            supports_usage_in_streaming: Some(false),
            ..Default::default()
        })),
    });
    openrouter.insert("anthropic/claude-sonnet-4-6", Model {
        id: "anthropic/claude-sonnet-4-6".into(),
        name: "Anthropic Claude Sonnet 4.6 (OpenRouter)".into(),
        api: "openai-completions".into(),
        provider: "openrouter".into(),
        base_url: "https://openrouter.ai/api/v1".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 3.0, output: 15.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 200000,
        max_tokens: 8192,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            thinking_format: Some("openrouter".into()),
            max_tokens_field: Some("max_tokens".into()),
            supports_usage_in_streaming: Some(false),
            ..Default::default()
        })),
    });
    map.insert("openrouter", openrouter);

    // --- cerebras ---
    let mut cerebras = HashMap::new();
    cerebras.insert("llama3.1-8b", Model {
        id: "llama3.1-8b".into(),
        name: "Llama 3.1 8B".into(),
        api: "openai-completions".into(),
        provider: "cerebras".into(),
        base_url: "https://api.cerebras.ai/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.1, output: 0.1, cache_read: 0.0, cache_write: 0.0 },
        context_window: 32000,
        max_tokens: 8000,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    cerebras.insert("gpt-oss-120b", Model {
        id: "gpt-oss-120b".into(),
        name: "GPT OSS 120B".into(),
        api: "openai-completions".into(),
        provider: "cerebras".into(),
        base_url: "https://api.cerebras.ai/v1".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.25, output: 0.69, cache_read: 0.0, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    map.insert("cerebras", cerebras);

    // --- mistral ---
    let mut mistral = HashMap::new();
    mistral.insert("codestral-latest", Model {
        id: "codestral-latest".into(),
        name: "Codestral (latest)".into(),
        api: "mistral-conversations".into(),
        provider: "mistral".into(),
        base_url: "https://api.mistral.ai".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.2, output: 0.6, cache_read: 0.0, cache_write: 0.0 },
        context_window: 256000,
        max_tokens: 8192,
        headers: None,
        compat: None,
    });
    mistral.insert("mistral-large-latest", Model {
        id: "mistral-large-latest".into(),
        name: "Mistral Large (latest)".into(),
        api: "mistral-conversations".into(),
        provider: "mistral".into(),
        base_url: "https://api.mistral.ai".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 2.0, output: 6.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 256000,
        max_tokens: 32768,
        headers: None,
        compat: None,
    });
    map.insert("mistral", mistral);

    // --- together ---
    let mut together = HashMap::new();
    together.insert("deepseek-ai/DeepSeek-R1", Model {
        id: "deepseek-ai/DeepSeek-R1".into(),
        name: "DeepSeek R1 (Together)".into(),
        api: "openai-completions".into(),
        provider: "together".into(),
        base_url: "https://api.together.xyz".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 7.0, output: 7.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            thinking_format: Some("together".into()),
            ..Default::default()
        })),
    });
    together.insert("meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8", Model {
        id: "meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8".into(),
        name: "Llama 4 Maverick".into(),
        api: "openai-completions".into(),
        provider: "together".into(),
        base_url: "https://api.together.xyz".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost { input: 0.2, output: 0.6, cache_read: 0.0, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 8192,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            thinking_format: Some("together".into()),
            ..Default::default()
        })),
    });
    map.insert("together", together);

    // --- fireworks ---
    let mut fireworks = HashMap::new();
    fireworks.insert("accounts/fireworks/models/deepseek-r1", Model {
        id: "accounts/fireworks/models/deepseek-r1".into(),
        name: "DeepSeek R1 (Fireworks)".into(),
        api: "openai-completions".into(),
        provider: "fireworks".into(),
        base_url: "https://api.fireworks.ai/inference".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 8.0, output: 8.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 131072,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    map.insert("fireworks", fireworks);

    // --- minimax ---
    let mut minimax = HashMap::new();
    minimax.insert("MiniMax-M1", Model {
        id: "MiniMax-M1".into(),
        name: "MiniMax M1".into(),
        api: "openai-completions".into(),
        provider: "minimax".into(),
        base_url: "https://api.minimax.chat/v1".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.4, output: 1.6, cache_read: 0.0, cache_write: 0.0 },
        context_window: 1048576,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    map.insert("minimax", minimax);

    // --- moonshotai ---
    let mut moonshotai = HashMap::new();
    moonshotai.insert("kimi-k2", Model {
        id: "kimi-k2".into(),
        name: "Kimi K2".into(),
        api: "openai-completions".into(),
        provider: "moonshotai".into(),
        base_url: "https://api.moonshot.cn/v1".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.5, output: 1.5, cache_read: 0.0, cache_write: 0.0 },
        context_window: 1048576,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    map.insert("moonshotai", moonshotai);

    // --- kimi-coding ---
    let mut kimi_coding = HashMap::new();
    kimi_coding.insert("kimi-coding", Model {
        id: "kimi-coding".into(),
        name: "Kimi Coding".into(),
        api: "openai-completions".into(),
        provider: "kimi-coding".into(),
        base_url: "https://api.kimi.com/v1".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.5, output: 2.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 262144,
        max_tokens: 32768,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    map.insert("kimi-coding", kimi_coding);

    // --- cloudflare ---
    let mut cloudflare = HashMap::new();
    cloudflare.insert("@cf/meta/llama-4-scout-17b-16e-instruct", Model {
        id: "@cf/meta/llama-4-scout-17b-16e-instruct".into(),
        name: "Llama 4 Scout".into(),
        api: "openai-completions".into(),
        provider: "cloudflare-workers-ai".into(),
        base_url: "https://api.cloudflare.com/client/v4".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost { input: 0.0, output: 0.0, cache_read: 0.0, cache_write: 0.0 },
        context_window: 8192,
        max_tokens: 4096,
        headers: None,
        compat: Some(ModelCompat::OpenAICompletions(OpenAICompletionsCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_usage_in_streaming: Some(false),
            max_tokens_field: Some("max_tokens".into()),
            ..Default::default()
        })),
    });
    map.insert("cloudflare-workers-ai", cloudflare);

    map
}

// Default impl for OpenAICompletionsCompat
impl Default for OpenAICompletionsCompat {
    fn default() -> Self {
        Self {
            supports_store: None,
            supports_developer_role: None,
            supports_reasoning_effort: None,
            supports_usage_in_streaming: None,
            max_tokens_field: None,
            requires_tool_result_name: None,
            requires_assistant_after_tool_result: None,
            requires_thinking_as_text: None,
            requires_reasoning_content_on_assistant_messages: None,
            thinking_format: None,
            open_router_routing: None,
            vercel_gateway_routing: None,
            zai_tool_stream: None,
            supports_strict_mode: None,
            cache_control_format: None,
            send_session_affinity_headers: None,
            supports_long_cache_retention: None,
        }
    }
}
