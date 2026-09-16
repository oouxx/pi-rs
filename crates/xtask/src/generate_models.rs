//! 抓取 OpenRouter / models.dev / Vercel AI Gateway / NVIDIA NIM 的模型列表，
//! 生成 `models_generated.json`。
//!
//! 对应原版 `packages/ai/scripts/generate-models.ts` 的「抓取 + 转换」部分。
//! provider 列表与原版一致；深层元数据（`thinkingLevelMap` 由
//! `reasoning_options` 推导、Anthropic compat、手工定价 tiers、OpenAI 长上下文
//! 定价覆盖、OpenAI/DeepSeek/Ant-Ling/Codex/Azure 的手工补充模型）仍为已登记
//! 差距（`DEVIATIONS.md` #2）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    context_length: Option<u64>,
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
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OpenRouterResponse {
    data: Option<Vec<OpenRouterModelRecord>>,
}

// ---- AI Gateway / NVIDIA 响应结构 -------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct AiGatewayPricing {
    #[serde(default)]
    input: Option<serde_json::Value>,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    input_cache_read: Option<serde_json::Value>,
    #[serde(default)]
    input_cache_write: Option<serde_json::Value>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AiGatewayModelRecord {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    pricing: Option<AiGatewayPricing>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_tokens: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AiGatewayResponse {
    #[serde(default)]
    data: Option<Vec<AiGatewayModelRecord>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NvidiaModelRecord {
    id: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NvidiaResponse {
    #[serde(default)]
    data: Option<Vec<NvidiaModelRecord>>,
}

// ---- 产物结构（字段 serde 属性与 pi-ai 的 `Model` 对齐） ---------------------

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct BuildModel {
    id: String,
    name: String,
    api: String,
    provider: String,
    #[serde(rename = "baseUrl")]
    base_url: String,
    reasoning: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "thinkingLevelMap")]
    thinking_level_map: Option<serde_json::Value>,
    input: Vec<String>,
    cost: BuildModelCost,
    #[serde(rename = "contextWindow")]
    context_window: u64,
    #[serde(rename = "maxTokens")]
    max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    headers: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compat: Option<BuildModelCompat>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
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

/// Subset of TS `OpenAICompletionsCompat` that pi-rs models.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct BuildModelCompat {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_usage_in_streaming: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_store: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_reasoning_effort: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_assistant_after_tool_result: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_reasoning_content_on_assistant_messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_thinking_as_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_tool_result_name: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_developer_role: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_strict_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    send_session_affinity_headers: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supports_long_cache_retention: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    zai_tool_stream: Option<bool>,
}

impl BuildModelCompat {
    fn new() -> Self {
        Self::default()
    }
    fn max_tokens_field(mut self, value: &str) -> Self {
        self.max_tokens_field = Some(value.to_string());
        self
    }
    fn thinking_format(mut self, value: &str) -> Self {
        self.thinking_format = Some(value.to_string());
        self
    }
    fn developer_role(mut self, value: bool) -> Self {
        self.supports_developer_role = Some(value);
        self
    }
    fn store(mut self, value: bool) -> Self {
        self.supports_store = Some(value);
        self
    }
    fn reasoning_effort(mut self, value: bool) -> Self {
        self.supports_reasoning_effort = Some(value);
        self
    }
    fn strict_mode(mut self, value: bool) -> Self {
        self.supports_strict_mode = Some(value);
        self
    }
    fn long_cache_retention(mut self, value: bool) -> Self {
        self.supports_long_cache_retention = Some(value);
        self
    }
    fn session_affinity(mut self, value: bool) -> Self {
        self.send_session_affinity_headers = Some(value);
        self
    }
    fn requires_reasoning_content(mut self, value: bool) -> Self {
        self.requires_reasoning_content_on_assistant_messages = Some(value);
        self
    }
    fn zai_tool_stream(mut self, value: bool) -> Self {
        self.zai_tool_stream = Some(value);
        self
    }
}

// ---- 基础 URL / header 常量（对齐原版） -------------------------------------

const TOGETHER_BASE_URL: &str = "https://api.together.ai/v1";
const VERTEX_BASE_URL: &str = "https://{location}-aiplatform.googleapis.com";
const NVIDIA_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const AI_GATEWAY_MODELS_URL: &str = "https://ai-gateway.vercel.sh/v1";
const AI_GATEWAY_BASE_URL: &str = "https://ai-gateway.vercel.sh";
const CLOUDFLARE_WORKERS_AI_BASE_URL: &str =
    "https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai/v1";
const CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/compat";
const CLOUDFLARE_AI_GATEWAY_OPENAI_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/openai";
const CLOUDFLARE_AI_GATEWAY_ANTHROPIC_BASE_URL: &str =
    "https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/anthropic";

const NVIDIA_HEADER_POLL_SECONDS: (&str, &str) = ("NVCF-POLL-SECONDS", "3600");

const COPILOT_HEADERS: &[(&str, &str)] = &[
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];

const QWEN_TOKEN_PLAN_INDIVIDUAL_MODEL_IDS: &[&str] = &[
    "deepseek-v4-flash-0731",
    "deepseek-v4-pro",
    "deepseek-v4-pro-0813",
    "glm-5.2",
    "qwen3.6-flash",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.8-flash",
    "qwen3.8-max",
];

const ZAI_TOOL_STREAM_UNSUPPORTED_MODELS: &[&str] =
    &["glm-4.5", "glm-4.5-air", "glm-4.5-flash", "glm-4.5v"];

const NVIDIA_NIM_UNSUPPORTED_MODELS: &[&str] = &[
    "abacusai/dracarys-llama-3.1-70b-instruct",
    "bytedance/seed-oss-36b-instruct",
    "deepseek-ai/deepseek-v4-flash",
    "deepseek-ai/deepseek-v4-pro",
    "google/gemma-2-2b-it",
    "google/gemma-3n-e2b-it",
    "google/gemma-3n-e4b-it",
    "google/gemma-4-31b-it",
    "meta/llama-3.2-1b-instruct",
    "meta/llama-4-maverick-17b-128e-instruct",
    "microsoft/phi-4-mini-instruct",
    "minimaxai/minimax-m2.7",
    "mistralai/mistral-nemotron",
    "nvidia/nemotron-mini-4b-instruct",
    "qwen/qwen3-next-80b-a3b-instruct",
    "qwen/qwen3.5-397b-a17b",
    "sarvamai/sarvam-m",
    "upstage/solar-10.7b-instruct",
];

// ---- 抓取 -------------------------------------------------------------------

#[allow(clippy::ref_option)]
fn parse_price(s: &Option<String>) -> f64 {
    s.as_ref()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn round_cost(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
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
    let body: OpenRouterResponse = resp.json().context("parse OpenRouter response")?;
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
    let value: serde_json::Value = serde_json::from_str(&text).context("parse models.dev JSON")?;
    Ok(value)
}

fn fetch_ai_gateway_models(client: &reqwest::blocking::Client) -> Result<Vec<BuildModel>> {
    let resp = client
        .get(format!("{AI_GATEWAY_MODELS_URL}/models"))
        .send()
        .context("GET Vercel AI Gateway models")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("Vercel AI Gateway API returned status {status}");
    }
    let body: AiGatewayResponse = resp.json().context("parse AI Gateway response")?;
    let mut models = Vec::new();
    for model in body.data.unwrap_or_default() {
        let tags = model.tags.unwrap_or_default();
        if !tags.iter().any(|t| t == "tool-use") {
            continue;
        }
        let mut input = vec!["text".to_string()];
        if tags.iter().any(|t| t == "vision") {
            input.push("image".to_string());
        }
        let num = |v: &Option<serde_json::Value>| -> f64 {
            match v {
                Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
                Some(serde_json::Value::String(s)) => s.parse().unwrap_or(0.0),
                _ => 0.0,
            }
        };
        let pricing = model.pricing.unwrap_or(AiGatewayPricing {
            input: None,
            output: None,
            input_cache_read: None,
            input_cache_write: None,
        });
        models.push(BuildModel {
            id: model.id.clone(),
            name: model.name.unwrap_or_else(|| model.id.clone()),
            api: "anthropic-messages".into(),
            provider: "vercel-ai-gateway".into(),
            base_url: AI_GATEWAY_BASE_URL.into(),
            reasoning: tags.iter().any(|t| t == "reasoning"),
            thinking_level_map: None,
            input,
            cost: BuildModelCost {
                input: round_cost(num(&pricing.input) * 1_000_000.0),
                output: round_cost(num(&pricing.output) * 1_000_000.0),
                cache_read: round_cost(num(&pricing.input_cache_read) * 1_000_000.0),
                cache_write: round_cost(num(&pricing.input_cache_write) * 1_000_000.0),
            },
            context_window: model.context_window.unwrap_or(4096),
            max_tokens: model.max_tokens.unwrap_or(4096),
            headers: None,
            compat: None,
        });
    }
    Ok(models)
}

/// Live NVIDIA NIM model ids → `normalize(id) → live_id` (match TS
/// `fetchNvidiaNimModelIds`).
fn fetch_nvidia_nim_ids(client: &reqwest::blocking::Client) -> Result<HashMap<String, String>> {
    let resp = client
        .get(format!("{NVIDIA_BASE_URL}/models"))
        .send()
        .context("GET NVIDIA NIM models")?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("NVIDIA NIM API returned status {status}");
    }
    let body: NvidiaResponse = resp.json().context("parse NVIDIA NIM response")?;
    let mut ids = HashMap::new();
    for model in body.data.unwrap_or_default() {
        ids.insert(model.id.clone(), model.id.clone());
        ids.insert(normalize_nvidia_model_id(&model.id), model.id);
    }
    Ok(ids)
}

