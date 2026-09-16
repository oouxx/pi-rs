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
    #[serde(default)]
    reasoning: Option<OpenRouterReasoning>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OpenRouterReasoning {
    #[serde(default)]
    mandatory: Option<bool>,
    #[serde(default)]
    supported_efforts: Option<Vec<serde_json::Value>>,
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
    compat: Option<Compat>,
    /// models.dev `reasoning_options` (not serialized; feeds the metadata
    /// pipeline's verified thinking-level map).
    #[serde(skip)]
    reasoning_options: Option<serde_json::Value>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tiers: Vec<BuildModelCostTier>,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct BuildModelCostTier {
    input: f64,
    output: f64,
    #[serde(rename = "cacheRead")]
    cache_read: f64,
    #[serde(rename = "cacheWrite")]
    cache_write: f64,
    #[serde(rename = "inputTokensAbove")]
    input_tokens_above: u64,
}

/// Provider-specific `compat` object. Backed by a JSON object so the metadata
/// pipeline can carry the full Anthropic / Responses / OpenAI-completions union
/// (and fields pi-rs models only loosely) without a hand-typed Rust union.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
#[serde(transparent)]
struct Compat(serde_json::Map<String, serde_json::Value>);

impl Compat {
    fn new() -> Self {
        Self::default()
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn insert(&mut self, key: &str, value: impl Into<serde_json::Value>) {
        self.0.insert(key.to_string(), value.into());
    }

    fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }

    fn get_bool(&self, key: &str) -> bool {
        self.0
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    /// Shallow-merge a JSON object into this compat (later keys win).
    fn merge(&mut self, other: serde_json::Value) {
        if let serde_json::Value::Object(map) = other {
            for (key, value) in map {
                self.0.insert(key, value);
            }
        }
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::Value::Object(self.0.clone())
    }

    // ---- chainable builders (same call sites as before) ----
    fn set(mut self, key: &str, value: impl Into<serde_json::Value>) -> Self {
        self.insert(key, value);
        self
    }
    fn max_tokens_field(self, value: &str) -> Self {
        self.set("maxTokensField", value)
    }
    fn thinking_format(self, value: &str) -> Self {
        self.set("thinkingFormat", value)
    }
    fn developer_role(self, value: bool) -> Self {
        self.set("supportsDeveloperRole", value)
    }
    fn store(self, value: bool) -> Self {
        self.set("supportsStore", value)
    }
    fn reasoning_effort(self, value: bool) -> Self {
        self.set("supportsReasoningEffort", value)
    }
    fn strict_mode(self, value: bool) -> Self {
        self.set("supportsStrictMode", value)
    }
    fn long_cache_retention(self, value: bool) -> Self {
        self.set("supportsLongCacheRetention", value)
    }
    fn session_affinity(self, value: bool) -> Self {
        self.set("sendSessionAffinityHeaders", value)
    }
    fn requires_reasoning_content(self, value: bool) -> Self {
        self.set("requiresReasoningContentOnAssistantMessages", value)
    }
    fn zai_tool_stream(self, value: bool) -> Self {
        self.set("zaiToolStream", value)
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
                tiers: Vec::new(),
            },
            context_window: model.context_window.unwrap_or(4096),
            max_tokens: model.max_tokens.unwrap_or(4096),
            headers: None,
            compat: None,
            reasoning_options: None,
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
        tiers: Vec::new(),
    }
}

/// TS `getModelsDevCost`: base cost plus `cost.tiers` (context-sized tiers).
fn get_cost_with_tiers(model: &serde_json::Value) -> BuildModelCost {
    let mut cost = get_cost(model);
    if let Some(tiers) = model
        .get("cost")
        .and_then(|c| c.get("tiers"))
        .and_then(serde_json::Value::as_array)
    {
        for tier in tiers {
            let context = tier.get("tier");
            if context.and_then(|c| c.get("type")).and_then(serde_json::Value::as_str)
                != Some("context")
            {
                continue;
            }
            let Some(size) = context.and_then(|c| c.get("size")).and_then(serde_json::Value::as_u64)
            else {
                continue;
            };
            cost.tiers.push(BuildModelCostTier {
                input_tokens_above: size,
                input: tier.get("input").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                output: tier.get("output").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                cache_read: tier
                    .get("cache_read")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
                cache_write: tier
                    .get("cache_write")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
            });
        }
    }
    cost
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
    compat: Option<Compat>,
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
        reasoning_options: model.get("reasoning_options").cloned(),
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
    compat: fn() -> Option<Compat>,
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


// ---- provider 级 compat / thinkingLevelMap helper（对齐原版） ----

const TOGETHER_REASONING_ONLY_MODELS: &[&str] = &["deepseek-ai/DeepSeek-R1", "MiniMaxAI/MiniMax-M2.7"];
const TOGETHER_REASONING_EFFORT_MODELS: &[&str] = &["openai/gpt-oss-20b", "openai/gpt-oss-120b"];
const TOGETHER_TOGGLE_REASONING_EFFORT_MODELS: &[&str] = &["deepseek-ai/DeepSeek-V4-Pro"];
const OPENCODE_LONG_CACHE_UNSUPPORTED: &[&str] = &[
    "opencode:deepseek-v4-flash",
    "opencode:deepseek-v4-pro",
    "opencode:kimi-k2.5",
    "opencode:kimi-k2.6",
    "opencode:minimax-m2.7",
    "opencode-go:kimi-k2.6",
];
const QWEN_TOKEN_PLAN_REASONING_EFFORT_FALLBACK: &[&str] = &["glm-5", "glm-5.1"];
const FIREWORKS_ADAPTIVE_THINKING_FALLBACK_MODELS: &[&str] = &[
    "accounts/fireworks/models/deepseek-v4-flash-0731",
    "accounts/fireworks/models/deepseek-v4-flash-vision-exp",
    "accounts/fireworks/models/deepseek-v4-pro-0813",
    "accounts/fireworks/models/qwen3p8-max",
    "accounts/fireworks/models/qwen3p8-2p4t-a95b",
];

fn together_base_compat() -> Compat {
    Compat::new()
        .store(false)
        .developer_role(false)
        .reasoning_effort(false)
        .max_tokens_field("max_tokens")
        .strict_mode(false)
        .long_cache_retention(false)
}

fn get_together_compat(id: &str, reasoning: bool) -> Compat {
    if !reasoning {
        return together_base_compat();
    }
    if TOGETHER_REASONING_EFFORT_MODELS.contains(&id) {
        return together_base_compat().reasoning_effort(true).thinking_format("openai");
    }
    if TOGETHER_TOGGLE_REASONING_EFFORT_MODELS.contains(&id) {
        return together_base_compat().reasoning_effort(true).thinking_format("together");
    }
    if TOGETHER_REASONING_ONLY_MODELS.contains(&id) {
        return together_base_compat();
    }
    together_base_compat().thinking_format("together")
}

fn get_together_thinking_level_map(id: &str, reasoning: bool) -> Option<serde_json::Value> {
    if !reasoning {
        return None;
    }
    if TOGETHER_REASONING_EFFORT_MODELS.contains(&id) {
        return Some(serde_json::json!({"off": null, "minimal": null}));
    }
    if TOGETHER_TOGGLE_REASONING_EFFORT_MODELS.contains(&id) {
        return Some(serde_json::json!({
            "minimal": null, "low": null, "medium": null, "high": "high", "xhigh": null
        }));
    }
    if TOGETHER_REASONING_ONLY_MODELS.contains(&id) {
        return Some(serde_json::json!({
            "off": null, "minimal": null, "low": null, "medium": null
        }));
    }
    Some(serde_json::json!({"minimal": null, "low": null, "medium": null}))
}

fn has_reasoning_option(model: &serde_json::Value, kind: &str) -> bool {
    model
        .get("reasoning_options")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|opts| {
            opts.iter()
                .any(|o| o.get("type").and_then(serde_json::Value::as_str) == Some(kind))
        })
}

fn set_thinking_level_map(model: &mut BuildModel, map: Option<serde_json::Value>) {
    if map.is_some() {
        model.thinking_level_map = map;
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
    let huggingface_compat = || Some(Compat::new().developer_role(false));
    let xiaomi_compat = || {
        Some(
            Compat::new()
                .requires_reasoning_content(true)
                .thinking_format("deepseek"),
        )
    };
    let nvidia_compat = || {
        Some(
            Compat::new()
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
            Compat::new()
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
            compat: || Some(Compat::new().session_affinity(true)),
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
            let compat = (model.get("structured_output").and_then(serde_json::Value::as_bool)
                == Some(true))
            .then(|| Compat::new().strict_mode(true));
            models.push(build_model(
                id,
                model,
                "bedrock-converse-stream",
                "amazon-bedrock",
                &get_bedrock_base_url(id),
                compat,
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
                let compat = affinity.then(|| Compat::new().session_affinity(true));
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
                    Some(Compat::new().session_affinity(true)),
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
            let (mut api, mut base_url, mut compat): (&str, String, Option<Compat>) =
                match npm {
                    Some("@ai-sdk/openai") => (
                        "openai-responses",
                        format!("{base_path}/v1"),
                        Some(Compat::new().set("sessionAffinityFormat", "openai-nosession")),
                    ),
                    Some("@ai-sdk/anthropic") => ("anthropic-messages", base_path.to_string(), None),
                    Some("@ai-sdk/google") => {
                        ("google-generative-ai", format!("{base_path}/v1"), None)
                    }
                    Some("@ai-sdk/alibaba") => (
                        "openai-completions",
                        format!("{base_path}/v1"),
                        Some(Compat::new().set("cacheControlFormat", "anthropic")),
                    ),
                    _ => ("openai-completions", format!("{base_path}/v1"), None),
                };
            if provider == "opencode" && id == "grok-build-0.1" {
                compat = Some(
                    compat
                        .unwrap_or_else(Compat::new)
                        .set("supportsReasoningEffort", false),
                );
            }
            if (provider == "opencode" || provider == "opencode-go") && id == "kimi-k2.6" {
                compat = Some(
                    compat
                        .unwrap_or_else(Compat::new)
                        .thinking_format("deepseek")
                        .set("supportsReasoningEffort", false),
                );
            }
            if provider == "opencode-go" {
                if id == "minimax-m2.7" || id == "qwen3.5-plus" || id == "qwen3.6-plus" {
                    api = "openai-completions";
                    base_url = format!("{base_path}/v1");
                }
                if id == "qwen3.5-plus" || id == "qwen3.6-plus" {
                    compat = Some(compat.unwrap_or_else(Compat::new).thinking_format("qwen"));
                }
            }
            if api == "openai-completions" {
                let mut c = compat.unwrap_or_else(Compat::new).max_tokens_field("max_tokens");
                if OPENCODE_LONG_CACHE_UNSUPPORTED
                    .contains(&format!("{provider}:{id}").as_str())
                {
                    c = c.long_cache_retention(false);
                }
                compat = Some(c);
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
                    Compat::new()
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
            entry.cost = get_cost_with_tiers(model);
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
            let is_kimi_k3 = normalized_id == "k3";
            let allow_empty_signature = is_kimi_k3 || normalized_id == "kimi-for-coding";
            let compat = if allow_empty_signature {
                Compat::new()
                    .set("forceAdaptiveThinking", true)
                    .set("allowEmptySignature", true)
            } else {
                Compat::new().set("forceAdaptiveThinking", true)
            };
            let mut entry = build_model(
                normalized_id,
                model,
                "anthropic-messages",
                "kimi-coding",
                "https://api.kimi.com/coding",
                Some(compat),
            );
            entry.reasoning = is_kimi_k3 || model_reasoning(model);
            if is_alias {
                entry.name = "Kimi For Coding".into();
            }
            // Subscription-backed: models.dev reports zero cost; use equivalent rates.
            let implied: Option<[f64; 4]> = match normalized_id {
                "k3" => Some([3.0, 15.0, 0.3, 0.0]),
                "kimi-for-coding" => Some([0.95, 4.0, 0.19, 0.0]),
                "kimi-for-coding-highspeed" => Some([1.9, 8.0, 0.38, 0.0]),
                "kimi-k2-thinking" => Some([0.6, 2.5, 0.15, 0.0]),
                _ => None,
            };
            if let Some([input, output, cache_read, cache_write]) = implied {
                if entry.cost.input == 0.0 {
                    entry.cost.input = input;
                }
                if entry.cost.output == 0.0 {
                    entry.cost.output = output;
                }
                if entry.cost.cache_read == 0.0 {
                    entry.cost.cache_read = cache_read;
                }
                if entry.cost.cache_write == 0.0 {
                    entry.cost.cache_write = cache_write;
                }
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
            let options = model.get("reasoning_options").cloned().unwrap_or(serde_json::Value::Null);
            let mut thinking_level_map = get_effort_thinking_level_map(&options);
            if let Some(serde_json::Value::Object(map)) = &mut thinking_level_map {
                if id == "glm-5.2" || id == "glm-5.2-highspeed" {
                    map.insert("off".into(), serde_json::json!("none"));
                }
            }
            let mut compat = Compat::new()
                .developer_role(false)
                .thinking_format("zai");
            if thinking_level_map.is_some() {
                compat = compat.reasoning_effort(true);
            }
            if !ZAI_TOOL_STREAM_UNSUPPORTED_MODELS.contains(&id.as_str()) {
                compat = compat.zai_tool_stream(true);
            }
            let mut entry = build_model(
                id, model, "openai-completions", provider, base_url, Some(compat),
            );
            // Reference cost: data.zai.models[id].cost when present.
            if let Some(reference) = data
                .get("zai")
                .and_then(|z| z.get("models"))
                .and_then(|m| m.get(id))
                .and_then(|m| m.get("cost"))
            {
                entry.cost = BuildModelCost {
                    input: reference.get("input").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                    output: reference.get("output").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                    cache_read: reference
                        .get("cache_read")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0),
                    cache_write: reference
                        .get("cache_write")
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0),
                    tiers: Vec::new(),
                };
            }
            set_thinking_level_map(&mut entry, thinking_level_map);
            models.push(entry);
        }
    }

    // ---- Baseten（toggle / effort compat + thinkingLevelMap） ----
    if let Some(items) = models_object(data, "baseten") {
        let base_compat = || {
            Compat::new()
                .store(false)
                .developer_role(false)
                .reasoning_effort(false)
                .strict_mode(true)
                .long_cache_retention(false)
                .max_tokens_field("max_tokens")
                .set("supportsUsageInStreaming", true)
        };
        for (id, model) in items {
            if is_deprecated(model) {
                continue;
            }
            let is_glm52 = id == "zai-org/GLM-5.2" || id == "zai-org/GLM-5.2-Fast";
            let supports_toggle = has_reasoning_option(model, "toggle") || is_glm52;
            let supports_effort = has_reasoning_option(model, "effort") || is_glm52;
            let compat = match (supports_toggle, supports_effort) {
                (true, true) => base_compat()
                    .reasoning_effort(true)
                    .thinking_format("baseten")
                    .set("chatTemplateArgs", serde_json::json!({"enable_thinking": {"$var": "thinking.enabled"}})),
                (true, false) => base_compat()
                    .thinking_format("baseten")
                    .set("chatTemplateArgs", serde_json::json!({"enable_thinking": {"$var": "thinking.enabled"}})),
                (false, true) => base_compat().reasoning_effort(true).thinking_format("openai"),
                (false, false) => base_compat(),
            };
            let thinking_level_map = if is_glm52 {
                Some(serde_json::json!({
                    "off": "none", "minimal": null, "low": null, "medium": null,
                    "high": "high", "xhigh": null, "max": "max"
                }))
            } else if supports_toggle {
                Some(serde_json::json!({
                    "off": "off", "minimal": null, "low": null, "medium": null,
                    "high": "high", "xhigh": null, "max": null
                }))
            } else {
                get_effort_thinking_level_map(&model.get("reasoning_options").cloned().unwrap_or(serde_json::Value::Null))
            };
            let mut entry = build_model(
                id,
                model,
                "openai-completions",
                "baseten",
                "https://inference.baseten.co/v1",
                Some(compat),
            );
            if is_glm52 {
                entry.input = vec!["text".to_string()];
            }
            set_thinking_level_map(&mut entry, thinking_level_map);
            models.push(entry);
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
                let mut compat = Compat::new()
                    .store(false)
                    .developer_role(false)
                    .session_affinity(true)
                    .long_cache_retention(false);
                if id.contains("kimi-k3") {
                    compat = compat
                        .set("requiresReasoningContentOnAssistantMessages", true)
                        .thinking_format("openai")
                        .set("deferredToolsMode", "kimi");
                }
                ("openai-completions", "https://api.fireworks.ai/inference/v1", Some(compat))
            } else {
                let force_adaptive = has_reasoning_option(model, "effort")
                    || FIREWORKS_ADAPTIVE_THINKING_FALLBACK_MODELS.contains(&id.as_str());
                let mut compat = Compat::new()
                    .set("supportsToolReferences", true)
                    .set("allowEmptySignature", true)
                    .session_affinity(true)
                    .set("supportsEagerToolInputStreaming", false)
                    .set("supportsCacheControlOnTools", false)
                    .long_cache_retention(false);
                if force_adaptive {
                    compat = compat.set("forceAdaptiveThinking", true);
                }
                ("anthropic-messages", "https://api.fireworks.ai/inference", Some(compat))
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
                let options = model.get("reasoning_options").cloned().unwrap_or(serde_json::Value::Null);
                let thinking_level_map = get_effort_thinking_level_map(&options).or_else(|| {
                    QWEN_TOKEN_PLAN_REASONING_EFFORT_FALLBACK.contains(&id.as_str())
                        .then(|| {
                            serde_json::json!({
                                "minimal": null, "low": null, "medium": null,
                                "high": "high", "xhigh": null, "max": "max"
                            })
                        })
                });
                let compat = qwen_compat().map(|c| c.reasoning_effort(thinking_level_map.is_some()));
                let mut entry = build_model(id, model, "openai-completions", provider, base_url, compat);
                set_thinking_level_map(&mut entry, thinking_level_map);
                models.push(entry);
            }
        }
    }

    // ---- Moonshot AI（kimi-k3 有特殊 compat） ----
    for (key, provider, base_url) in [
        ("moonshotai", "moonshotai", "https://api.moonshot.ai/v1"),
        ("moonshotai-cn", "moonshotai-cn", "https://api.moonshot.cn/v1"),
    ] {
        let Some(items) = models_object(data, key) else {
            continue;
        };
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            let mut compat = Compat::new()
                .store(false)
                .developer_role(false)
                .reasoning_effort(false)
                .max_tokens_field("max_tokens")
                .strict_mode(false)
                .thinking_format("deepseek");
            if id == "kimi-k3" {
                compat = compat
                    .requires_reasoning_content(true)
                    .set("deferredToolsMode", "kimi")
                    .thinking_format("openai")
                    .reasoning_effort(true);
            }
            let mut entry = build_model(id, model, "openai-completions", provider, base_url, Some(compat));
            if id == "kimi-k3" {
                entry.reasoning = true;
            }
            models.push(entry);
        }
    }

    // ---- Google（flash-latest 别名沿用被指向模型的元数据） ----
    if let Some(items) = models_object(data, "google") {
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            let source = match id.as_str() {
                "gemini-flash-latest" => items.get("gemini-3.5-flash").unwrap_or(model),
                "gemini-flash-lite-latest" => items.get("gemini-3.1-flash-lite").unwrap_or(model),
                _ => model,
            };
            let mut entry = build_model(
                id,
                source,
                "google-generative-ai",
                "google",
                "https://generativelanguage.googleapis.com/v1beta",
                openai_compat(),
            );
            entry.name = model_name(model, id);
            entry.reasoning_options = source.get("reasoning_options").cloned();
            models.push(entry);
        }
    }

    // ---- Mistral（cacheRead 缺省为 input×0.1） ----
    if let Some(items) = models_object(data, "mistral") {
        for (id, model) in items {
            if !is_tool_call(model) {
                continue;
            }
            let mut entry = build_model(
                id,
                model,
                "mistral-conversations",
                "mistral",
                "https://api.mistral.ai",
                None,
            );
            if model
                .get("cost")
                .and_then(|c| c.get("cache_read"))
                .is_none()
                && entry.cost.input > 0.0
            {
                entry.cost.cache_read = round_cost(entry.cost.input * 0.1);
            }
            models.push(entry);
        }
    }

    // ---- xAI（Responses API，固定 compat） ----
    if let Some(items) = models_object(data, "xai") {
        for (id, model) in items {
            if !is_tool_call(model) || is_deprecated(model) {
                continue;
            }
            let entry = build_model(
                id,
                model,
                "openai-responses",
                "xai",
                "https://api.x.ai/v1",
                Some(Compat::new().long_cache_retention(false)),
            );
            models.push(entry);
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
            let reasoning = model_reasoning(model);
            let mut entry = build_model(
                id,
                model,
                "openai-completions",
                "together",
                TOGETHER_BASE_URL,
                Some(get_together_compat(id, reasoning)),
            );
            set_thinking_level_map(&mut entry, get_together_thinking_level_map(id, reasoning));
            models.push(entry);
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

    models
}

/// TS `azureOpenAiModels`: clone the (already-overridden) `openai` Responses
/// models into `azure-openai-responses`.
fn clone_azure(models: &[BuildModel]) -> Vec<BuildModel> {
    let azure_overrides: HashMap<&str, u64> = [
        ("gpt-5.4", 1_050_000_u64),
        ("gpt-5.5", 1_050_000),
        ("gpt-5.6-luna", 1_050_000),
        ("gpt-5.6-sol", 1_050_000),
        ("gpt-5.6-terra", 1_050_000),
    ]
    .into_iter()
    .collect();
    models
        .iter()
        .filter(|m| m.provider == "openai" && m.api == "openai-responses")
        .map(|m| {
            let mut clone = m.clone();
            clone.api = "azure-openai-responses".into();
            clone.provider = "azure-openai-responses".into();
            clone.base_url = String::new();
            // TS keys reasoning options by provider:id, so the Azure clone
            // (different provider) has none.
            clone.reasoning_options = None;
            // TS rebuilds the cost object, dropping `tiers`.
            clone.cost.tiers = Vec::new();
            if let Some(ctx) = azure_overrides.get(m.id.as_str()) {
                clone.context_window = *ctx;
            }
            clone
        })
        .collect()
}

/// Hand-curated entries that models.dev does not provide (or that pi curates
/// explicitly). Mirrors the ant-ling / openai-codex / missing-OpenAI / DeepSeek
/// / Mistral Medium 3.5 additions in the original script.
fn manual_models() -> Vec<BuildModel> {
    let mut models = Vec::new();

    let ant_ling_compat = Compat::new()
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
        let mut entry = manual_model(
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
        );
        // TS wraps most Codex costs in long-context pricing, but not
        // `gpt-5.3-codex-spark` (explicit plain cost object).
        if id != "gpt-5.3-codex-spark" {
            entry.cost = with_openai_long_context_pricing(&entry.cost);
        }
        models.push(entry);
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
    let deepseek_compat = Compat::new()
        .requires_reasoning_content(true)
        .thinking_format("deepseek");
    let mut deepseek_flash = manual_model(
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
    );
    deepseek_flash.thinking_level_map = Some(serde_json::json!({
        "minimal": null, "low": "low", "medium": null, "high": "high", "max": "max"
    }));
    models.push(deepseek_flash);
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
    compat: Option<Compat>,
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
            tiers: Vec::new(),
        },
        context_window,
        max_tokens,
        headers: None,
        compat,
        reasoning_options: None,
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

/// TS `getOpenRouterThinkingLevelMap`.
fn openrouter_thinking_level_map(reasoning: Option<&OpenRouterReasoning>) -> Option<serde_json::Value> {
    let reasoning = reasoning?;
    let efforts = reasoning.supported_efforts.clone().unwrap_or_default();
    if efforts.is_empty() {
        return (reasoning.mandatory == Some(true)).then(|| serde_json::json!({ "off": null }));
    }
    let options = serde_json::json!([{ "type": "effort", "values": efforts }]);
    match get_effort_thinking_level_map(&options) {
        Some(serde_json::Value::Object(mut map)) => {
            map.insert(
                "off".into(),
                if reasoning.mandatory == Some(true) {
                    serde_json::Value::Null
                } else {
                    serde_json::json!("none")
                },
            );
            Some(serde_json::Value::Object(map))
        }
        _ => (reasoning.mandatory == Some(true)).then(|| serde_json::json!({ "off": null })),
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
            let use_anthropic_messages =
                m.id.starts_with("anthropic/") && !m.id.ends_with(":batch");
            BuildModel {
                id: m.id.clone(),
                name: m.name,
                api: if use_anthropic_messages {
                    "anthropic-messages".into()
                } else {
                    "openai-completions".into()
                },
                provider: "openrouter".into(),
                base_url: if use_anthropic_messages {
                    "https://openrouter.ai/api".into()
                } else {
                    "https://openrouter.ai/api/v1".into()
                },
                reasoning,
                thinking_level_map: openrouter_thinking_level_map(m.reasoning.as_ref()),
                input: input_modalities,
                cost: BuildModelCost {
                    input: round_cost(
                        parse_price(&m.pricing.as_ref().and_then(|p| p.prompt.clone()))
                            * 1_000_000.0,
                    ),
                    output: round_cost(
                        parse_price(&m.pricing.as_ref().and_then(|p| p.completion.clone()))
                            * 1_000_000.0,
                    ),
                    cache_read: round_cost(
                        parse_price(
                            &m.pricing.as_ref().and_then(|p| p.input_cache_read.clone()),
                        ) * 1_000_000.0,
                    ),
                    cache_write: round_cost(
                        parse_price(
                            &m.pricing.as_ref().and_then(|p| p.input_cache_write.clone()),
                        ) * 1_000_000.0,
                    ),
                    tiers: Vec::new(),
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
                compat: None,
                reasoning_options: None,
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
            None,
        ));
    }

    // 覆盖修正 → Azure 镜像 → 深层元数据（对齐 TS 顺序）。
    apply_overrides_all(&mut all_models);
    let azure = clone_azure(&all_models);
    all_models.extend(azure);
    apply_metadata(&mut all_models);

    // 按 (provider, id) 排序，保证两次运行字节级一致，可复现、可 diff
    all_models.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));

    let mut by_provider: BTreeMap<String, BTreeMap<String, &BuildModel>> = BTreeMap::new();
    for model in &all_models {
        // First occurrence wins (models.dev priority over manual/extra
        // sources), matching the original script's dedup.
        by_provider
            .entry(model.provider.clone())
            .or_default()
            .entry(model.id.clone())
            .or_insert(model);
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


// ============================================================================
// 深层元数据管线（对齐 generate-models.ts 的 apply* 系列）
// ============================================================================

const THINKING_LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
const OPENAI_LONG_CONTEXT_INPUT_THRESHOLD: u64 = 272_000;
const KIMI_K3_MAX_TOKENS: u64 = 131_072;

fn b(value: bool) -> serde_json::Value {
    serde_json::Value::Bool(value)
}

fn thinking_map_mut(model: &mut BuildModel) -> &mut serde_json::Map<String, serde_json::Value> {
    if model.thinking_level_map.is_none() {
        model.thinking_level_map = Some(serde_json::json!({}));
    }
    match model.thinking_level_map.as_mut() {
        Some(serde_json::Value::Object(map)) => map,
        _ => unreachable!("thinking_level_map is always an object"),
    }
}

fn merge_thinking(model: &mut BuildModel, pairs: &[(&str, serde_json::Value)]) {
    let map = thinking_map_mut(model);
    for (key, value) in pairs {
        map.insert((*key).to_string(), value.clone());
    }
}

fn merge_compat_value(model: &mut BuildModel, value: serde_json::Value) {
    model.compat.get_or_insert_with(Compat::new).merge(value);
}

fn merge_compat(model: &mut BuildModel, pairs: &[(&str, serde_json::Value)]) {
    let compat = model.compat.get_or_insert_with(Compat::new);
    for (key, value) in pairs {
        compat.insert(key, value.clone());
    }
}

/// TS `getEffortThinkingLevelMap`.
fn get_effort_thinking_level_map(options: &serde_json::Value) -> Option<serde_json::Value> {
    let mut effort_values: Vec<serde_json::Value> = Vec::new();
    if let Some(items) = options.as_array() {
        for option in items {
            if option.get("type").and_then(serde_json::Value::as_str) == Some("effort") {
                if let Some(values) = option.get("values").and_then(serde_json::Value::as_array) {
                    effort_values.extend(values.iter().cloned());
                }
            }
        }
    }
    if effort_values.is_empty() {
        return None;
    }
    let has = |name: &str| effort_values.iter().any(|v| v.as_str() == Some(name));
    if !THINKING_LEVELS.iter().any(|level| has(level)) && !has("none") {
        return None;
    }
    let mut map = serde_json::Map::new();
    map.insert(
        "off".into(),
        if has("none") {
            serde_json::json!("none")
        } else {
            serde_json::Value::Null
        },
    );
    for level in THINKING_LEVELS {
        map.insert(
            level.into(),
            if has(level) {
                serde_json::json!(level)
            } else {
                serde_json::Value::Null
            },
        );
    }
    Some(serde_json::Value::Object(map))
}

fn openai_completions_default_compat() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({
        "supportsStore": true,
        "supportsDeveloperRole": true,
        "supportsReasoningEffort": true,
        "supportsUsageInStreaming": true,
        "supportsFinishReason": true,
        "maxTokensField": "max_completion_tokens",
        "requiresToolResultName": false,
        "requiresAssistantAfterToolResult": false,
        "requiresThinkingAsText": false,
        "requiresReasoningContentOnAssistantMessages": false,
        "thinkingFormat": "openai",
        "openRouterRouting": {},
        "vercelGatewayRouting": {},
        "chatTemplateKwargs": {},
        "chatTemplateArgs": {},
        "zaiToolStream": false,
        "supportsStrictMode": true,
        "supportsOpenAIGrammarTools": false,
        "sendSessionAffinityHeaders": false,
        "supportsLongCacheRetention": true,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

fn detect_openai_completions_compat(model: &BuildModel) -> serde_json::Map<String, serde_json::Value> {
    let provider = model.provider.as_str();
    let base_url = model.base_url.as_str();
    let id = model.id.as_str();

    let is_zai = matches!(provider, "zai" | "zai-coding-cn")
        || base_url.contains("api.z.ai")
        || base_url.contains("open.bigmodel.cn");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot =
        matches!(provider, "moonshotai" | "moonshotai-cn") || base_url.contains("api.moonshot.");
    let is_openrouter = provider == "openrouter" || base_url.contains("openrouter.ai");
    let is_cloudflare_workers =
        provider == "cloudflare-workers-ai" || base_url.contains("api.cloudflare.com");
    let is_cloudflare_gateway =
        provider == "cloudflare-ai-gateway" || base_url.contains("gateway.ai.cloudflare.com");
    let is_nvidia = provider == "nvidia" || base_url.contains("integrate.api.nvidia.com");
    let is_ant_ling = provider == "ant-ling" || base_url.contains("api.ant-ling.com");
    let is_deepseek = provider == "deepseek" || base_url.to_lowercase().contains("deepseek.com");
    let is_together_reasoning_only =
        is_together && matches!(id, "deepseek-ai/DeepSeek-R1" | "MiniMaxAI/MiniMax-M2.7");

    let is_non_standard = is_nvidia
        || provider == "cerebras"
        || base_url.contains("cerebras.ai")
        || provider == "xai"
        || base_url.contains("api.x.ai")
        || is_together
        || base_url.contains("chutes.ai")
        || is_deepseek
        || is_zai
        || is_moonshot
        || provider == "opencode"
        || base_url.contains("opencode.ai")
        || is_cloudflare_workers
        || is_cloudflare_gateway
        || is_ant_ling;
    let use_max_tokens = base_url.contains("chutes.ai")
        || is_deepseek
        || is_moonshot
        || is_cloudflare_gateway
        || is_together
        || is_nvidia
        || is_ant_ling
        || is_zai;
    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_openrouter_developer_role =
        is_openrouter && (id.starts_with("anthropic/") || id.starts_with("openai/"));
    let cache_control_format = (provider == "openrouter"
        && (id.starts_with("anthropic/") || id.starts_with("~anthropic/")))
    .then_some(serde_json::json!("anthropic"));

    let mut compat = openai_completions_default_compat();
    compat.insert("supportsStore".into(), b(!is_non_standard));
    compat.insert(
        "supportsDeveloperRole".into(),
        b(is_openrouter_developer_role || (!is_non_standard && !is_openrouter)),
    );
    compat.insert(
        "supportsReasoningEffort".into(),
        b(!is_grok
            && !is_zai
            && !is_moonshot
            && !is_together
            && !is_cloudflare_gateway
            && !is_nvidia
            && !is_ant_ling),
    );
    compat.insert(
        "maxTokensField".into(),
        serde_json::json!(if use_max_tokens {
            "max_tokens"
        } else {
            "max_completion_tokens"
        }),
    );
    compat.insert(
        "requiresReasoningContentOnAssistantMessages".into(),
        b(is_deepseek),
    );
    compat.insert(
        "thinkingFormat".into(),
        serde_json::json!(if is_deepseek {
            "deepseek"
        } else if is_zai {
            "zai"
        } else if is_together && !is_together_reasoning_only {
            "together"
        } else if is_ant_ling {
            "ant-ling"
        } else if is_openrouter {
            "openrouter"
        } else {
            "openai"
        }),
    );
    compat.insert(
        "supportsStrictMode".into(),
        b(!is_moonshot && !is_together && !is_cloudflare_gateway && !is_nvidia),
    );
    if let Some(value) = cache_control_format {
        compat.insert("cacheControlFormat".into(), value);
    }
    compat.insert("sendSessionAffinityHeaders".into(), b(is_openrouter));
    compat.insert(
        "supportsLongCacheRetention".into(),
        b(!(is_together
            || is_cloudflare_workers
            || is_cloudflare_gateway
            || is_nvidia
            || is_ant_ling)),
    );
    compat
}

fn compat_delta(
    detected: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let defaults = openai_completions_default_compat();
    let mut delta = serde_json::Map::new();
    for (key, value) in detected {
        let default_value = defaults.get(&key);
        let is_empty = |v: Option<&serde_json::Value>| {
            v.is_some_and(|v| v.as_object().is_some_and(serde_json::Map::is_empty))
        };
        if is_empty(Some(&value)) && is_empty(default_value) {
            continue;
        }
        if default_value != Some(&value) {
            delta.insert(key, value);
        }
    }
    delta
}

fn apply_openai_completions_compat_metadata(model: &mut BuildModel) {
    if model.api != "openai-completions" {
        return;
    }
    let detected = compat_delta(detect_openai_completions_compat(model));
    let mut merged = Compat(detected);
    if let Some(existing) = &model.compat {
        merged.merge(existing.to_value());
    }
    model.compat = if merged.is_empty() { None } else { Some(merged) };
}

fn supports_direct_reasoning_effort(model: &BuildModel) -> bool {
    match model.api.as_str() {
        "anthropic-messages" => model
            .compat
            .as_ref()
            .is_some_and(|c| c.get_bool("forceAdaptiveThinking")),
        "openai-responses" | "azure-openai-responses" | "openai-codex-responses" => true,
        "openai-completions" => {
            let mut merged = Compat(detect_openai_completions_compat(model));
            if let Some(existing) = &model.compat {
                merged.merge(existing.to_value());
            }
            let thinking = merged.get("thinkingFormat").and_then(serde_json::Value::as_str);
            thinking == Some("openai") && merged.get_bool("supportsReasoningEffort")
        }
        _ => false,
    }
}

fn apply_models_dev_reasoning_option_metadata(model: &mut BuildModel) {
    let Some(options) = model.reasoning_options.clone() else {
        return;
    };
    if !supports_direct_reasoning_effort(model) {
        return;
    }
    if let Some(serde_json::Value::Object(map)) = get_effort_thinking_level_map(&options) {
        let target = thinking_map_mut(model);
        for (key, value) in map {
            target.insert(key, value);
        }
    }
}

const VERIFIED_ANTHROPIC_MID_CONVO_EFFORT_PROVIDERS: &[&str] = &["anthropic", "openrouter"];
const EAGER_TOOL_INPUT_STREAMING_UNSUPPORTED_ANTHROPIC_MODELS: &[&str] = &[
    "github-copilot:claude-haiku-4.5",
    "github-copilot:claude-sonnet-4",
    "github-copilot:claude-sonnet-4.5",
];

fn supports_anthropic_mid_convo_effort(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    let id = id.strip_prefix('~').unwrap_or(&id);
    let id = id.strip_prefix("anthropic/").unwrap_or(id);
    let is_opus_5 = id == "claude-opus-5"
        || (id.starts_with("claude-opus-5-")
            && id.len() == "claude-opus-5-".len() + 8
            && id["claude-opus-5-".len()..].chars().all(|c| c.is_ascii_digit()));
    let is_fable_5_1 = (id.starts_with("claude-fable-5") || id.starts_with("claude-mythos-5"))
        && (id.contains(".1") || id.contains("-1"));
    is_opus_5 || is_fable_5_1
}

fn get_anthropic_messages_compat(provider: &str, model_id: &str) -> Option<serde_json::Value> {
    let mut compat = serde_json::Map::new();
    if VERIFIED_ANTHROPIC_MID_CONVO_EFFORT_PROVIDERS.contains(&provider)
        && supports_anthropic_mid_convo_effort(model_id)
    {
        compat.insert("supportsMidConvoEffort".into(), b(true));
    }
    if EAGER_TOOL_INPUT_STREAMING_UNSUPPORTED_ANTHROPIC_MODELS
        .contains(&format!("{provider}:{model_id}").as_str())
    {
        compat.insert("supportsEagerToolInputStreaming".into(), b(false));
    }
    if provider == "xiaomi" || provider.starts_with("xiaomi-token-plan-") {
        compat.insert("allowEmptySignature".into(), b(true));
    }
    if compat.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(compat))
    }
}

fn apply_anthropic_messages_compat_metadata(model: &mut BuildModel) {
    if model.api != "anthropic-messages" {
        return;
    }
    let Some(compat) = get_anthropic_messages_compat(&model.provider, &model.id) else {
        return;
    };
    let supports_mid_convo = compat
        .get("supportsMidConvoEffort")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    merge_compat_value(model, compat);
    if supports_mid_convo {
        merge_thinking(model, &[("off", serde_json::Value::Null)]);
    }
}

fn is_anthropic_adaptive_thinking_model(model_id: &str) -> bool {
    [
        "opus-4-6", "opus-4.6", "opus-4-7", "opus-4.7", "opus-4-8", "opus-4.8", "opus-5", "opus.5",
        "sonnet-4-6", "sonnet-4.6", "sonnet-5", "sonnet.5", "fable-5", "mythos-5",
    ]
    .iter()
    .any(|needle| model_id.contains(needle))
}

fn is_anthropic_temperature_unsupported_model(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    ["opus-4-7", "opus-4.7", "opus-4-8", "opus-4.8", "opus-5", "opus.5"]
        .iter()
        .any(|needle| id.contains(needle))
}

fn supports_open_ai_xhigh(model_id: &str) -> bool {
    ["gpt-5.2", "gpt-5.3", "gpt-5.4", "gpt-5.5", "gpt-5.6", "gpt-6-astra"]
        .iter()
        .any(|needle| model_id.contains(needle))
}

fn supports_open_ai_max(model: &BuildModel) -> bool {
    (model.id.contains("gpt-5.6") || model.id.contains("gpt-6-astra"))
        && matches!(
            model.api.as_str(),
            "openai-responses"
                | "azure-openai-responses"
                | "openai-codex-responses"
                | "openai-completions"
        )
}

fn is_google_thinking_api(model: &BuildModel) -> bool {
    model.api == "google-generative-ai" || model.api == "google-vertex"
}

fn is_gemini3_pro(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.contains("gemini-3") && id.contains("-pro")
}

fn is_gemini3_flash(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    (id.contains("gemini-3") && id.contains("-flash"))
        || id == "gemini-flash-latest"
        || id == "gemini-flash-lite-latest"
}

fn is_gemma4(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.contains("gemma4") || id.contains("gemma-4")
}

fn thinking_values<'a>(
    pairs: &'a [(&'a str, Option<&str>)],
) -> Vec<(&'a str, serde_json::Value)> {
    pairs
        .iter()
        .map(|(key, value)| (*key, value.map_or(serde_json::Value::Null, |v| serde_json::json!(v))))
        .collect()
}

