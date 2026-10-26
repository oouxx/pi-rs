//! pi-web-search — `web_search` / `web_fetch` 工具扩展。
//!
//! 支持两个后端：
//! - **DeepSeek Search/Fetch API**（`POST /v1/tools/search` /
//!   `/v1/tools/fetch`，Bearer 鉴权；请求/响应/错误码见
//!   <https://api-docs.deepseek.com/zh-cn/api/search> 与 `/api/fetch`）；
//! - **本地 Ollama 实验性 API**（原 `@ollama/pi-web-search` 行为）。
//!
//! 后端可配置（settings.json `webSearch`，见 [`WebSearchConfig`]），每一档
//! 的优先级都是「settings → 环境变量 → 内置默认」：
//!
//! | 配置项 | 环境变量 | 默认 |
//! | ------ | -------- | ---- |
//! | `webSearch.provider` | `PI_WEB_SEARCH_PROVIDER` | `auto`（有 DeepSeek 凭据用 DeepSeek，否则 Ollama） |
//! | `webSearch.deepseek.apiKey` | `DEEPSEEK_API_KEY` | 无（缺凭据时 `auto` 回落 Ollama） |
//! | `webSearch.deepseek.baseUrl` | `DEEPSEEK_BASE_URL` | `https://api.deepseek.com` |
//! | `webSearch.ollama.host` | `OLLAMA_HOST` / `OLLAMA_BASE_URL` | `http://localhost:11434` |
//!
//! 宿主（`create_agent_session`）还会把 `ModelRuntime` 解析到的 DeepSeek
//! 凭据（auth.json / models.json / 环境变量）填进 `apiKey`，因此
//! `pi /login deepseek` 之后无需再设环境变量。
//!
//! Ollama 路径的工具名、参数 schema、输出文本与 `details`、错误文本保持
//! 与原版一致（`max_results` 默认 5；401 → "Unauthorized. Run
//! `ollama signin`…"）。DeepSeek 复用同一套工具契约，差异见
//! `DEVIATIONS.md`。
use std::time::Duration;

use async_trait::async_trait;
use pi_extension_api::{
    ExtensionContext, HookHandler, ToolCallOutput, ToolDefinition, ToolRegistry,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::timeout;

/// 默认 Ollama 地址，与原版 `getOllamaHost()` 一致。
const DEFAULT_OLLAMA_HOST: &str = "http://localhost:11434";
/// DeepSeek API Base URL（Search/Fetch 端点由它派生出 `/v1/tools/...`）。
const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
/// 单次请求超时（原版无显式超时，依赖调用方 signal）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// 原版工具默认返回条数。
const DEFAULT_MAX_RESULTS: u64 = 5;
/// DeepSeek Search API 的 `count` 上限。
const DEEPSEEK_MAX_RESULTS: u64 = 50;
/// DeepSeek Search API 的 `query` 上限（Unicode 字符数）。
const DEEPSEEK_MAX_QUERY_CHARS: usize = 128;

const PROVIDER_ENV: &str = "PI_WEB_SEARCH_PROVIDER";
const DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";
const DEEPSEEK_BASE_URL_ENV: &str = "DEEPSEEK_BASE_URL";

// ============================================================================
// API 响应类型
// ============================================================================

/// 搜索结果的公共字段。DeepSeek 额外提供 `summary` / `snippet`，Ollama 只有
/// `content`；`body()` 负责选出最适合给模型阅读的一段。
#[derive(Debug, Clone, Deserialize)]
struct SearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    snippet: String,
}

impl SearchResult {
    /// DeepSeek 的 `summary`（约 600～800 字）面向模型阅读，优先使用；
    /// 缺失时依次回退到短摘要 `snippet` 和 `content`。
    fn body(&self) -> &str {
        let summary = self.summary.trim();
        if !summary.is_empty() {
            return summary;
        }
        let snippet = self.snippet.trim();
        if !snippet.is_empty() {
            return snippet;
        }
        self.content.trim()
    }
}

