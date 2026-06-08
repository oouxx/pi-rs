//! Build-time model generation — ports pi's generate-models.ts and generate-image-models.ts.
//!
//! At compile time, fetches available models from OpenRouter and models.dev APIs,
//! processes them, and generates JSON that the main crate embeds at compile time.

use std::collections::HashMap;

#[derive(serde::Serialize, serde::Deserialize)]
struct OpenRouterModelRecord {
    id: String,
    name: String,
    #[serde(default)]
    context_length: Option<u64>,
    top_provider: Option<OpenRouterTopProvider>,
    #[serde(default)]
    pricing: Option<OpenRouterPricing>,
    #[serde(default)]
    architecture: Option<OpenRouterArchitecture>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OpenRouterTopProvider {
    #[serde(default)]
    max_completion_tokens: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OpenRouterPricing {
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    input_cache_read: Option<String>,
    #[serde(default)]
    input_cache_write: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    modality: Option<String>,
    #[serde(default)]
    input_modalities: Option<Vec<String>>,
    #[serde(default)]
    output_modalities: Option<Vec<String>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OpenRouterResponse {
    data: Option<Vec<OpenRouterModelRecord>>,
}

// JSON output shape (matches the Model struct serde attributes)
#[derive(serde::Serialize, serde::Deserialize)]
struct BuildModel {
    id: String,
    name: String,
    api: String,
    provider: String,
    #[serde(rename = "baseUrl")]
    base_url: String,
    reasoning: bool,
    input: Vec<String>,
    cost: BuildModelCost,
    #[serde(rename = "contextWindow")]
    context_window: u64,
    #[serde(rename = "maxTokens")]
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    compat: Option<BuildModelCompat>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct BuildModelCost {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    #[serde(rename = "cacheRead")]
    cache_read: f64,
    #[serde(default)]
    #[serde(rename = "cacheWrite")]
    cache_write: f64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum BuildModelCompat {
    OpenAICompletions {
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "maxTokensField")]
        max_tokens_field: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "thinkingFormat")]
        thinking_format: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "supportsUsageInStreaming")]
        supports_usage_in_streaming: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "supportsStore")]
        supports_store: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "supportsReasoningEffort")]
        supports_reasoning_effort: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "requiresAssistantAfterToolResult")]
        requires_assistant_after_tool_result: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "requiresReasoningContentOnAssistantMessages")]
        requires_reasoning_content_on_assistant_messages: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "requiresThinkingAsText")]
        requires_thinking_as_text: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "requiresToolResultName")]
        requires_tool_result_name: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "supportsDeveloperRole")]
        supports_developer_role: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "supportsStrictMode")]
        supports_strict_mode: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "cacheControlFormat")]
        cache_control_format: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "sendSessionAffinityHeaders")]
        send_session_affinity_headers: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "supportsLongCacheRetention")]
        supports_long_cache_retention: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(rename = "zaiToolStream")]
        zai_tool_stream: Option<bool>,
    },
}