fn openai_responses_none_reasoning(model_id: &str) -> bool {
    [
        "gpt-5.1",
        "gpt-5.2",
        "gpt-5.3-codex",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.4-nano",
        "gpt-5.5",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
    ]
    .contains(&model_id)
}

fn apply_thinking_level_metadata(model: &mut BuildModel) {
    let id = model.id.clone();
    let provider = model.provider.clone();
    let api = model.api.clone();

    if (api == "openai-responses" || api == "azure-openai-responses") && id.starts_with("gpt-5") {
        merge_thinking(model, &[("off", serde_json::Value::Null)]);
    }
    if id == "gpt-6-astra"
        && matches!(
            api.as_str(),
            "openai-responses" | "azure-openai-responses" | "openai-codex-responses"
        )
    {
        merge_thinking(
            model,
            &thinking_values(&[
                ("off", None),
                ("minimal", None),
                ("low", Some("low")),
                ("medium", Some("medium")),
                ("high", Some("high")),
                ("xhigh", Some("xhigh")),
                ("max", Some("max")),
            ]),
        );
    }
    if provider == "github-copilot" && id.starts_with("gpt-5") {
        merge_thinking(model, &[("minimal", serde_json::json!("low"))]);
    }
    if api == "openai-responses" && provider == "openai" && openai_responses_none_reasoning(&id) {
        merge_thinking(model, &[("off", serde_json::json!("none"))]);
    }
    if provider == "xai" && api == "openai-responses" && model.thinking_level_map.is_none() {
        merge_thinking(
            model,
            &[("off", serde_json::Value::Null), ("minimal", serde_json::Value::Null)],
        );
    }
    if supports_open_ai_xhigh(&id) {
        merge_thinking(model, &[("xhigh", serde_json::json!("xhigh"))]);
    }
    if supports_open_ai_max(model) {
        merge_thinking(model, &[("max", serde_json::json!("max"))]);
    }
    if provider == "openai" && id == "gpt-5.5" {
        merge_thinking(model, &[("minimal", serde_json::Value::Null)]);
    }
    if id.ends_with("gpt-5.5-pro") {
        merge_thinking(
            model,
            &[
                ("off", serde_json::Value::Null),
                ("minimal", serde_json::Value::Null),
                ("low", serde_json::Value::Null),
            ],
        );
    }
    if ["opus-4-6", "opus-4.6", "sonnet-4-6", "sonnet-4.6"]
        .iter()
        .any(|n| id.contains(n))
    {
        merge_thinking(model, &[("max", serde_json::json!("max"))]);
    }
    if [
        "opus-4-7", "opus-4.7", "opus-4-8", "opus-4.8", "opus-5", "opus.5", "sonnet-5", "sonnet.5",
    ]
    .iter()
    .any(|n| id.contains(n))
    {
        merge_thinking(
            model,
            &[("xhigh", serde_json::json!("xhigh")), ("max", serde_json::json!("max"))],
        );
    }
    if id.contains("fable-5") {
        merge_thinking(
            model,
            &[
                ("off", serde_json::Value::Null),
                ("xhigh", serde_json::json!("xhigh")),
                ("max", serde_json::json!("max")),
            ],
        );
    }
    if api == "anthropic-messages" && is_anthropic_adaptive_thinking_model(&id) {
        merge_compat(model, &[("forceAdaptiveThinking", b(true))]);
    }
    if api == "anthropic-messages" && is_anthropic_temperature_unsupported_model(&id) {
        merge_compat(model, &[("supportsTemperature", b(false))]);
    }
    if api == "openai-completions" && id.contains("deepseek-v4") {
        let map = if provider == "openrouter" {
            thinking_values(&[
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("xhigh", Some("xhigh")),
                ("max", None),
            ])
        } else if (provider == "deepseek" || provider == "opencode" || provider == "opencode-go")
            && id.contains("deepseek-v4-flash")
        {
            thinking_values(&[
                ("minimal", None),
                ("low", Some("low")),
                ("medium", None),
                ("high", Some("high")),
                ("max", Some("max")),
            ])
        } else {
            thinking_values(&[
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("max", Some("max")),
            ])
        };
        merge_thinking(model, &map);
    }
    if is_google_thinking_api(model) && is_gemini3_pro(&id) {
        merge_thinking(
            model,
            &[
                ("off", serde_json::Value::Null),
                ("minimal", serde_json::Value::Null),
                ("low", serde_json::json!("LOW")),
                ("medium", serde_json::Value::Null),
                ("high", serde_json::json!("HIGH")),
            ],
        );
    }
    if is_google_thinking_api(model) && is_gemini3_flash(&id) {
        merge_thinking(model, &[("off", serde_json::Value::Null)]);
    }
    if is_google_thinking_api(model) && is_gemma4(&id) {
        merge_thinking(
            model,
            &[
                ("off", serde_json::Value::Null),
                ("minimal", serde_json::json!("MINIMAL")),
                ("low", serde_json::Value::Null),
                ("medium", serde_json::Value::Null),
                ("high", serde_json::json!("HIGH")),
            ],
        );
    }
    if provider == "groq" && id == "qwen/qwen3.6-27b" {
        merge_thinking(
            model,
            &[
                ("minimal", serde_json::Value::Null),
                ("low", serde_json::Value::Null),
                ("medium", serde_json::Value::Null),
                ("high", serde_json::json!("default")),
            ],
        );
    }
    if provider == "openai-codex" && supports_open_ai_xhigh(&id) {
        merge_thinking(model, &[("minimal", serde_json::json!("low"))]);
    }
    if matches!(provider.as_str(), "moonshotai" | "moonshotai-cn")
        && matches!(id.as_str(), "kimi-k2.7-code" | "kimi-k2.7-code-highspeed")
    {
        merge_thinking(model, &[("off", serde_json::Value::Null)]);
    }
    if provider == "openrouter" && id.starts_with("inception/mercury-2") {
        merge_thinking(model, &[("off", serde_json::Value::Null)]);
    }
    if provider == "openrouter" && id == "z-ai/glm-5.2" {
        merge_thinking(model, &[("xhigh", serde_json::json!("xhigh"))]);
    }
    if provider == "fireworks" {
        if api == "anthropic-messages" && model.compat.as_ref().is_some_and(|c| c.get_bool("forceAdaptiveThinking")) {
            if id == "accounts/fireworks/models/qwen3p8-max" && model.thinking_level_map.is_none() {
                model.thinking_level_map = get_effort_thinking_level_map(&serde_json::json!([
                    { "type": "effort", "values": ["low", "medium", "xhigh"] }
                ]));
            }
            let has_toggle = model
                .reasoning_options
                .as_ref()
                .and_then(serde_json::Value::as_array)
                .is_some_and(|opts| {
                    opts.iter()
                        .any(|o| o.get("type").and_then(serde_json::Value::as_str) == Some("toggle"))
                })
                || id == "accounts/fireworks/models/qwen3p8-2p4t-a95b";
            if has_toggle {
                merge_thinking(model, &[("off", serde_json::json!("none"))]);
            }
            if id == "accounts/fireworks/models/deepseek-v4-pro-0813" {
                merge_thinking(model, &[("low", serde_json::json!("low"))]);
            }
        }
        if id.contains("glm-5p2") {
            merge_thinking(
                model,
                &[
                    ("off", serde_json::json!("none")),
                    ("minimal", serde_json::Value::Null),
                    ("low", serde_json::Value::Null),
                    ("medium", serde_json::Value::Null),
                    ("max", serde_json::json!("max")),
                ],
            );
        }
        if id.contains("kimi-k3") {
            merge_thinking(model, &[("medium", serde_json::Value::Null)]);
        }
    }
    if provider == "opencode-go" && id == "glm-5.2" {
        merge_thinking(
            model,
            &thinking_values(&[
                ("off", None),
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("max", Some("max")),
            ]),
        );
    }
    if provider == "opencode-go" && id == "kimi-k2.6" {
        merge_thinking(
            model,
            &[
                ("minimal", serde_json::Value::Null),
                ("low", serde_json::Value::Null),
                ("medium", serde_json::Value::Null),
            ],
        );
    }
    if provider == "opencode" && id == "grok-build-0.1" {
        merge_thinking(
            model,
            &[
                ("off", serde_json::Value::Null),
                ("minimal", serde_json::Value::Null),
                ("low", serde_json::Value::Null),
                ("medium", serde_json::Value::Null),
            ],
        );
    }
    if provider == "ant-ling" && model.reasoning {
        merge_thinking(
            model,
            &thinking_values(&[
                ("off", None),
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("xhigh", Some("xhigh")),
            ]),
        );
    }
    if provider == "github-copilot" {
        type ThinkingOverride = (&'static str, &'static [(&'static str, Option<&'static str>)]);
        let overrides: &[ThinkingOverride] = &[
            ("claude-opus-4.7", &[("minimal", Some("low"))]),
            ("claude-opus-4.8", &[("minimal", Some("low"))]),
            ("claude-opus-5", &[("minimal", Some("low"))]),
            ("claude-sonnet-4.6", &[("minimal", Some("low")), ("max", Some("max"))]),
        ];
        if let Some((_, pairs)) = overrides.iter().find(|(model_id, _)| *model_id == id) {
            merge_thinking(model, &thinking_values(pairs));
        }
    }
}

