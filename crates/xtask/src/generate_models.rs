//! 抓取 OpenRouter / models.dev 的模型列表，生成 `models_generated.json`。
//!
//! 对应原版 `packages/ai/scripts/generate-models.ts` 的"抓取 + 转换"部分。
//! 与原版差异：当前只覆盖原版的子集（详见 `DEVIATIONS.md`），且产物
//! 入仓由维护者手动跑本工具，而非每次编译期联网。

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result};

// ---- OpenRouter 响应结构 ----------------------------------------------------

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

// ---- 产物结构（字段 serde 属性与 pi-ai 的 `Model` 对齐） ---------------------

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

// ---- 抓取 -------------------------------------------------------------------

#[allow(clippy::ref_option)]
fn parse_price(s: &Option<String>) -> f64 {
    s.as_ref()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn fetch_openrouter_models(client: &reqwest::blocking::Client) -> Result<Vec<OpenRouterModelRecord>> {
    let resp = client
        .get("https://openrouter.ai/api/v1/models")
        .send()
        .context("GET https://openrouter.ai/api/v1/models")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("OpenRouter API returned status {status}");
    }
    let body: OpenRouterResponse = resp
        .json()
        .context("parse OpenRouter response")?;
    Ok(body.data.unwrap_or_default())
}

fn fetch_models_dev(client: &reqwest::blocking::Client) -> Result<serde_json::Value> {
    let resp = client
        .get("https://models.dev/api.json")
        .send()
        .context("GET https://models.dev/api.json")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("models.dev API returned status {status}");
    }
    let text = resp.text().context("read models.dev body")?;
    let value: serde_json::Value =
        serde_json::from_str(&text).context("parse models.dev JSON")?;
    Ok(value)
}

// ---- 转换 -------------------------------------------------------------------

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
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        output: cost
            .and_then(|c| c.get("output"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        cache_read: cost
            .and_then(|c| c.get("cache_read"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
        cache_write: cost
            .and_then(|c| c.get("cache_write"))
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0),
    }
}

fn process_openrouter_models(raw: Vec<OpenRouterModelRecord>) -> Vec<BuildModel> {
    raw.into_iter()
        .filter(|m| {
            m.supported_parameters
                .as_ref()
                .is_some_and(|p| p.iter().any(|p| p == "tools"))
        })
        .map(|m| {
            let reasoning = m
                .supported_parameters
                .as_ref()
                .is_some_and(|p| p.iter().any(|p| p == "reasoning"));
            let input_modalities =
                m.architecture
                    .as_ref()
                    .map_or_else(|| vec!["text".to_string()], |arch| {
                        let mut inputs = vec!["text".to_string()];
                        if arch.modality.as_deref() == Some("image")
                            || arch
                                .input_modalities
                                .as_ref()
                                .is_some_and(|m| m.contains(&"image".to_string()))
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
    let Some(data) = data.as_object() else {
        return models;
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
            "openai-responses",
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
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if !tool_call {
                    continue;
                }
                if !include_deprecated
                    && m.get("status").and_then(serde_json::Value::as_str) == Some("deprecated")
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
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    api: api.into(),
                    provider: effective_provider.into(),
                    base_url: base_url.into(),
                    reasoning: m
                        .get("reasoning")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    input: get_input_modalities(m),
                    cost: get_cost(m),
                    context_window: m
                        .get("limit")
                        .and_then(|l| l.get("context"))
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(4096),
                    max_tokens: m
                        .get("limit")
                        .and_then(|l| l.get("output"))
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(4096),
                    compat: None,
                });
            }
        }
    }

    models
}

// ---- 入口 -------------------------------------------------------------------

pub fn run(out: &std::path::Path, check_only: bool) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("build HTTP client")?;

    let openrouter_raw = fetch_openrouter_models(&client)?;
    let openrouter_models = process_openrouter_models(openrouter_raw);
    eprintln!("OpenRouter: {} tool-capable models", openrouter_models.len());

    let models_dev = fetch_models_dev(&client)?;
    let models_dev_models = if models_dev.is_object() {
        process_models_dev(&models_dev)
    } else {
        Vec::new()
    };
    eprintln!("models.dev: {} models", models_dev_models.len());

    // models.dev 优先于 OpenRouter（同 provider+id 去重，保留先出现的）
    let mut all_models = models_dev_models;
    for m in openrouter_models {
        if !all_models
            .iter()
            .any(|e| e.provider == m.provider && e.id == m.id)
        {
            all_models.push(m);
        }
    }

    // 按 (provider, id) 排序，保证两次运行字节级一致，可复现、可 diff
    all_models.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));

    let mut by_provider: BTreeMap<String, BTreeMap<String, &BuildModel>> = BTreeMap::new();
    for model in &all_models {
        by_provider
            .entry(model.provider.clone())
            .or_default()
            .insert(model.id.clone(), model);
    }

    let json = serde_json::to_string_pretty(&by_provider).context("serialize models")?;

    if check_only {
        let existing = std::fs::read_to_string(out).with_context(|| {
            format!(
                "产物不存在，请先跑 `cargo run -p xtask -- generate-models` 生成: {}",
                out.display()
            )
        })?;
        if existing != json {
            std::fs::write(format!("{}.new", out.display()), &json)?;
            anyhow::bail!(
                "models_generated.json 已过期（新内容已写入 {0}.new 供对照），请跑 \
                 `cargo run -p xtask -- generate-models` 刷新后提交",
                out.display()
            );
        }
        eprintln!("OK: {} 与现场抓取一致（{} models, {} providers）", out.display(), all_models.len(), by_provider.len());
        return Ok(());
    }

    std::fs::write(out, &json).with_context(|| format!("write {}", out.display()))?;
    eprintln!(
        "已生成 {}（{} models, {} providers）",
        out.display(),
        all_models.len(),
        by_provider.len()
    );
    Ok(())
}