/// Ollama `web_fetch` 的响应；DeepSeek Fetch 不返回 `links`，反序列化为空数组。
#[derive(Debug, Clone, Deserialize)]
struct FetchResponse {
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    links: Vec<String>,
}

// ============================================================================
// 配置（settings.json `webSearch`）
// ============================================================================

/// `web_search` 后端配置，对应 settings.json 的 `webSearch` 段
/// （pi-rs 扩展；TS 原版无对应设置，只有本地 Ollama）。
///
/// ```json
/// {
///   "webSearch": {
///     "provider": "auto",
///     "deepseek": { "baseUrl": "https://api.deepseek.com", "apiKey": "${DEEPSEEK_API_KEY}" },
///     "ollama": { "host": "http://localhost:11434" }
///   }
/// }
/// ```
///
/// 每一档的优先级都是「settings → 环境变量 → 内置默认」，因此只用环境变量
/// 的既有用法不受影响。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct WebSearchConfig {
    /// `"auto"`（默认）/ `"deepseek"` / `"ollama"`。`auto` 时有 DeepSeek
    /// 凭据就用 DeepSeek，否则用 Ollama。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepseek: Option<DeepSeekConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama: Option<OllamaConfig>,
}

/// DeepSeek Search/Fetch 后端设置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct DeepSeekConfig {
    /// 覆盖 API base URL；接受 `https://api.deepseek.com` 或带 `/v1` 的写法。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// API Key；支持 `${ENV}` 模板（由宿主解析后传入）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// 本地 Ollama 后端设置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct OllamaConfig {
    /// Ollama 地址，默认 `http://localhost:11434`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

// ============================================================================
// 后端选择
// ============================================================================

#[derive(Debug, Clone)]
enum Backend {
    Ollama {
        host: String,
    },
    DeepSeek {
        base_url: String,
        api_key: String,
    },
    /// 配置无效（未知 provider、显式 DeepSeek 但缺 key）。保留错误信息，
    /// 在工具调用时返回，而不是静默回退到另一个后端。
    Invalid {
        message: String,
    },
}

/// 后端配置来源：settings 的 `webSearch` 段 → 环境变量 → 内置默认。
#[derive(Debug, Clone, Default)]
struct BackendSources {
    provider: Option<String>,
    deepseek_api_key: Option<String>,
    deepseek_base_url: Option<String>,
    ollama_host: Option<String>,
}

impl BackendSources {
    /// 合并 settings 与进程环境（settings 优先）。
    fn from_config(config: &WebSearchConfig) -> Self {
        let deepseek = config.deepseek.clone().unwrap_or_default();
        let ollama = config.ollama.clone().unwrap_or_default();
        Self {
            provider: non_empty(config.provider.clone()).or_else(|| env_non_empty(PROVIDER_ENV)),
            deepseek_api_key: non_empty(deepseek.api_key)
                .or_else(|| env_non_empty(DEEPSEEK_API_KEY_ENV)),
            deepseek_base_url: non_empty(deepseek.base_url)
                .or_else(|| env_non_empty(DEEPSEEK_BASE_URL_ENV)),
            ollama_host: non_empty(ollama.host)
                .or_else(|| env_non_empty("OLLAMA_HOST"))
                .or_else(|| env_non_empty("OLLAMA_BASE_URL")),
        }
    }