fn apply_strict_tool_compat_metadata(model: &mut BuildModel) {
    if matches!(model.provider.as_str(), "openai" | "cloudflare-ai-gateway")
        && model.api == "openai-responses"
    {
        merge_compat(model, &[("supportsStrictMode", b(true))]);
    } else if model.provider == "anthropic" && model.api == "anthropic-messages" {
        merge_compat(model, &[("supportsStrictTools", b(true))]);
    }
}

const OPENAI_GRAMMAR_TOOL_PROVIDERS: &[&str] = &[
    "openai",
    "openai-codex",
    "azure-openai-responses",
    "github-copilot",
    "opencode",
    "cloudflare-ai-gateway",
];
const OPENAI_GRAMMAR_TOOL_APIS: &[&str] = &[
    "openai-responses",
    "azure-openai-responses",
    "openai-codex-responses",
];

fn apply_openai_grammar_tool_compat_metadata(model: &mut BuildModel) {
    if !OPENAI_GRAMMAR_TOOL_APIS.contains(&model.api.as_str())
        || !OPENAI_GRAMMAR_TOOL_PROVIDERS.contains(&model.provider.as_str())
    {
        return;
    }
    let Some(number) = model
        .id
        .strip_prefix("gpt-")
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse::<u32>().ok())
    else {
        return;
    };
    if number < 5 {
        return;
    }
    merge_compat(model, &[("supportsOpenAIGrammarTools", b(true))]);
}

