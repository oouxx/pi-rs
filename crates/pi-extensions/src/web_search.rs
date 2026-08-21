//! pi-web-search — `web_search` / `web_fetch` 工具扩展。
//!
//! 对应 npm 包 `@ollama/pi-web-search`（v0.0.5，Ollama 官方，MIT）：
//! 通过本地 Ollama 实例的实验性 API 提供实时搜索与网页内容提取：
//! - `POST {host}/api/experimental/web_search`  body `{query, max_results}`
//! - `POST {host}/api/experimental/web_fetch`   body `{url}`
//!
//! 行为对齐原版 `index.ts`：
//! - 工具名 `web_search` / `web_fetch`，label/description 与原版一致，
//!   参数 JSON Schema 与 typebox 输出一致（`required: ["query"]` /
//!   `required: ["url"]`，`max_results` 可选、默认 5）
//! - 输出文本格式一致：搜索为 `1. {title}\n   URL: {url}\n   {content}`
//!   逐条 `\n\n` 连接（空结果 → "No results found."）；fetch 为
//!   `Title: ...` + `Content:` + `Links found: N` + 最多 10 条 `  - link`
//! - `details` 结构与原版一致：搜索 `{results: [...]}`、fetch
//!   `{title, content, links}`
//! - 错误文本一致：401 → "Unauthorized. Run `ollama signin` to authenticate."；
//!   其他非 2xx → `Search/Fetch API error (status N): <body|statusText>`；
//!   连接失败 → "Could not connect to Ollama at {host}. Make sure Ollama is
//!   running and web_search|web_fetch is enabled."
//!
//! 有意差异（pi-rs 增强，见各文件 doc 注明）：
//! - 宿主可配：默认 `http://localhost:11434`（与原版 `getOllamaHost()` 一致），
//!   可用 `OLLAMA_HOST` / `OLLAMA_BASE_URL` 环境变量覆盖（与 pi-ai 的
//!   ollama provider 的端点覆盖规则一致，见 crates/pi-ai/DEVIATIONS.md #4）
//! - 请求加 60s 超时（原版依赖调用方 signal 取消，无显式超时；本地服务
//!   挂死不应卡死 agent 循环）
//! - 连接失败判定用 reqwest `is_connect()`（覆盖 ECONNREFUSED 及同类
//!   localhost 连接错误），消息文本与原版一致
//!
//! 注：扩展工具的 `ToolCallOutput.is_error` 在 agent 循环边界（
//! `agent_session.rs` 的 dispatch 包装）当前不传递到
//! `AgentMessage::ToolResult.is_error`（与 goal/subagent 扩展相同的既有
//! 架构约定）；错误通过 content 文本 + `is_error: true` 表达。

use std::time::Duration;

use async_trait::async_trait;
use pi_extension_api::{ExtensionContext, HookHandler, ToolCallOutput, ToolDefinition, ToolRegistry};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::timeout;

/// 默认 Ollama 地址，与原版 `getOllamaHost()` 一致。
const DEFAULT_OLLAMA_HOST: &str = "http://localhost:11434";
/// 单次请求超时（原版无显式超时，依赖调用方 signal）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

// ============================================================================
// API 响应类型 — 对应原版 SearchResponse / FetchResponse
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FetchResponse {
    #[serde(default)]
    title: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    links: Vec<String>,
}

// ============================================================================
// WebSearchExtension
// ============================================================================

/// web_search / web_fetch 扩展：通过本地 Ollama 提供实时搜索与网页抓取。
pub struct WebSearchExtension {
    host: String,
    client: reqwest::Client,
    timeout: Duration,
}