    fn resolve(&self) -> Backend {
        let source = if self.provider.is_some() {
            format!("{PROVIDER_ENV}/webSearch.provider")
        } else {
            "webSearch.provider".to_string()
        };
        let mode = self
            .provider
            .as_deref()
            .map(str::trim)
            .unwrap_or("auto")
            .to_ascii_lowercase();
        let api_key = non_empty(self.deepseek_api_key.clone());
        let base_url = self.deepseek_base_url.as_deref();
        let ollama_host = self.ollama_host.as_deref();
        match mode.as_str() {
            "" | "auto" => match api_key {
                Some(key) => Backend::deepseek(base_url, &key),
                None => Backend::ollama(ollama_host),
            },
            "deepseek" => match api_key {
                Some(key) => Backend::deepseek(base_url, &key),
                None => Backend::Invalid {
                    message: format!(
                        "DeepSeek backend selected ({source}) but no API key is configured. \
                         Set webSearch.deepseek.apiKey, {DEEPSEEK_API_KEY_ENV}, or log in with \
                         `pi /login deepseek`."
                    ),
                },
            },
            "ollama" => Backend::ollama(ollama_host),
            other => Backend::Invalid {
                message: format!(
                    "Unknown web search provider '{other}' (from {source}). Use 'auto', \
                     'deepseek', or 'ollama'."
                ),
            },
        }
    }
}

impl Backend {
    fn ollama(host: Option<&str>) -> Self {
        let host = host
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .unwrap_or(DEFAULT_OLLAMA_HOST)
            .trim_end_matches('/');
        Self::Ollama {
            host: host.to_string(),
        }
    }

    fn deepseek(base_url: Option<&str>, api_key: &str) -> Self {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Self::Invalid {
                message: format!(
                    "DeepSeek API key is empty. Set {DEEPSEEK_API_KEY_ENV} or configure \
                     DeepSeek auth in pi-rs."
                ),
            };
        }
        let base_url = base_url
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .unwrap_or(DEFAULT_DEEPSEEK_BASE_URL)
            .trim_end_matches('/');
        // 同时接受 `https://api.deepseek.com` 与 OpenAI 兼容的
        // `https://api.deepseek.com/v1` 两种写法。
        let base_url = base_url
            .strip_suffix("/v1")
            .unwrap_or(base_url)
            .trim_end_matches('/');
        Self::DeepSeek {
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        }
    }

    fn endpoint(&self, tool: &str) -> Result<String, String> {
        match self {
            Self::Ollama { host } => Ok(format!("{host}/api/experimental/{tool}")),
            Self::DeepSeek { base_url, .. } => {
                let action = match tool {
                    "web_search" => "search",
                    "web_fetch" => "fetch",
                    _ => return Err(format!("Unsupported web tool '{tool}'.")),
                };
                Ok(format!("{base_url}/v1/tools/{action}"))
            }
            Self::Invalid { message } => Err(message.clone()),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Ollama { .. } => "Ollama",
            Self::DeepSeek { .. } => "DeepSeek",
            Self::Invalid { .. } => "web search",
        }
    }

    fn location(&self) -> &str {
        match self {
            Self::Ollama { host } => host,
            Self::DeepSeek { base_url, .. } => base_url,
            Self::Invalid { .. } => "",
        }
    }

    fn is_deepseek(&self) -> bool {
        matches!(self, Self::DeepSeek { .. })
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Trimmed, non-empty value (`None` for blank strings).
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 从 DeepSeek 的 OpenAI 兼容错误体中提取 `error.message`。
fn deepseek_error_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let message = value.get("error")?.get("message")?.as_str()?.trim();
    (!message.is_empty()).then(|| message.to_string())
}

/// 把工具参数 `max_results` 映射为 DeepSeek 的 `count`（1～50）。
fn deepseek_count(max_results: &Value) -> Result<u64, String> {
    let Some(count) = max_results.as_f64() else {
        return Err("web_search max_results must be a number.".into());
    };
    if !count.is_finite() || count < 1.0 {
        return Err("web_search max_results must be at least 1.".into());
    }
    Ok((count.trunc() as u64).min(DEEPSEEK_MAX_RESULTS))
}