const OPENAI_TOOL_SEARCH_MODEL_IDS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.4-pro",
    "gpt-5.5",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];
const OPENAI_CODEX_ADDITIONAL_TOOLS_MODEL_IDS: &[&str] =
    &["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-6-astra"];

fn apply_openai_tool_search_metadata(model: &mut BuildModel) {
    let is_openai_responses = model.provider == "openai" && model.api == "openai-responses";
    let is_openai_codex = model.provider == "openai-codex" && model.api == "openai-codex-responses";
    if !(is_openai_responses || is_openai_codex) || !OPENAI_TOOL_SEARCH_MODEL_IDS.contains(&model.id.as_str())
    {
        return;
    }
    let supports_additional = (is_openai_responses
        && OPENAI_TOOL_SEARCH_MODEL_IDS.contains(&model.id.as_str()))
        || (is_openai_codex && OPENAI_CODEX_ADDITIONAL_TOOLS_MODEL_IDS.contains(&model.id.as_str()));
    if supports_additional {
        merge_compat(model, &[("supportsAdditionalTools", b(true))]);
    }
    merge_compat(model, &[("supportsToolSearch", b(true))]);
}

fn apply_openai_explicit_prompt_cache_metadata(model: &mut BuildModel) {
    if model.provider != "openai" || model.api != "openai-responses" {
        return;
    }
    if model.cost.cache_write <= 0.0 {
        return;
    }
    merge_compat(model, &[("supportsExplicitPromptCacheMode", b(true))]);
}