impl WebSearchExtension {
    /// 创建扩展。host 优先级：`OLLAMA_HOST` > `OLLAMA_BASE_URL` >
    /// `http://localhost:11434`（与原版默认一致）。
    #[must_use]
    pub fn new() -> Self {
        let host = std::env::var("OLLAMA_HOST")
            .ok()
            .filter(|h| !h.trim().is_empty())
            .or_else(|| {
                std::env::var("OLLAMA_BASE_URL")
                    .ok()
                    .filter(|h| !h.trim().is_empty())
            })
            .unwrap_or_else(|| DEFAULT_OLLAMA_HOST.to_string());
        Self {
            host,
            client: reqwest::Client::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// 指定 Ollama 地址（测试用；也可用于配置远程端点）。
    #[must_use]
    pub fn with_host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    /// 指定单次请求超时（测试用）。
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// POST JSON 到 Ollama 实验性 API，返回已解析的 JSON 值。
    ///
    /// 错误映射（对齐原版）：
    /// - 连接失败 → `Could not connect to Ollama at {host}. Make sure Ollama
    ///   is running and {tool} is enabled.`（原版 ECONNREFUSED 分支）
    /// - 401 → `Unauthorized. Run \`ollama signin\` to authenticate.`
    /// - 其他非 2xx → `{Tool} API error (status {N}): {body|statusText}`
    /// - 超时 → 明确的超时消息（pi-rs 增强）
    async fn post(&self, tool: &str, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.host, path);
        let response = timeout(
            self.timeout,
            self.client.post(&url).json(&body).send(),
        )
        .await
        .map_err(|_| {
            let timeout_desc = if self.timeout.as_secs() >= 1 {
                format!("{}s", self.timeout.as_secs())
            } else {
                format!("{}ms", self.timeout.as_millis())
            };
            format!(
                "{tool} request to Ollama at {} timed out after {}.",
                self.host, timeout_desc
            )
        })?
        .map_err(|e| {
            if e.is_connect() {
                format!(
                    "Could not connect to Ollama at {}. Make sure Ollama is running and {} is enabled.",
                    self.host, tool
                )
            } else {
                format!("{tool} request to Ollama at {} failed: {e}", self.host)
            }
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err("Unauthorized. Run `ollama signin` to authenticate.".into());
        }
        if !status.is_success() {
            let detail = response
                .text()
                .await
                .unwrap_or_default()
                .trim()
                .to_string();
            let detail = if detail.is_empty() {
                status
                    .canonical_reason()
                    .unwrap_or("unknown error")
                    .to_string()
            } else {
                detail
            };
            return Err(format!(
                "{tool} API error (status {}): {}",
                status.as_u16(),
                detail
            ));
        }

        response
            .json::<Value>()
            .await
            .map_err(|e| format!("Failed to parse {tool} response from Ollama: {e}"))
    }

    /// 执行搜索（对齐原版 web_search 工具的 execute）。
    async fn run_search(&self, query: &str, max_results: Value) -> Result<ToolCallOutput, String> {
        let data = self
            .post(
                "web_search",
                "/api/experimental/web_search",
                json!({ "query": query, "max_results": max_results }),
            )
            .await?;
        let parsed: SearchResponse = serde_json::from_value(data)
            .map_err(|e| format!("Failed to parse web_search response: {e}"))?;

        let formatted = if parsed.results.is_empty() {
            "No results found.".to_string()
        } else {
            parsed
                .results
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    format!("{}. {}\n   URL: {}\n   {}", i + 1, r.title, r.url, r.content)
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        Ok(text_response(formatted, json!({ "results": parsed.results })))
    }

    /// 执行抓取：原版 web_fetch 工具的 execute。
    async fn run_fetch(&self, url: &str) -> Result<ToolCallOutput, String> {
        let data = self
            .post(
                "web_fetch",
                "/api/experimental/web_fetch",
                json!({ "url": url }),
            )
            .await?;
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
                description: "Search the web for real-time information using your local \
                    Ollama instance's web_search API. Requires Ollama running locally with \
                    web search enabled."
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
                            "description": "Maximum number of search results to return (default: 5)",
                            "default": 5,
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
                description: "Fetch and extract text content from a web page URL using \
                    your local Ollama instance's web_fetch API. Requires Ollama running \
                    locally with web fetch enabled."
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
                let max_results = params.get("max_results").cloned().unwrap_or(json!(5));
                Some(match self.run_search(&query, max_results).await {
                    Ok(output) => output,
                    Err(msg) => error_output(msg),
                })
            }
            "web_fetch" => {
                let Some(url) = params.get("url").and_then(|v| v.as_str()).map(str::to_string)
                else {
                    return Some(error_output(
                        "web_fetch requires a 'url' string parameter.",
                    ));
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