// ---- models.dev 转换辅助 ----------------------------------------------------

fn normalize_nvidia_model_id(model_id: &str) -> String {
    model_id.to_lowercase().replace('_', ".")
}

fn get_bedrock_base_url(model_id: &str) -> String {
    if model_id.starts_with("eu.") {
        "https://bedrock-runtime.eu-central-1.amazonaws.com".into()
    } else {
        "https://bedrock-runtime.us-east-1.amazonaws.com".into()
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

fn is_tool_call(model: &serde_json::Value) -> bool {
    model
        .get("tool_call")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn is_deprecated(model: &serde_json::Value) -> bool {
    model.get("status").and_then(serde_json::Value::as_str) == Some("deprecated")
}

fn model_name(model: &serde_json::Value, id: &str) -> String {
    model
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(id)
        .to_string()
}

fn model_reasoning(model: &serde_json::Value) -> bool {
    model
        .get("reasoning")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn model_context_window(model: &serde_json::Value) -> u64 {
    model
        .get("limit")
        .and_then(|l| l.get("context"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(4096)
}

fn model_max_tokens(model: &serde_json::Value) -> u64 {
    model
        .get("limit")
        .and_then(|l| l.get("output"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(4096)
}

fn models_object<'a>(
    data: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    data.get(key)?.get("models")?.as_object()
}

/// Build one catalog entry from a models.dev model record.
#[allow(clippy::too_many_arguments)]
fn build_model(
    id: &str,
    model: &serde_json::Value,
    api: &str,
    provider: &str,
    base_url: &str,
    compat: Option<BuildModelCompat>,
) -> BuildModel {
    BuildModel {
        id: id.to_string(),
        name: model_name(model, id),
        api: api.to_string(),
        provider: provider.to_string(),
        base_url: base_url.to_string(),
        reasoning: model_reasoning(model),
        thinking_level_map: None,
        input: get_input_modalities(model),
        cost: get_cost(model),
        context_window: model_context_window(model),
        max_tokens: model_max_tokens(model),
        headers: None,
        compat,
    }
}

/// Generic provider table entry: iterate `key.models` and emit with a fixed
/// `api`/`provider`/`baseUrl`, applying the standard tool-call / deprecated
/// filters.
struct SimpleProvider {
    key: &'static str,
    provider: &'static str,
    api: &'static str,
    base_url: &'static str,
    include_deprecated: bool,
    compat: fn() -> Option<BuildModelCompat>,
}

fn push_simple_providers(
    models: &mut Vec<BuildModel>,
    data: &serde_json::Map<String, serde_json::Value>,
    specs: &[SimpleProvider],
) {
    for spec in specs {
        let Some(items) = models_object(data, spec.key) else {
            continue;
        };
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            if !spec.include_deprecated && is_deprecated(model) {
                continue;
            }
            models.push(build_model(
                id,
                model,
                spec.api,
                spec.provider,
                spec.base_url,
                (spec.compat)(),
            ));
        }
    }
}

// ---- models.dev 转换（provider 列表对齐原版） -------------------------------

fn process_models_dev(data: &serde_json::Value) -> Vec<BuildModel> {
    let mut models = Vec::new();
    let Some(data) = data.as_object() else {
        return models;
    };

    // ---- 通用 provider ----
    let openai_compat = || None;
    let together_compat = || {
        Some(
            BuildModelCompat::new()
                .store(false)
                .developer_role(false)
                .reasoning_effort(false)
                .max_tokens_field("max_tokens")
                .strict_mode(false)
                .long_cache_retention(false),
        )
    };
    let huggingface_compat = || {
        Some(
            BuildModelCompat::new()
                .developer_role(false)
                .max_tokens_field("max_tokens"),
        )
    };
    let moonshot_compat = || {
        Some(
            BuildModelCompat::new()
                .store(false)
                .developer_role(false)
                .reasoning_effort(false)
                .max_tokens_field("max_tokens")
                .strict_mode(false)
                .thinking_format("deepseek"),
        )
    };
    let xiaomi_compat = || {
        Some(
            BuildModelCompat::new()
                .requires_reasoning_content(true)
                .thinking_format("deepseek"),
        )
    };
    let nvidia_compat = || {
        Some(
            BuildModelCompat::new()
                .store(false)
                .developer_role(false)
                .reasoning_effort(false)
                .max_tokens_field("max_tokens")
                .strict_mode(false)
                .long_cache_retention(false),
        )
    };
    let qwen_compat = || {
        Some(
            BuildModelCompat::new()
                .thinking_format("qwen")
                .developer_role(false)
                .store(false)
                .reasoning_effort(true),
        )
    };

    let simple = [
        SimpleProvider {
            key: "anthropic",
            provider: "anthropic",
            api: "anthropic-messages",
            base_url: "https://api.anthropic.com",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "openai",
            provider: "openai",
            api: "openai-responses",
            base_url: "https://api.openai.com/v1",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "google",
            provider: "google",
            api: "google-generative-ai",
            base_url: "https://generativelanguage.googleapis.com/v1beta",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "groq",
            provider: "groq",
            api: "openai-completions",
            base_url: "https://api.groq.com/openai/v1",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "cerebras",
            provider: "cerebras",
            api: "openai-completions",
            base_url: "https://api.cerebras.ai/v1",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "xai",
            provider: "xai",
            api: "openai-completions",
            base_url: "https://api.x.ai/v1",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "huggingface",
            provider: "huggingface",
            api: "openai-completions",
            base_url: "https://router.huggingface.co/v1",
            include_deprecated: true,
            compat: huggingface_compat,
        },
        SimpleProvider {
            key: "cloudflare-workers-ai",
            provider: "cloudflare-workers-ai",
            api: "openai-completions",
            base_url: CLOUDFLARE_WORKERS_AI_BASE_URL,
            include_deprecated: true,
            compat: || Some(BuildModelCompat::new().session_affinity(true)),
        },
        SimpleProvider {
            key: "moonshotai",
            provider: "moonshotai",
            api: "openai-completions",
            base_url: "https://api.moonshot.ai/v1",
            include_deprecated: true,
            compat: moonshot_compat,
        },
        SimpleProvider {
            key: "moonshotai-cn",
            provider: "moonshotai-cn",
            api: "openai-completions",
            base_url: "https://api.moonshot.cn/v1",
            include_deprecated: true,
            compat: moonshot_compat,
        },
        SimpleProvider {
            key: "xiaomi",
            provider: "xiaomi",
            api: "openai-completions",
            base_url: "https://api.xiaomimimo.com/v1",
            include_deprecated: false,
            compat: xiaomi_compat,
        },
        SimpleProvider {
            key: "xiaomi-token-plan-cn",
            provider: "xiaomi-token-plan-cn",
            api: "openai-completions",
            base_url: "https://token-plan-cn.xiaomimimo.com/v1",
            include_deprecated: false,
            compat: xiaomi_compat,
        },
        SimpleProvider {
            key: "xiaomi-token-plan-ams",
            provider: "xiaomi-token-plan-ams",
            api: "openai-completions",
            base_url: "https://token-plan-ams.xiaomimimo.com/v1",
            include_deprecated: false,
            compat: xiaomi_compat,
        },
        SimpleProvider {
            key: "xiaomi-token-plan-sgp",
            provider: "xiaomi-token-plan-sgp",
            api: "openai-completions",
            base_url: "https://token-plan-sgp.xiaomimimo.com/v1",
            include_deprecated: false,
            compat: xiaomi_compat,
        },
        SimpleProvider {
            key: "mistral",
            provider: "mistral",
            api: "mistral-conversations",
            base_url: "https://api.mistral.ai",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "minimax",
            provider: "minimax",
            api: "anthropic-messages",
            base_url: "https://api.minimax.io/anthropic",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "minimax-cn",
            provider: "minimax-cn",
            api: "anthropic-messages",
            base_url: "https://api.minimaxi.com/anthropic",
            include_deprecated: true,
            compat: openai_compat,
        },
        SimpleProvider {
            key: "qwen-token-plan",
            provider: "qwen-token-plan",
            api: "openai-completions",
            base_url: "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
            include_deprecated: true,
            compat: qwen_compat,
        },
        SimpleProvider {
            key: "qwen-token-plan-cn",
            provider: "qwen-token-plan-cn",
            api: "openai-completions",
            base_url: "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            include_deprecated: true,
            compat: qwen_compat,
        },
    ];

    push_simple_providers(&mut models, data, &simple);

    // Curated exclusions that match the original script.
    models.retain(|m| !(m.provider == "openai" && m.id == "gpt-5.6"));
    models.retain(|m| {
        m.provider != "xai"
            || !matches!(
                m.id.as_str(),
                "grok-3"
                    | "grok-3-fast"
                    | "grok-4.20-0309-non-reasoning"
                    | "grok-4.20-0309-reasoning"
                    | "grok-build-0.1"
                    | "grok-code-fast-1"
            )
    });

    // ---- Amazon Bedrock（按 id 选择 region baseUrl、跳过不支持流式工具的模型） ----
    if let Some(items) = models_object(data, "amazon-bedrock") {
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            if id == "anthropic.claude-opus-5"
                || id.starts_with("ai21.jamba")
                || id.starts_with("mistral.mistral-7b-instruct-v0")
            {
                continue;
            }
            models.push(build_model(
                id,
                model,
                "bedrock-converse-stream",
                "amazon-bedrock",
                &get_bedrock_base_url(id),
                None,
            ));
        }
    }

    // ---- Google Vertex（仅 Gemini 模型） ----
    if let Some(items) = models_object(data, "google-vertex") {
        for (id, model) in items {
            if !is_tool_call(model) || !id.starts_with("gemini-") {
                continue;
            }
            if id == "gemini-3.1-flash-lite-preview" {
                continue;
            }
            models.push(build_model(
                id,
                model,
                "google-vertex",
                "google-vertex",
                VERTEX_BASE_URL,
                None,
            ));
        }
    }

    // ---- Cloudflare AI Gateway（按 `upstream/nativeId` 前缀路由）+ workers-ai 镜像 ----
    {
        let mut gateway_ids: BTreeSet<String> = BTreeSet::new();
        if let Some(items) = models_object(data, "cloudflare-ai-gateway") {
            for (prefixed_id, model) in items {
                if !is_tool_call(model) {
                    continue;
                }
                let Some((upstream, native_id)) = prefixed_id.split_once('/') else {
                    continue;
                };
                let (api, base_url, id, affinity) = match upstream {
                    "openai" => (
                        "openai-responses",
                        CLOUDFLARE_AI_GATEWAY_OPENAI_BASE_URL,
                        native_id,
                        false,
                    ),
                    "anthropic" => (
                        "anthropic-messages",
                        CLOUDFLARE_AI_GATEWAY_ANTHROPIC_BASE_URL,
                        native_id,
                        true,
                    ),
                    "workers-ai" => (
                        "openai-completions",
                        CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL,
                        prefixed_id.as_str(),
                        true,
                    ),
                    _ => continue,
                };
                let compat = affinity.then(|| BuildModelCompat::new().session_affinity(true));
                gateway_ids.insert(id.to_string());
                let mut entry = build_model(id, model, api, "cloudflare-ai-gateway", base_url, compat);
                entry.name = model_name(model, id);
                models.push(entry);
            }
        }
        if let Some(items) = models_object(data, "cloudflare-workers-ai") {
            for (model_id, model) in items {
                if !is_tool_call(model) {
                    continue;
                }
                let id = format!("workers-ai/{model_id}");
                if gateway_ids.contains(&id) {
                    continue;
                }
                gateway_ids.insert(id.clone());
                let mut entry = build_model(
                    &id,
                    model,
                    "openai-completions",
                    "cloudflare-ai-gateway",
                    CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL,
                    Some(BuildModelCompat::new().session_affinity(true)),
                );
                entry.name = model_name(model, &id);
                models.push(entry);
            }
        }
    }

    // ---- OpenCode Zen / Go（按 models.dev `provider.npm` 路由） ----
    for (key, provider, base_path) in [
        ("opencode", "opencode", "https://opencode.ai/zen"),
        ("opencode-go", "opencode-go", "https://opencode.ai/zen/go"),
    ] {
        let Some(items) = models_object(data, key) else {
            continue;
        };
        for (id, model) in items {
            if !is_tool_call(model) || is_deprecated(model) {
                continue;
            }
            if (provider == "opencode" || provider == "opencode-go") && id == "gpt-5.3-codex-spark" {
                continue;
            }
            let npm = model
                .get("provider")
                .and_then(|p| p.get("npm"))
                .and_then(serde_json::Value::as_str);
            let (mut api, mut base_url, mut compat): (&str, String, Option<BuildModelCompat>) =
                match npm {
                    Some("@ai-sdk/openai") => (
                        "openai-responses",
                        format!("{base_path}/v1"),
                        None,
                    ),
                    Some("@ai-sdk/anthropic") => ("anthropic-messages", base_path.to_string(), None),
                    Some("@ai-sdk/google") => {
                        ("google-generative-ai", format!("{base_path}/v1"), None)
                    }
                    Some("@ai-sdk/alibaba") => (
                        "openai-completions",
                        format!("{base_path}/v1"),
                        Some(BuildModelCompat::new()),
                    ),
                    _ => (
                        "openai-completions",
                        format!("{base_path}/v1"),
                        Some(BuildModelCompat::new()),
                    ),
                };
            if provider == "opencode-go" {
                if id == "minimax-m2.7" || id == "qwen3.5-plus" || id == "qwen3.6-plus" {
                    api = "openai-completions";
                    base_url = format!("{base_path}/v1");
                }
                if id == "qwen3.5-plus" || id == "qwen3.6-plus" {
                    compat = Some(BuildModelCompat::new().thinking_format("qwen"));
                }
            }
            if api == "openai-completions" {
                compat = Some(
                    compat
                        .unwrap_or_else(BuildModelCompat::new)
                        .max_tokens_field("max_tokens"),
                );
            }
            models.push(build_model(
                id, model, api, provider, &base_url, compat,
            ));
        }
    }

    // ---- GitHub Copilot（Claude → anthropic，GPT/Grok/OSWE/MAI → responses，其余 completions） ----
    if let Some(items) = models_object(data, "github-copilot") {
        for (id, model) in items {
            if !is_tool_call(model) || is_deprecated(model) {
                continue;
            }
            let is_claude = id.starts_with("claude-haiku-4")
                || id.starts_with("claude-sonnet-4")
                || id.starts_with("claude-sonnet-5")
                || id.starts_with("claude-opus-4")
                || id.starts_with("claude-opus-5")
                || id.starts_with("claude-fable-4")
                || id.starts_with("claude-fable-5");
            let needs_responses = id.starts_with("gpt-")
                || id.starts_with("grok-")
                || id.starts_with("oswe")
                || id.starts_with("mai-");
            let api = if is_claude {
                "anthropic-messages"
            } else if needs_responses {
                "openai-responses"
            } else {
                "openai-completions"
            };
            let compat = if api == "openai-completions" {
                Some(
                    BuildModelCompat::new()
                        .store(false)
                        .developer_role(false)
                        .reasoning_effort(false),
                )
            } else {
                None
            };
            let mut entry = build_model(
                id,
                model,
                api,
                "github-copilot",
                "https://api.individual.githubcopilot.com",
                compat,
            );
            entry.context_window = entry.context_window.max(128_000);
            if entry.max_tokens == 4096 {
                entry.max_tokens = 8192;
            }
            entry.headers = Some(
                COPILOT_HEADERS
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            );
            models.push(entry);
        }
    }

    // ---- Kimi For Coding（规范化 k2p5/k2p6/k2p7 别名） ----
    if let Some(items) = models_object(data, "kimi-for-coding") {
        let has_canonical = items.contains_key("kimi-for-coding");
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            let is_alias = matches!(id.as_str(), "k2p5" | "k2p6" | "k2p7");
            if is_alias && has_canonical {
                continue;
            }
            let normalized_id = if is_alias { "kimi-for-coding" } else { id };
            let mut entry = build_model(
                normalized_id,
                model,
                "anthropic-messages",
                "kimi-coding",
                "https://api.kimi.com/coding",
                None,
            );
            if is_alias {
                entry.name = "Kimi For Coding".into();
            }
            models.push(entry);
        }
    }

    // ---- ZAI / ZAI Coding CN ----
    for (source, provider, base_url) in [
        (
            "zai-coding-plan",
            "zai",
            "https://api.z.ai/api/coding/paas/v4",
        ),
        (
            "zhipuai-coding-plan",
            "zai-coding-cn",
            "https://open.bigmodel.cn/api/coding/paas/v4",
        ),
    ] {
        let Some(items) = models_object(data, source) else {
            continue;
        };
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            let mut compat = BuildModelCompat::new()
                .developer_role(false)
                .thinking_format("zai");
            if !ZAI_TOOL_STREAM_UNSUPPORTED_MODELS.contains(&id.as_str()) {
                compat = compat.zai_tool_stream(true);
            }
            models.push(build_model(
                id, model, "openai-completions", provider, base_url, Some(compat),
            ));
        }
    }

    // ---- Baseten ----
    if let Some(items) = models_object(data, "baseten") {
        let base_compat = || {
            BuildModelCompat::new()
                .store(false)
                .developer_role(false)
                .reasoning_effort(false)
                .strict_mode(true)
                .long_cache_retention(false)
                .max_tokens_field("max_tokens")
        };
        for (id, model) in items {
            if is_deprecated(model) {
                continue;
            }
            models.push(build_model(
                id,
                model,
                "openai-completions",
                "baseten",
                "https://inference.baseten.co/v1",
                Some(base_compat()),
            ));
        }
    }

    // ---- NVIDIA NIM（按现场返回的 model id 过滤） ----
    if let Some(items) = models_object(data, "nvidia") {
        // Fetched separately in `run`; here we just emit with the models.dev id
        // and let the live filter happen in `run` via `process_nvidia_models`.
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            let mut entry = build_model(
                id,
                model,
                "openai-completions",
                "nvidia",
                NVIDIA_BASE_URL,
                nvidia_compat(),
            );
            entry.headers = Some(
                [(NVIDIA_HEADER_POLL_SECONDS.0.to_string(), NVIDIA_HEADER_POLL_SECONDS.1.to_string())]
                    .into_iter()
                    .collect(),
            );
            models.push(entry);
        }
    }

    // ---- Fireworks（glm-*/kimi-k3 → completions，其余 anthropic） ----
    if let Some(items) = models_object(data, "fireworks-ai") {
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            let (api, base_url, compat) = if id.contains("glm-") || id.contains("kimi-k3") {
                (
                    "openai-completions",
                    "https://api.fireworks.ai/inference/v1",
                    Some(
                        BuildModelCompat::new()
                            .store(false)
                            .developer_role(false)
                            .session_affinity(true)
                            .long_cache_retention(false),
                    ),
                )
            } else {
                ("anthropic-messages", "https://api.fireworks.ai/inference", None)
            };
            models.push(build_model(
                id, model, api, "fireworks", base_url, compat,
            ));
        }
    }

    // ---- Alibaba Token Plan → qwen-token-plan / -individual / -cn ----
    {
        let individual: BTreeSet<&str> = QWEN_TOKEN_PLAN_INDIVIDUAL_MODEL_IDS.iter().copied().collect();
        for (source, provider, base_url, filter) in [
            (
                "alibaba-token-plan",
                "qwen-token-plan",
                "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
                None,
            ),
            (
                "alibaba-token-plan",
                "qwen-token-plan-individual",
                "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1",
                Some(&individual),
            ),
            (
                "alibaba-token-plan-cn",
                "qwen-token-plan-cn",
                "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
                None,
            ),
        ] {
            let Some(items) = models_object(data, source) else {
                continue;
            };
            for (id, model) in items {
                if !is_tool_call(model) || id == "qwen3.8-max-preview" {
                    continue;
                }
                if let Some(allowed) = filter {
                    if !allowed.contains(id.as_str()) {
                        continue;
                    }
                }
                models.push(build_model(
                    id,
                    model,
                    "openai-completions",
                    provider,
                    base_url,
                    qwen_compat(),
                ));
            }
        }
    }

    // ---- Together（models.dev 键可能是 together / togetherai / together-ai） ----
    if let Some(items) = models_object(data, "together")
        .or_else(|| models_object(data, "togetherai"))
        .or_else(|| models_object(data, "together-ai"))
    {
        for (id, model) in items {
            if !is_tool_call(model) || is_deprecated(model) {
                continue;
            }
            models.push(build_model(
                id,
                model,
                "openai-completions",
                "together",
                TOGETHER_BASE_URL,
                together_compat(),
            ));
        }
    }

    // ---- MiniMax：只保留直连支持的模型 ----
    models.retain(|m| {
        (m.provider != "minimax" && m.provider != "minimax-cn")
            || matches!(
                m.id.as_str(),
                "MiniMax-M2.7" | "MiniMax-M2.7-highspeed" | "MiniMax-M3"
            )
    });

    // ---- 手工补充模型（models.dev 缺失或需覆盖） ----
    models.extend(manual_models());

    // ---- Azure OpenAI Responses：openai-responses 模型的镜像 ----
    let azure_overrides: HashMap<&str, u64> = [
        ("gpt-5.4", 1_050_000_u64),
        ("gpt-5.5", 1_050_000),
        ("gpt-5.6-luna", 1_050_000),
        ("gpt-5.6-sol", 1_050_000),
        ("gpt-5.6-terra", 1_050_000),
    ]
    .into_iter()
    .collect();
    let azure: Vec<BuildModel> = models
        .iter()
        .filter(|m| m.provider == "openai" && m.api == "openai-responses")
        .map(|m| {
            let mut clone = m.clone();
            clone.api = "azure-openai-responses".into();
            clone.provider = "azure-openai-responses".into();
            clone.base_url = String::new();
            if let Some(ctx) = azure_overrides.get(m.id.as_str()) {
                clone.context_window = *ctx;
            }
            clone
        })
        .collect();
    models.extend(azure);

    models
}

/// Hand-curated entries that models.dev does not provide (or that pi curates
/// explicitly). Mirrors the ant-ling / openai-codex / missing-OpenAI / DeepSeek
/// / Mistral Medium 3.5 additions in the original script.
fn manual_models() -> Vec<BuildModel> {
    let mut models = Vec::new();

    let ant_ling_compat = BuildModelCompat::new()
        .store(false)
        .developer_role(false)
        .reasoning_effort(false)
        .max_tokens_field("max_tokens")
        .long_cache_retention(false);
    for (id, name, cost, compat) in [
        (
            "Ling-2.6-flash",
            "Ling 2.6 Flash",
            [0.01, 0.02, 0.0, 0.0],
            ant_ling_compat.clone(),
        ),
        (
            "Ling-2.6-1T",
            "Ling 2.6 1T",
            [0.06, 0.25, 0.0, 0.0],
            ant_ling_compat.clone(),
        ),
        (
            "Ring-2.6-1T",
            "Ring 2.6 1T",
            [0.06, 0.25, 0.0, 0.0],
            ant_ling_compat.clone().thinking_format("ant-ling"),
        ),
    ] {
        let reasoning = id == "Ring-2.6-1T";
        models.push(manual_model(
            id,
            name,
            "openai-completions",
            "ant-ling",
            "https://api.ant-ling.com/v1",
            reasoning,
            &["text"],
            cost,
            262_144,
            65_536,
            Some(compat),
        ));
    }

    // OpenAI Codex (ChatGPT OAuth) — explicit list, not from models.dev.
    const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
    const CODEX_CONTEXT: u64 = 272_000;
    const CODEX_SPARK_CONTEXT: u64 = 128_000;
    const CODEX_MAX_TOKENS: u64 = 128_000;
    for (id, name, input, cost, ctx) in [
        (
            "gpt-6-astra",
            "GPT-6 Astra",
            vec!["text", "image"],
            [10.0, 50.0, 1.0, 12.5],
            CODEX_CONTEXT,
        ),
        (
            "gpt-5.3-codex-spark",
            "GPT-5.3 Codex Spark",
            vec!["text"],
            [1.75, 14.0, 0.175, 0.0],
            CODEX_SPARK_CONTEXT,
        ),
        (
            "gpt-5.5",
            "GPT-5.5",
            vec!["text", "image"],
            [5.0, 30.0, 0.5, 0.0],
            CODEX_CONTEXT,
        ),
        (
            "gpt-5.6-luna",
            "GPT-5.6 Luna",
            vec!["text", "image"],
            [0.2, 1.2, 0.02, 0.25],
            CODEX_CONTEXT,
        ),
        (
            "gpt-5.6-sol",
            "GPT-5.6 Sol",
            vec!["text", "image"],
            [5.0, 30.0, 0.5, 6.25],
            CODEX_CONTEXT,
        ),
        (
            "gpt-5.6-terra",
            "GPT-5.6 Terra",
            vec!["text", "image"],
            [2.0, 12.0, 0.2, 2.5],
            CODEX_CONTEXT,
        ),
    ] {
        models.push(manual_model(
            id,
            name,
            "openai-codex-responses",
            "openai-codex",
            CODEX_BASE_URL,
            true,
            &input,
            cost,
            ctx,
            CODEX_MAX_TOKENS,
            None,
        ));
    }

    // Missing OpenAI models.
    for (id, name, reasoning, input, cost, ctx, max) in [
        (
            "gpt-6-astra",
            "GPT-6 Astra",
            true,
            vec!["text", "image"],
            [10.0, 50.0, 1.0, 12.5],
            272_000_u64,
            128_000_u64,
        ),
        (
            "gpt-5.6-sol",
            "GPT-5.6 Sol",
            true,
            vec!["text", "image"],
            [5.0, 30.0, 0.5, 6.25],
            272_000,
            128_000,
        ),
        (
            "gpt-5.6-terra",
            "GPT-5.6 Terra",
            true,
            vec!["text", "image"],
            [2.0, 12.0, 0.2, 2.5],
            272_000,
            128_000,
        ),
        (
            "gpt-5.6-luna",
            "GPT-5.6 Luna",
            true,
            vec!["text", "image"],
            [0.2, 1.2, 0.02, 0.25],
            272_000,
            128_000,
        ),
        (
            "gpt-5-chat-latest",
            "GPT-5 Chat Latest",
            false,
            vec!["text", "image"],
            [1.25, 10.0, 0.125, 0.0],
            128_000,
            16_384,
        ),
    ] {
        models.push(manual_model(
            id,
            name,
            "openai-responses",
            "openai",
            "https://api.openai.com/v1",
            reasoning,
            &input,
            cost,
            ctx,
            max,
            None,
        ));
    }

    // DeepSeek curated entries (time-based off-peak rates not representable).
    let deepseek_compat = BuildModelCompat::new()
        .requires_reasoning_content(true)
        .thinking_format("deepseek");
    models.push(manual_model(
        "deepseek-flash",
        "DeepSeek V4.1 Flash",
        "openai-completions",
        "deepseek",
        "https://api.deepseek.com",
        true,
        &["text", "image"],
        [0.3, 1.2, 0.006, 0.0],
        1_000_000,
        384_000,
        Some(deepseek_compat.clone()),
    ));
    models.push(manual_model(
        "deepseek-v4-pro",
        "DeepSeek V4 Pro",
        "openai-completions",
        "deepseek",
        "https://api.deepseek.com",
        true,
        &["text"],
        [1.32, 3.96, 0.044, 0.0],
        1_000_000,
        384_000,
        Some(deepseek_compat),
    ));

    // Mistral Medium 3.5 (until models.dev includes it).
    models.push(manual_model(
        "mistral-medium-3.5",
        "Mistral Medium 3.5",
        "mistral-conversations",
        "mistral",
        "https://api.mistral.ai",
        true,
        &["text", "image"],
        [1.5, 7.5, 0.0, 0.0],
        262_144,
        262_144,
        None,
    ));

    models
}

/// Build a hand-curated model entry.
#[allow(clippy::too_many_arguments)]
fn manual_model(
    id: &str,
    name: &str,
    api: &str,
    provider: &str,
    base_url: &str,
    reasoning: bool,
    modalities: &[&str],
    cost: [f64; 4],
    context_window: u64,
    max_tokens: u64,
    compat: Option<BuildModelCompat>,
) -> BuildModel {
    let input = if modalities.contains(&"image") {
        vec!["text".to_string(), "image".to_string()]
    } else {
        vec!["text".to_string()]
    };
    BuildModel {
        id: id.to_string(),
        name: name.to_string(),
        api: api.to_string(),
        provider: provider.to_string(),
        base_url: base_url.to_string(),
        reasoning,
        thinking_level_map: None,
        input,
        cost: BuildModelCost {
            input: cost[0],
            output: cost[1],
            cache_read: cost[2],
            cache_write: cost[3],
        },
        context_window,
        max_tokens,
        headers: None,
        compat,
    }
}

/// NVIDIA live-id filter + id normalization (match TS `loadModelsDevData`'s
/// NVIDIA block). Applied after `process_models_dev` because it needs the live
/// NIM catalog.
fn process_nvidia_models(
    models: Vec<BuildModel>,
    live_ids: &HashMap<String, String>,
) -> Vec<BuildModel> {
    models
        .into_iter()
        .filter_map(|mut model| {
            if model.provider != "nvidia" {
                return Some(model);
            }
            let live_id = live_ids
                .get(&model.id)
                .or_else(|| live_ids.get(&normalize_nvidia_model_id(&model.id)))?;
            if NVIDIA_NIM_UNSUPPORTED_MODELS.contains(&live_id.as_str()) {
                return None;
            }
            model.id.clone_from(live_id);
            Some(model)
        })
        .collect()
}

// ---- OpenRouter 转换 --------------------------------------------------------

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
            let input_modalities = m.architecture.as_ref().map_or_else(
                || vec!["text".to_string()],
                |arch| {
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
                },
            );
            BuildModel {
                id: m.id.clone(),
                name: m.name,
                api: "openai-completions".into(),
                provider: "openrouter".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                reasoning,
                thinking_level_map: None,
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
                context_window: m
                    .top_provider
                    .as_ref()
                    .and_then(|t| t.context_length)
                    .or(m.context_length)
                    .unwrap_or(4096),
                max_tokens: m
                    .top_provider
                    .and_then(|t| t.max_completion_tokens)
                    .unwrap_or(4096),
                headers: None,
                compat: Some(
                    BuildModelCompat::new()
                        .max_tokens_field("max_tokens")
                        .thinking_format("openrouter")
                        .store(false),
                ),
            }
        })
        .collect()
}