const ANTHROPIC_ALLOWED_FALLBACK_MODELS: &[(&str, &[&str])] = &[
    ("claude-fable-5", &["claude-opus-4-8", "claude-opus-5"]),
    ("claude-opus-5", &["claude-opus-4-8"]),
];

fn apply_anthropic_allowed_fallback_model_metadata(models: &mut [BuildModel]) {
    let by_id: HashMap<String, (String, BuildModelCost, bool)> = models
        .iter()
        .filter(|m| m.provider == "anthropic" && m.api == "anthropic-messages")
        .map(|m| {
            (
                m.id.clone(),
                (
                    m.provider.clone(),
                    m.cost.clone(),
                    m.compat.as_ref().is_some_and(|c| c.get_bool("supportsMidConvoEffort")),
                ),
            )
        })
        .collect();

    for (model_id, fallback_ids) in ANTHROPIC_ALLOWED_FALLBACK_MODELS {
        let Some((_, _, supports_mid_convo)) = by_id.get(*model_id) else {
            continue;
        };
        let mut allowed = Vec::new();
        for fallback_id in *fallback_ids {
            if *supports_mid_convo && !supports_anthropic_mid_convo_effort(fallback_id) {
                continue;
            }
            if let Some((provider, cost, _)) = by_id.get(*fallback_id) {
                allowed.push(serde_json::json!({
                    "provider": provider,
                    "model": fallback_id,
                    "cost": cost,
                }));
            }
        }
        if allowed.is_empty() {
            continue;
        }
        if let Some(model) = models
            .iter_mut()
            .find(|m| m.provider == "anthropic" && m.api == "anthropic-messages" && m.id == *model_id)
        {
            merge_compat_value(model, serde_json::json!({ "allowedFallbackModels": allowed }));
        }
    }
}