/// 按 DeepSeek Search API 的约束校验 `query`：1～128 个 Unicode 字符且不能
/// 全为空白（超限时 API 返回 400，这里提前给出可执行的提示）。
fn deepseek_query(query: &str) -> Result<&str, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("web_search query must not be blank.".into());
    }
    let length = query.chars().count();
    if length > DEEPSEEK_MAX_QUERY_CHARS {
        return Err(format!(
            "web_search query must be at most {DEEPSEEK_MAX_QUERY_CHARS} characters \
             (got {length})."
        ));
    }
    Ok(query)
}

/// DeepSeek 文档中按状态码给出的可执行建议（402/403/404/408/413/429/5xx）。
/// 其余状态码返回 `None`，由通用错误路径带上 API 的 `error.message`。
fn deepseek_status_hint(status: reqwest::StatusCode) -> Option<String> {
    let code = status.as_u16();
    Some(match code {
        402 => "DeepSeek account has insufficient balance (HTTP 402). Top up at \
                https://platform.deepseek.com and retry."
            .to_string(),
        403 => "DeepSeek Fetch was rejected by its safety policy (HTTP 403). Try a different \
                URL."
            .to_string(),
        404 => "DeepSeek Fetch could not retrieve usable page content (HTTP 404): the URL may \
                be unavailable, blocked, or have no extractable text."
            .to_string(),
        408 => "DeepSeek timed out reading the request body (HTTP 408). Retry after a short \
                delay."
            .to_string(),
        413 => "Request body exceeds DeepSeek's size limit (HTTP 413).".to_string(),
        429 => "DeepSeek rate limit exceeded (HTTP 429). Retry after a short delay.".to_string(),
        500..=599 => format!("DeepSeek server error (HTTP {code}). Retry after a short delay."),
        _ => return None,
    })
}

// ============================================================================
// WebSearchExtension
// ============================================================================

/// web_search / web_fetch 扩展：通过 DeepSeek Search API 或本地 Ollama
/// 提供实时搜索与网页抓取。
pub struct WebSearchExtension {
    backend: Backend,
    client: reqwest::Client,
    timeout: Duration,
}

impl WebSearchExtension {
    /// 只用环境变量创建扩展（`PI_WEB_SEARCH_PROVIDER` 等）。
    #[must_use]
    pub fn new() -> Self {
        Self::from_config(&WebSearchConfig::default())
    }