// ---- 入口 -------------------------------------------------------------------

pub fn run(out: &std::path::Path, check_only: bool) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("build HTTP client")?;

    let openrouter_raw = fetch_openrouter_models(&client)?;
    let openrouter_models = process_openrouter_models(openrouter_raw);
    eprintln!("OpenRouter: {} tool-capable models", openrouter_models.len());

    let models_dev = fetch_models_dev(&client)?;
    let live_nvidia_ids = fetch_nvidia_nim_ids(&client).unwrap_or_default();
    eprintln!("NVIDIA NIM: {} live model ids", live_nvidia_ids.len());
    let models_dev_models = if models_dev.is_object() {
        process_nvidia_models(process_models_dev(&models_dev), &live_nvidia_ids)
    } else {
        Vec::new()
    };
    eprintln!("models.dev: {} models", models_dev_models.len());

    let ai_gateway_models = fetch_ai_gateway_models(&client)?;
    eprintln!(
        "Vercel AI Gateway: {} tool-capable models",
        ai_gateway_models.len()
    );

    // models.dev 优先；OpenRouter / AI Gateway 去重后追加。
    let mut all_models = models_dev_models;
    for m in openrouter_models.into_iter().chain(ai_gateway_models) {
        if !all_models
            .iter()
            .any(|e| e.provider == m.provider && e.id == m.id)
        {
            all_models.push(m);
        }
    }

    // OpenRouter `auto` alias (match TS).
    if !all_models
        .iter()
        .any(|m| m.provider == "openrouter" && m.id == "auto")
    {
        all_models.push(manual_model(
            "auto",
            "Auto",
            "openai-completions",
            "openrouter",
            "https://openrouter.ai/api/v1",
            true,
            &["text", "image"],
            [0.0, 0.0, 0.0, 0.0],
            2_000_000,
            30_000,
            Some(
                BuildModelCompat::new()
                    .max_tokens_field("max_tokens")
                    .thinking_format("openrouter")
                    .store(false),
            ),
        ));
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
        eprintln!(
            "OK: {} 与现场抓取一致（{} models, {} providers）",
            out.display(),
            all_models.len(),
            by_provider.len()
        );
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