fn with_openai_long_context_pricing(cost: &BuildModelCost) -> BuildModelCost {
    BuildModelCost {
        input: cost.input,
        output: cost.output,
        cache_read: cost.cache_read,
        cache_write: cost.cache_write,
        tiers: vec![BuildModelCostTier {
            input_tokens_above: OPENAI_LONG_CONTEXT_INPUT_THRESHOLD,
            input: round_cost(cost.input * 2.0),
            output: round_cost(cost.output * 1.5),
            cache_read: round_cost(cost.cache_read * 2.0),
            cache_write: round_cost(cost.cache_write * 2.0),
        }],
    }
}

fn openai_gpt_56_standard_cost(model_id: &str) -> Option<BuildModelCost> {
    match model_id {
        "gpt-5.6-luna" => Some(BuildModelCost {
            input: 0.2,
            output: 1.2,
            cache_read: 0.02,
            cache_write: 0.25,
            tiers: Vec::new(),
        }),
        "gpt-5.6-terra" => Some(BuildModelCost {
            input: 2.0,
            output: 12.0,
            cache_read: 0.2,
            cache_write: 2.5,
            tiers: Vec::new(),
        }),
        _ => None,
    }
}

const GITHUB_COPILOT_EXTENDED_CONTEXT_MODELS: &[&str] = &[
    "claude-fable-5",
    "claude-opus-4.6",
    "claude-opus-4.7",
    "claude-opus-4.8",
    "claude-opus-5",
    "claude-sonnet-4.6",
    "claude-sonnet-5",
    "gpt-5.3-codex",
    "gpt-5.4",
    "gpt-5.5",
];
const OPENAI_SHORT_CONTEXT_CAPPED_MODEL_IDS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.5",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];
const OPENAI_LONG_CONTEXT_PRICING_MODEL_IDS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.5",
    "gpt-5.5-pro",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-6-astra",
];
const OPENROUTER_KIMI_K3_MODEL_IDS: &[&str] = &["moonshotai/kimi-k3", "~moonshotai/kimi-latest"];