fn parse_price(s: &Option<String>) -> f64 {
    s.as_ref()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn fetch_openrouter_models(client: &reqwest::blocking::Client) -> Vec<OpenRouterModelRecord> {
    match client.get("https://openrouter.ai/api/v1/models").send() {
        Ok(resp) if resp.status().is_success() => resp
            .json::<OpenRouterResponse>()
            .ok()
            .and_then(|d| d.data)
            .unwrap_or_default(),
        Ok(resp) => {
            println!(
                "cargo:warning=OpenRouter API returned status {}",
                resp.status()
            );
            vec![]
        }
        Err(e) => {
            println!("cargo:warning=Failed to fetch OpenRouter models: {}", e);
            vec![]
        }
    }
}

fn fetch_models_dev(client: &reqwest::blocking::Client) -> serde_json::Value {
    match client.get("https://models.dev/api.json").send() {
        Ok(resp) if resp.status().is_success() => {
            let text = resp.text().unwrap_or_default();
            serde_json::from_str(&text).unwrap_or_else(|e| {
                println!("cargo:warning=models.dev JSON parse error: {}", e);
                serde_json::Value::Null
            })
        }
        Ok(resp) => {
            println!(
                "cargo:warning=models.dev API returned status {}",
                resp.status()
            );
            serde_json::Value::Null
        }
        Err(e) => {
            println!("cargo:warning=Failed to fetch models.dev: {}", e);
            serde_json::Value::Null
        }
    }
}

fn get_input_modalities(model: &serde_json::Value) -> Vec<String> {
    let mut inputs = vec!["text".to_string()];
    if let Some(modalities) = model
        .get("modalities")
        .and_then(|m| m.get("input"))
        .and_then(|a| a.as_array())
    {
        if modalities.iter().any(|m| m.as_str() == Some("image")) {
            inputs.push("image".to_string());
        }
    }
    inputs
}

fn get_cost(model: &serde_json::Value) -> BuildModelCost {
    let cost = model.get("cost");
    BuildModelCost {
        input: cost
            .and_then(|c| c.get("input"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        output: cost
            .and_then(|c| c.get("output"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        cache_read: cost
            .and_then(|c| c.get("cache_read"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        cache_write: cost
            .and_then(|c| c.get("cache_write"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
    }
}

fn process_openrouter_models(raw: Vec<OpenRouterModelRecord>) -> Vec<BuildModel> {
    raw.into_iter()
        .filter(|m| {
            m.supported_parameters
                .as_ref()
                .map(|p| p.iter().any(|p| p == "tools"))
                .unwrap_or(false)
        })
        .map(|m| {
            let reasoning = m
                .supported_parameters
                .as_ref()
                .map(|p| p.iter().any(|p| p == "reasoning"))
                .unwrap_or(false);
            let input_modalities =
                m.architecture
                    .as_ref()
                    .map_or(vec!["text".to_string()], |arch| {
                        let mut inputs = vec!["text".to_string()];
                        if arch.modality.as_deref() == Some("image")
                            || arch
                                .input_modalities
                                .as_ref()
                                .map_or(false, |m| m.contains(&"image".to_string()))
                        {
                            inputs.push("image".to_string());
                        }
                        inputs
                    });
            BuildModel {
                id: m.id.clone(),
                name: m.name,
                api: "openai-completions".into(),
                provider: "openrouter".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                reasoning,
                input: input_modalities,
                cost: BuildModelCost {
                    input: parse_price(&m.pricing.as_ref().and_then(|p| p.prompt.clone()))
                        * 1_000_000.0,
                    output: parse_price(&m.pricing.as_ref().and_then(|p| p.completion.clone()))
                        * 1_000_000.0,
                    cache_read: parse_price(
                        &m.pricing.as_ref().and_then(|p| p.input_cache_read.clone()),
                    ) * 1_000_000.0,
                    cache_write: parse_price(
                        &m.pricing.as_ref().and_then(|p| p.input_cache_write.clone()),
                    ) * 1_000_000.0,
                },
                context_window: m.context_length.unwrap_or(4096),
                max_tokens: m
                    .top_provider
                    .and_then(|t| t.max_completion_tokens)
                    .unwrap_or(4096),
                compat: Some(BuildModelCompat::OpenAICompletions {
                    max_tokens_field: Some("max_tokens".into()),
                    thinking_format: Some("openrouter".into()),
                    supports_usage_in_streaming: Some(false),
                    supports_store: None,
                    supports_reasoning_effort: None,
                    requires_assistant_after_tool_result: None,
                    requires_reasoning_content_on_assistant_messages: None,
                    requires_thinking_as_text: None,
                    requires_tool_result_name: None,
                    supports_developer_role: None,
                    supports_strict_mode: None,
                    cache_control_format: None,
                    send_session_affinity_headers: None,
                    supports_long_cache_retention: None,
                    zai_tool_stream: None,
                }),
            }
        })
        .collect()
}

fn process_models_dev(data: &serde_json::Value) -> Vec<BuildModel> {
    let mut models = Vec::new();
    let data = match data.as_object() {
        Some(d) => d,
        None => return models,
    };

    for (provider_name, api, base_url, include_deprecated) in [
        (
            "anthropic",
            "anthropic-messages",
            "https://api.anthropic.com",
            true,
        ),
        (
            "openai",
            "openai-completions",
            "https://api.openai.com/v1",
            true,
        ),
        (
            "google",
            "google-generative-ai",
            "https://generativelanguage.googleapis.com/v1beta",
            true,
        ),
        (
            "deepseek",
            "openai-completions",
            "https://api.deepseek.com",
            true,
        ),
        (
            "groq",
            "openai-completions",
            "https://api.groq.com/openai/v1",
            true,
        ),
        (
            "cerebras",
            "openai-completions",
            "https://api.cerebras.ai/v1",
            true,
        ),
        ("xai", "openai-completions", "https://api.x.ai/v1", true),
        (
            "mistral",
            "mistral-conversations",
            "https://api.mistral.ai",
            true,
        ),
        (
            "together",
            "openai-completions",
            "https://api.together.xyz/v1",
            false,
        ),
        (
            "fireworks-ai",
            "anthropic-messages",
            "https://api.fireworks.ai/inference",
            true,
        ),
        (
            "github-copilot",
            "openai-completions",
            "https://api.individual.githubcopilot.com",
            true,
        ),
        (
            "minimax",
            "openai-completions",
            "https://api.minimax.chat/v1",
            true,
        ),
        (
            "minimax-cn",
            "openai-completions",
            "https://api.minimax.chat/v1",
            true,
        ),
    ] {
        if let Some(items) = data
            .get(provider_name)
            .and_then(|d| d.get("models"))
            .and_then(|m| m.as_object())
        {
            for (id, m) in items {
                let tool_call = m
                    .get("tool_call")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !tool_call {
                    continue;
                }
                if !include_deprecated
                    && m.get("status").and_then(|v| v.as_str()) == Some("deprecated")
                {
                    continue;
                }
                let effective_provider = if provider_name == "together" {
                    "together"
                } else {
                    provider_name
                };
                models.push(BuildModel {
                    id: id.clone(),
                    name: m
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(id)
                        .to_string(),
                    api: api.into(),
                    provider: effective_provider.into(),
                    base_url: base_url.into(),
                    reasoning: m
                        .get("reasoning")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    input: get_input_modalities(m),
                    cost: get_cost(m),
                    context_window: m
                        .get("limit")
                        .and_then(|l| l.get("context"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(4096),
                    max_tokens: m
                        .get("limit")
                        .and_then(|l| l.get("output"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(4096),
                    compat: None,
                });
            }
        }
    }

    models
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            println!("cargo:warning=HTTP client failed: {}", e);
            return;
        }
    };

    let openrouter_raw = fetch_openrouter_models(&client);
    let openrouter_models = process_openrouter_models(openrouter_raw);
    println!(
        "cargo:warning=OpenRouter: {} tool-capable models",
        openrouter_models.len()
    );

    let models_dev = fetch_models_dev(&client);
    let models_dev_models = if models_dev.is_object() {
        process_models_dev(&models_dev)
    } else {
        Vec::new()
    };
    println!(
        "cargo:warning=models.dev: {} models",
        models_dev_models.len()
    );

    let mut all_models = models_dev_models;
    for m in openrouter_models {
        if !all_models
            .iter()
            .any(|e| e.provider == m.provider && e.id == m.id)
        {
            all_models.push(m);
        }
    }

    let mut by_provider: HashMap<String, HashMap<String, &BuildModel>> = HashMap::new();
    for model in &all_models {
        by_provider
            .entry(model.provider.clone())
            .or_default()
            .insert(model.id.clone(), model);
    }

    let json = serde_json::to_string_pretty(&by_provider).unwrap_or_else(|e| {
        println!("cargo:warning=Serialize failed: {}", e);
        String::new()
    });
    std::fs::write(format!("{}/models_generated.json", out_dir), &json)
        .unwrap_or_else(|e| println!("cargo:warning=Write failed: {}", e));
    println!(
        "cargo:warning=Generated {} models across {} providers",
        all_models.len(),
        by_provider.len()
    );
}