    /// 按 settings 的 `webSearch` 段创建扩展；未配置的项回落到环境变量与
    /// 内置默认。
    #[must_use]
    pub fn from_config(config: &WebSearchConfig) -> Self {
        Self {
            backend: BackendSources::from_config(config).resolve(),
            client: reqwest::Client::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// 强制使用指定 Ollama 地址（测试用；也可用于配置远程端点）。
    #[must_use]
    pub fn with_host(mut self, host: &str) -> Self {
        self.backend = Backend::ollama(Some(host));
        self
    }

    /// 强制使用 DeepSeek 后端（测试用；也可用于配置自定义网关）。
    #[must_use]
    pub fn with_deepseek(mut self, api_key: &str, base_url: &str) -> Self {
        self.backend = Backend::deepseek(Some(base_url), api_key);
        self
    }

    /// 指定单次请求超时（测试用）。
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// POST JSON 到当前后端，返回已解析的 JSON 值。
    ///
    /// 错误映射：
    /// - Ollama 连接失败 → 原版 `Could not connect to Ollama …` 文案
    /// - DeepSeek 连接失败 → 提示检查网络 / `DEEPSEEK_BASE_URL`
    /// - 401 → Ollama 提示 `ollama signin`；DeepSeek 提示设置 API key
    /// - 其他非 2xx → `{Tool} API error (status {N}): {detail}`
    /// - 超时 → 明确的超时消息（pi-rs 增强）
    async fn post(&self, tool: &str, body: Value) -> Result<Value, String> {
        let url = self.backend.endpoint(tool)?;
        let label = self.backend.label();
        let location = self.backend.location();

        let mut request = self.client.post(&url).json(&body);
        if let Backend::DeepSeek { api_key, .. } = &self.backend {
            request = request.bearer_auth(api_key);
        }

        let response = timeout(self.timeout, request.send())
            .await
            .map_err(|_| {
                let timeout_desc = if self.timeout.as_secs() >= 1 {
                    format!("{}s", self.timeout.as_secs())
                } else {
                    format!("{}ms", self.timeout.as_millis())
                };
                format!("{tool} request to {label} at {location} timed out after {timeout_desc}.")
            })?
            .map_err(|e| {
                if !e.is_connect() {
                    return format!("{tool} request to {label} at {location} failed: {e}");
                }
                if self.backend.is_deepseek() {
                    format!(
                        "Could not connect to DeepSeek at {location}. Check your network \
                         connection or {DEEPSEEK_BASE_URL_ENV}."
                    )
                } else {
                    format!(
                        "Could not connect to Ollama at {location}. Make sure Ollama is running \
                         and {tool} is enabled."
                    )
                }
            })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(if self.backend.is_deepseek() {
                format!(
                    "Unauthorized. DeepSeek API key is invalid or missing. Set \
                     {DEEPSEEK_API_KEY_ENV}."
                )
            } else {
                "Unauthorized. Run `ollama signin` to authenticate.".to_string()
            });
        }
        if !status.is_success() {
            let raw = response.text().await.unwrap_or_default().trim().to_string();
            if self.backend.is_deepseek() {
                let detail = deepseek_error_message(&raw);
                if let Some(hint) = deepseek_status_hint(status) {
                    // Keep the API's own message (it carries `request_id` on
                    // billing errors) alongside the actionable hint.
                    return Err(match detail {
                        Some(detail) => format!("{hint} (API: {detail})"),
                        None => hint,
                    });
                }
                let detail = detail.unwrap_or_else(|| {
                    if raw.is_empty() {
                        status
                            .canonical_reason()
                            .unwrap_or("unknown error")
                            .to_string()
                    } else {
                        raw
                    }
                });
                return Err(format!(
                    "{tool} API error (status {}): {detail}",
                    status.as_u16()
                ));
            }
            let detail = if raw.is_empty() {
                status
                    .canonical_reason()
                    .unwrap_or("unknown error")
                    .to_string()
            } else {
                raw
            };
            return Err(format!(
                "{tool} API error (status {}): {detail}",
                status.as_u16()
            ));
        }

        response
            .json::<Value>()
            .await
            .map_err(|e| format!("Failed to parse {tool} response from {label}: {e}"))
    }

    /// 执行搜索（对齐原版 web_search 工具的 execute）。
    ///
    /// DeepSeek 侧只发 `query` + `count`：`need_content` 保持默认 `false`
    /// （正文不计费差异，但 `summary` 本就是给模型读的长摘要，正文没有额外
    /// 价值）；`ori_question` / `localization` 是宿主侧信息，工具参数里没有
    /// 对应字段。
    async fn run_search(&self, query: &str, max_results: &Value) -> Result<ToolCallOutput, String> {
        let body = if self.backend.is_deepseek() {
            json!({
                "query": deepseek_query(query)?,
                "count": deepseek_count(max_results)?,
            })
        } else {
            json!({ "query": query, "max_results": max_results })
        };
        let data = self.post("web_search", body).await?;

        // 保留 API 原始结果（DeepSeek 还会返回 rerank_score / site_name /
        // date / authority 等字段），供 details 消费方使用。
        let raw_results = data.get("results").cloned().unwrap_or_else(|| json!([]));
        let parsed: Vec<SearchResult> = serde_json::from_value(raw_results.clone())
            .map_err(|e| format!("Failed to parse web_search response: {e}"))?;

        let formatted = if parsed.is_empty() {
            "No results found.".to_string()
        } else {
            parsed
                .iter()
                .enumerate()
                .map(|(i, r)| format!("{}. {}\n   URL: {}\n   {}", i + 1, r.title, r.url, r.body()))
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        Ok(text_response(formatted, json!({ "results": raw_results })))
    }

    /// 执行抓取：原版 web_fetch 工具的 execute。
    async fn run_fetch(&self, url: &str) -> Result<ToolCallOutput, String> {
        let data = self.post("web_fetch", json!({ "url": url })).await?;
        let parsed: FetchResponse = serde_json::from_value(data)
            .map_err(|e| format!("Failed to parse web_fetch response: {e}"))?;

        let mut lines = vec![
            format!("Title: {}", parsed.title),
            String::new(),
            "Content:".to_string(),
            parsed.content.clone(),
            String::new(),
            format!("Links found: {}", parsed.links.len()),
        ];
        lines.extend(parsed.links.iter().take(10).map(|l| format!("  - {l}")));
        let formatted = lines.join("\n");

        Ok(text_response(
            formatted,
            json!({ "title": parsed.title, "content": parsed.content, "links": parsed.links }),
        ))
    }
}

impl Default for WebSearchExtension {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HookHandler for WebSearchExtension {
    fn name(&self) -> &str {
        "web_search"
    }

    fn register_tools(&self, tools: &mut ToolRegistry) {
        tools.register(
            "web_search",
            ToolDefinition {
                name: "web_search".into(),
                label: Some("Web Search".into()),
                description: "Search the web for real-time information. Uses the DeepSeek \
                    Search API when a DeepSeek API key is configured, otherwise a local \
                    Ollama instance's web_search API."
                    .into(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query to execute",
                        },
                        "max_results": {
                            "type": "number",
                            "description": "Maximum number of search results to return (default: 5, max: 50 for DeepSeek)",
                            "default": DEFAULT_MAX_RESULTS,
                        },
                    },
                    "required": ["query"],
                })),
                ..Default::default()
            },
        );
        tools.register(
            "web_fetch",
            ToolDefinition {
                name: "web_fetch".into(),
                label: Some("Web Fetch".into()),
                description: "Fetch and extract text content from a web page URL. Uses the \
                    DeepSeek Fetch API when a DeepSeek API key is configured, otherwise a \
                    local Ollama instance's web_fetch API."
                    .into(),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL to fetch and extract content from",
                        },
                    },
                    "required": ["url"],
                })),
                ..Default::default()
            },
        );
    }

    async fn handle_tool_call(
        &self,
        tool_name: &str,
        params: Value,
        _ctx: &ExtensionContext,
    ) -> Option<ToolCallOutput> {
        match tool_name {
            "web_search" => {
                let Some(query) = params
                    .get("query")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                else {
                    return Some(error_output(
                        "web_search requires a 'query' string parameter.",
                    ));
                };
                let max_results = params
                    .get("max_results")
                    .cloned()
                    .unwrap_or_else(|| json!(DEFAULT_MAX_RESULTS));
                Some(match self.run_search(&query, &max_results).await {
                    Ok(output) => output,
                    Err(msg) => error_output(msg),
                })
            }
            "web_fetch" => {
                let Some(url) = params
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                else {
                    return Some(error_output("web_fetch requires a 'url' string parameter."));
                };
                Some(match self.run_fetch(&url).await {
                    Ok(output) => output,
                    Err(msg) => error_output(msg),
                })
            }
            _ => None,
        }
    }
}

/// 成功输出：文本 content + details（对齐原版返回结构）。
fn text_response(text: String, details: Value) -> ToolCallOutput {
    ToolCallOutput {
        content: vec![json!({ "type": "text", "text": text })],
        details: Some(details),
        is_error: false,
        terminate: None,
    }
}

/// 错误输出：错误文本作为 content，`is_error: true`。
fn error_output(message: impl Into<String>) -> ToolCallOutput {
    ToolCallOutput {
        content: vec![json!({ "type": "text", "text": message.into() })],
        details: None,
        is_error: true,
        terminate: None,
    }
}

#[cfg(test)]
mod tests;