/// TS `generateModels`'s "temporary overrides until upstream metadata is
/// corrected" loop.
fn apply_overrides(model: &mut BuildModel) {
    let id = model.id.clone();
    let provider = model.provider.clone();

    if provider == "github-copilot" && GITHUB_COPILOT_EXTENDED_CONTEXT_MODELS.contains(&id.as_str()) {
        model.context_window = 1_000_000;
    }
    if matches!(provider.as_str(), "anthropic" | "opencode" | "opencode-go")
        && matches!(
            id.as_str(),
            "claude-opus-4-6" | "claude-sonnet-4-6" | "claude-opus-4.6" | "claude-sonnet-4.6"
        )
    {
        model.context_window = 1_000_000;
    }
    if matches!(provider.as_str(), "opencode" | "opencode-go")
        && matches!(id.as_str(), "claude-sonnet-4-5" | "claude-sonnet-4")
    {
        model.context_window = 200_000;
    }
    if matches!(provider.as_str(), "opencode" | "opencode-go") && id == "gpt-5.4" {
        model.context_window = 272_000;
        model.max_tokens = 128_000;
    }
    if provider == "openai" && OPENAI_SHORT_CONTEXT_CAPPED_MODEL_IDS.contains(&id.as_str()) {
        model.context_window = OPENAI_LONG_CONTEXT_INPUT_THRESHOLD;
        model.max_tokens = 128_000;
    }
    if provider == "openai" && OPENAI_LONG_CONTEXT_PRICING_MODEL_IDS.contains(&id.as_str()) {
        let cost = openai_gpt_56_standard_cost(&id).unwrap_or_else(|| model.cost.clone());
        model.cost = with_openai_long_context_pricing(&cost);
    }
    if provider == "cloudflare-ai-gateway" {
        if let Some(standard) = openai_gpt_56_standard_cost(&id) {
            model.cost = with_openai_long_context_pricing(&standard);
        }
    }
    if provider == "openai" && id == "gpt-5-pro" {
        model.max_tokens = 128_000;
    }
    if (provider == "openrouter" && OPENROUTER_KIMI_K3_MODEL_IDS.contains(&id.as_str()))
        || (provider == "vercel-ai-gateway" && id == "moonshotai/kimi-k3")
    {
        model.max_tokens = KIMI_K3_MAX_TOKENS;
    }
    if provider == "openrouter" && id == "moonshotai/kimi-k2.5" {
        model.cost.input = 0.41;
        model.cost.output = 2.06;
        model.cost.cache_read = 0.07;
        model.max_tokens = 4096;
    }
    if provider == "openrouter" && id.starts_with("moonshotai/kimi-k2.6") {
        merge_compat(
            model,
            &[
                ("supportsDeveloperRole", b(false)),
                ("requiresReasoningContentOnAssistantMessages", b(true)),
            ],
        );
    }
    if provider == "openrouter" && id == "z-ai/glm-5" {
        model.cost.input = 0.6;
        model.cost.output = 1.9;
        model.cost.cache_read = 0.119;
    }
    // DeepSeek V4 compat override across openai-completions providers.
    if model.api == "openai-completions"
        && id.contains("deepseek-v4")
        && !matches!(
            provider.as_str(),
            "qwen-token-plan" | "qwen-token-plan-cn" | "qwen-token-plan-individual"
        )
    {
        let preserves_native = matches!(provider.as_str(), "openrouter" | "opencode");
        if preserves_native {
            merge_compat(
                model,
                &[("requiresReasoningContentOnAssistantMessages", b(true))],
            );
        } else {
            merge_compat(
                model,
                &[
                    ("requiresReasoningContentOnAssistantMessages", b(true)),
                    ("thinkingFormat", serde_json::json!("deepseek")),
                ],
            );
        }
    }
}

/// Run the full metadata pipeline over the combined catalog (match the TS
/// `for (const model of allModels) { apply* }` block).
fn apply_overrides_all(models: &mut [BuildModel]) {
    for model in models.iter_mut() {
        apply_overrides(model);
    }
}

fn apply_metadata(models: &mut [BuildModel]) {
    for model in models.iter_mut() {
        apply_openai_completions_compat_metadata(model);
        apply_anthropic_messages_compat_metadata(model);
        apply_models_dev_reasoning_option_metadata(model);
        apply_thinking_level_metadata(model);
        apply_strict_tool_compat_metadata(model);
        apply_openai_grammar_tool_compat_metadata(model);
        apply_openai_tool_search_metadata(model);
        apply_openai_explicit_prompt_cache_metadata(model);
    }
    apply_anthropic_allowed_fallback_model_metadata(models);
}
