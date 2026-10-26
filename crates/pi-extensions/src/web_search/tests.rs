//! web_search 扩展测试：本地 mock HTTP 服务，验证请求体、输出格式、
//! details 结构、错误映射（401 / 非 2xx / 连接失败 / 超时）与后端选择
//! （DeepSeek / Ollama）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use super::*;
use pi_extension_api::{
    create_builtin_source_info, ExtensionContext, ExtensionUIContext, RuntimeHandle,
    ToolInvocation,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ============================================================================
// Mock Ollama HTTP 服务
// ============================================================================

/// 一个路径对应的响应规格（状态码 + body + 可选响应延迟）。
#[derive(Clone)]
struct MockResponse {
    status: u16,
    body: String,
    delay: Option<Duration>,
}

/// 本地 mock 服务（Ollama / DeepSeek 共用）：按路径返回配置的响应，
/// 记录收到的请求体与 `Authorization` 头。
struct MockOllama {
    responses: Arc<Mutex<HashMap<String, MockResponse>>>,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    authorizations: Arc<Mutex<Vec<Option<String>>>>,
}

impl MockOllama {
    fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(HashMap::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
            authorizations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 配置某路径的响应（status + body）。
    fn respond(&self, path: &str, status: u16, body: &str) {
        self.responses.lock().unwrap().insert(
            path.to_string(),
            MockResponse {
                status,
                body: body.to_string(),
                delay: None,
            },
        );
    }

    /// 配置某路径的延迟响应（用于超时测试）。
    fn respond_with_delay(&self, path: &str, status: u16, body: &str, delay: Duration) {
        self.responses.lock().unwrap().insert(
            path.to_string(),
            MockResponse {
                status,
                body: body.to_string(),
                delay: Some(delay),
            },
        );
    }

    /// 启动监听，返回可用于扩展的 host（`http://127.0.0.1:{port}`）。
    async fn serve(&self) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let responses = Arc::clone(&self.responses);
        let requests = Arc::clone(&self.requests);
        let authorizations = Arc::clone(&self.authorizations);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let responses = Arc::clone(&responses);
                let requests = Arc::clone(&requests);
                let authorizations = Arc::clone(&authorizations);
                tokio::spawn(async move {
                    let Some((path, headers, body)) = read_http_request(&mut socket).await else {
                        return;
                    };
                    requests
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push((path.clone(), body));
                    authorizations
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(header_value(&headers, "authorization"));
                    let spec = responses
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(|| MockResponse {
                            status: 404,
                            body: "{\"error\":\"Not Found\"}".into(),
                            delay: None,
                        });
                    if let Some(delay) = spec.delay {
                        tokio::time::sleep(delay).await;
                    }
                    let reason = match spec.status {
                        200 => "OK",
                        401 => "Unauthorized",
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        _ => "Mock",
                    };
                    let header = format!(
                        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        spec.status,
                        reason,
                        spec.body.len()
                    );
                    let _ = socket.write_all(header.as_bytes()).await;
                    let _ = socket.write_all(spec.body.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        format!("http://{addr}")
    }

    /// 取第 `i` 个请求的 path + body。
    fn request(&self, i: usize) -> (String, String) {
        self.requests.lock().unwrap_or_else(|p| p.into_inner())[i].clone()
    }

    fn request_count(&self) -> usize {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    /// 取第 `i` 个请求的 `Authorization` 头（缺失时为 `None`）。
    fn authorization(&self, i: usize) -> Option<String> {
        self.authorizations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(i)
            .cloned()
            .flatten()
    }
}

/// 取请求头中的某个字段（大小写不敏感）。
fn header_value(headers: &str, name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// 读一个完整 HTTP/1.1 请求（headers 到 `\r\n\r\n`，body 按 Content-Length），
/// 返回 (path, headers, body)。
async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    let header_end = loop {
        let n = socket.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let path = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    let content_length = headers
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        let n = socket.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..header_end + content_length]).to_string();
    Some((path, headers, body))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// ============================================================================
// 测试辅助
// ============================================================================

/// 构造测试 ExtensionContext（handle_tool_call 需要）。
fn test_ctx() -> ExtensionContext {
    ExtensionContext::new(
        "test-session".into(),
        false,
        ExtensionUIContext::noop(),
        RuntimeHandle::noop(),
    )
}

/// 构造指向 mock 服务的扩展。
async fn mock_ext(mock: &MockOllama) -> WebSearchExtension {
    let host = mock.serve().await;
    WebSearchExtension::new().with_host(&host)
}

/// 调用 handle_tool_call 并取回输出。
async fn call(ext: &WebSearchExtension, tool: &str, params: Value) -> ToolCallOutput {
    ext.handle_tool_call(tool, params, &test_ctx(), &ToolInvocation::noop())
        .await
        .expect("tool handled")
}

// ============================================================================
// 工具注册
// ============================================================================

#[test]
fn test_register_tools_schemas() {
    let ext = WebSearchExtension::new();
    let mut reg = ToolRegistry::new(create_builtin_source_info("web_search"));
    ext.register_tools(&mut reg);
    let tools = reg.into_vec();
    assert_eq!(tools.len(), 2);

    let search = tools
        .iter()
        .find(|t| t.name == "web_search")
        .expect("web_search");
    assert_eq!(search.definition.label.as_deref(), Some("Web Search"));
    let params = search.definition.parameters.as_ref().expect("parameters");
    assert_eq!(params["type"], "object");
    assert_eq!(params["properties"]["query"]["type"], "string");
    assert_eq!(params["required"], json!(["query"]));
    assert_eq!(params["properties"]["max_results"]["default"], 5);
    assert!(params["properties"]["max_results"]
        .get("description")
        .is_some());

    let fetch = tools
        .iter()
        .find(|t| t.name == "web_fetch")
        .expect("web_fetch");
    assert_eq!(fetch.definition.label.as_deref(), Some("Web Fetch"));
    let params = fetch.definition.parameters.as_ref().expect("parameters");
    assert_eq!(params["required"], json!(["url"]));
    assert!(!params["properties"]["url"]["description"].is_null());
}

// ============================================================================
// web_search
// ============================================================================

#[tokio::test]
async fn test_web_search_formats_results() {
    let mock = MockOllama::new();
    mock.respond(
        "/api/experimental/web_search",
        200,
        r#"{"results":[
            {"title":"Rust","url":"https://rust-lang.org","content":"Systems language"},
            {"title":"Pi","url":"https://pi.dev","content":"Agent"}
        ]}"#,
    );
    let ext = mock_ext(&mock).await;

    let output = call(
        &ext,
        "web_search",
        json!({"query": "rust", "max_results": 2}),
    )
    .await;
    assert!(!output.is_error);
    let text = output.content[0]["text"].as_str().expect("text");
    assert_eq!(
        text,
        "1. Rust\n   URL: https://rust-lang.org\n   Systems language\n\n2. Pi\n   URL: https://pi.dev\n   Agent"
    );
    // details 携带原始 results（对齐 TS details: { results }）
    let results = output.details.as_ref().expect("details")["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["title"], "Rust");

    // 请求体：query + max_results 透传
    assert_eq!(mock.request_count(), 1);
    let (path, body) = mock.request(0);
    assert_eq!(path, "/api/experimental/web_search");
    let req: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(req["query"], "rust");
    assert_eq!(req["max_results"], 2);
}

#[tokio::test]
async fn test_web_search_empty_results() {
    let mock = MockOllama::new();
    mock.respond("/api/experimental/web_search", 200, r#"{"results":[]}"#);
    let ext = mock_ext(&mock).await;

    let output = call(&ext, "web_search", json!({"query": "nothing"})).await;
    assert!(!output.is_error);
    let text = output.content[0]["text"].as_str().expect("text");
    assert_eq!(text, "No results found.");
    let details = output.details.as_ref().expect("details");
    assert_eq!(details["results"], json!([]));
}

#[tokio::test]
async fn test_web_search_default_max_results_is_5() {
    let mock = MockOllama::new();
    mock.respond("/api/experimental/web_search", 200, r#"{"results":[]}"#);
    let ext = mock_ext(&mock).await;

    let _ = call(&ext, "web_search", json!({"query": "x"})).await;
    let (_path, body) = mock.request(0);
    let req: Value = serde_json::from_str(&body).expect("json body");
    // 原版 `params.max_results ?? 5`：缺省发送 5
    assert_eq!(req["max_results"], 5);
}

#[tokio::test]
async fn test_web_search_missing_query() {
    let ext = WebSearchExtension::new();
    let output = call(&ext, "web_search", json!({})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "web_search requires a 'query' string parameter."
    );
}

// ============================================================================
// web_fetch
// ============================================================================

#[tokio::test]
async fn test_web_fetch_formats_content_and_truncates_links() {
    let links: Vec<String> = (1..=12)
        .map(|i| format!("https://example.com/{i}"))
        .collect();
    let mock = MockOllama::new();
    mock.respond(
        "/api/experimental/web_fetch",
        200,
        &serde_json::to_string(&json!({
            "title": "Example Page",
            "content": "Body text here.",
            "links": links,
        }))
        .expect("json"),
    );
    let ext = mock_ext(&mock).await;

    let output = call(&ext, "web_fetch", json!({"url": "https://example.com"})).await;
    assert!(!output.is_error);
    let text = output.content[0]["text"].as_str().expect("text");
    assert!(
        text.starts_with(
            "Title: Example Page\n\nContent:\nBody text here.\n\nLinks found: 12\n  - https://example.com/1"
        ),
        "unexpected text: {text}"
    );
    // 链接只列出前 10 条
    assert!(text.contains("  - https://example.com/10"));
    assert!(!text.contains("  - https://example.com/11"));

    // details 携带完整 links（未截断，对齐 TS）
    let details = output.details.as_ref().expect("details");
    assert_eq!(details["title"], "Example Page");
    assert_eq!(details["content"], "Body text here.");
    assert_eq!(details["links"].as_array().unwrap().len(), 12);

    let (_path, body) = mock.request(0);
    let req: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(req["url"], "https://example.com");
}

#[tokio::test]
async fn test_web_fetch_missing_url() {
    let ext = WebSearchExtension::new();
    let output = call(&ext, "web_fetch", json!({})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "web_fetch requires a 'url' string parameter."
    );
}

// ============================================================================
// 错误映射（对齐原版 index.ts）
// ============================================================================

#[tokio::test]
async fn test_web_search_unauthorized() {
    let mock = MockOllama::new();
    mock.respond("/api/experimental/web_search", 401, r#"{"error":"auth"}"#);
    let ext = mock_ext(&mock).await;

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "Unauthorized. Run `ollama signin` to authenticate."
    );
}

#[tokio::test]
async fn test_web_search_api_error_with_body() {
    let mock = MockOllama::new();
    mock.respond("/api/experimental/web_search", 500, r#"{"error":"boom"}"#);
    let ext = mock_ext(&mock).await;

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "web_search API error (status 500): {\"error\":\"boom\"}"
    );
}

#[tokio::test]
async fn test_web_fetch_api_error_falls_back_to_status_text() {
    let mock = MockOllama::new();
    // 无 body 时回退 statusText（对齐 TS `errorText || response.statusText`）
    mock.respond("/api/experimental/web_fetch", 500, "");
    let ext = mock_ext(&mock).await;

    let output = call(&ext, "web_fetch", json!({"url": "https://x.dev"})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "web_fetch API error (status 500): Internal Server Error"
    );
}

#[tokio::test]
async fn test_unknown_path_404_passthrough() {
    let mock = MockOllama::new();
    // 未配置的路径 mock 返回 404（覆盖 post() 对非 2xx 的通用处理）
    let ext = mock_ext(&mock).await;
    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "web_search API error (status 404): {\"error\":\"Not Found\"}"
    );
}

#[tokio::test]
async fn test_connection_refused_message() {
    // 指向一个没有监听的端口
    let ext = WebSearchExtension::new().with_host("http://127.0.0.1:1");
    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "Could not connect to Ollama at http://127.0.0.1:1. Make sure Ollama is running and web_search is enabled."
    );
}

#[tokio::test]
async fn test_connection_refused_message_fetch() {
    let ext = WebSearchExtension::new().with_host("http://127.0.0.1:1");
    let output = call(&ext, "web_fetch", json!({"url": "https://x.dev"})).await;
    assert!(output.is_error);
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "Could not connect to Ollama at http://127.0.0.1:1. Make sure Ollama is running and web_fetch is enabled."
    );
}

#[tokio::test]
async fn test_request_timeout() {
    let mock = MockOllama::new();
    mock.respond_with_delay(
        "/api/experimental/web_search",
        200,
        r#"{"results":[]}"#,
        Duration::from_millis(500),
    );
    let host = mock.serve().await;
    let ext = WebSearchExtension::new()
        .with_host(&host)
        .with_timeout(Duration::from_millis(50));

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(output.is_error);
    let text = output.content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("timed out after 50ms"),
        "unexpected timeout text: {text}"
    );
}

#[tokio::test]
async fn test_unhandled_tool_returns_none() {
    let ext = WebSearchExtension::new();
    let result = ext
        .handle_tool_call(
            "read",
            json!({"path": "/x"}),
            &test_ctx(),
            &ToolInvocation::noop(),
        )
        .await;
    assert!(result.is_none());
}

// ============================================================================
// 后端选择（纯逻辑，显式构造来源，不读进程环境）
// ============================================================================

/// 显式构造后端来源（避免测试受进程环境变量影响）。
fn sources(
    provider: Option<&str>,
    deepseek_api_key: Option<&str>,
    deepseek_base_url: Option<&str>,
    ollama_host: Option<&str>,
) -> BackendSources {
    BackendSources {
        provider: provider.map(str::to_string),
        deepseek_api_key: deepseek_api_key.map(str::to_string),
        deepseek_base_url: deepseek_base_url.map(str::to_string),
        ollama_host: ollama_host.map(str::to_string),
    }
}

#[test]
fn test_backend_auto_prefers_deepseek_when_key_present() {
    let backend = sources(None, Some("sk-key"), None, Some("http://ollama:11434")).resolve();
    match &backend {
        Backend::DeepSeek { base_url, api_key } => {
            assert_eq!(base_url, "https://api.deepseek.com");
            assert_eq!(api_key, "sk-key");
        }
        other => panic!("expected DeepSeek, got {other:?}"),
    }
    assert_eq!(
        backend.endpoint("web_search").unwrap(),
        "https://api.deepseek.com/v1/tools/search"
    );
    assert_eq!(
        backend.endpoint("web_fetch").unwrap(),
        "https://api.deepseek.com/v1/tools/fetch"
    );
}

#[test]
fn test_backend_auto_falls_back_to_ollama_without_key() {
    let backend = sources(Some("auto"), None, None, None).resolve();
    assert!(matches!(backend, Backend::Ollama { .. }));
    assert_eq!(
        backend.endpoint("web_search").unwrap(),
        "http://localhost:11434/api/experimental/web_search"
    );
}

#[test]
fn test_backend_explicit_deepseek_requires_key() {
    // 显式指定 DeepSeek 但缺凭据：不回落到 Ollama，而是保留可执行的错误。
    let backend = sources(Some("deepseek"), Some("   "), None, None).resolve();
    assert!(matches!(backend, Backend::Invalid { .. }));
    let err = backend.endpoint("web_search").unwrap_err();
    assert!(
        err.contains("webSearch.deepseek.apiKey") && err.contains("DEEPSEEK_API_KEY"),
        "unexpected: {err}"
    );
}

#[test]
fn test_backend_unknown_provider_is_invalid() {
    let backend = sources(Some("Bing"), Some("sk"), None, None).resolve();
    let err = backend.endpoint("web_search").unwrap_err();
    assert!(
        err.contains("Unknown web search provider") && err.contains("'bing'"),
        "unexpected: {err}"
    );
}

#[test]
fn test_backend_explicit_ollama_ignores_deepseek_key() {
    let backend = sources(Some("ollama"), Some("sk-key"), None, Some("http://h:1/")).resolve();
    assert!(matches!(backend, Backend::Ollama { .. }));
    assert_eq!(
        backend.endpoint("web_fetch").unwrap(),
        "http://h:1/api/experimental/web_fetch"
    );
}

#[test]
fn test_backend_deepseek_base_url_accepts_v1_suffix() {
    for base in [
        "https://api.deepseek.com",
        "https://api.deepseek.com/",
        "https://api.deepseek.com/v1",
        "https://api.deepseek.com/v1/",
    ] {
        let backend = sources(Some("deepseek"), Some("sk"), Some(base), None).resolve();
        assert_eq!(
            backend.endpoint("web_search").unwrap(),
            "https://api.deepseek.com/v1/tools/search",
            "base_url {base} must normalize"
        );
    }
}

#[test]
fn test_unsupported_tool_has_no_endpoint() {
    let backend = sources(Some("deepseek"), Some("sk"), None, None).resolve();
    assert!(backend.endpoint("read").is_err());
    let ollama = sources(Some("ollama"), None, None, None).resolve();
    assert!(
        ollama.endpoint("read").is_ok(),
        "Ollama path accepts any tool"
    );
}

// ============================================================================
// DeepSeek Search API
// ============================================================================

/// 构造指向 mock 服务的 DeepSeek 扩展。
async fn deepseek_ext(mock: &MockOllama) -> WebSearchExtension {
    let host = mock.serve().await;
    WebSearchExtension::new().with_deepseek("sk-test", &host)
}

#[tokio::test]
async fn test_deepseek_search_sends_query_and_count_with_bearer_auth() {
    let mock = MockOllama::new();
    mock.respond(
        "/v1/tools/search",
        200,
        r#"{"results":[
            {"rerank_score":2.4,"title":"DeepSeek V4","url":"https://example.com/v4",
             "snippet":"short","summary":"long summary for the model","content":"",
             "site_name":"Example News","date":"2026-06-01T00:00:00+00:00","authority":3}
        ]}"#,
    );
    let ext = deepseek_ext(&mock).await;

    let output = call(
        &ext,
        "web_search",
        json!({"query": "  DeepSeek V4  ", "max_results": 3}),
    )
    .await;
    assert!(!output.is_error, "{output:?}");
    // 优先使用面向模型的长摘要，且 query 已 trim
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "1. DeepSeek V4\n   URL: https://example.com/v4\n   long summary for the model"
    );

    let (path, body) = mock.request(0);
    assert_eq!(path, "/v1/tools/search");
    let req: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(req["query"], "DeepSeek V4");
    assert_eq!(req["count"], 3);
    // DeepSeek 用 `count`，不透传 Ollama 的 `max_results`
    assert!(req.get("max_results").is_none(), "{req}");
    assert_eq!(mock.authorization(0).as_deref(), Some("Bearer sk-test"));

    // details 保留 API 原始结果（含 rerank_score / site_name / date 等）
    let results = output.details.as_ref().expect("details")["results"]
        .as_array()
        .expect("results array");
    assert_eq!(results[0]["rerank_score"], 2.4);
    assert_eq!(results[0]["authority"], 3);
}

#[tokio::test]
async fn test_deepseek_search_falls_back_to_snippet() {
    let mock = MockOllama::new();
    mock.respond(
        "/v1/tools/search",
        200,
        r#"{"results":[{"title":"T","url":"https://x.dev","snippet":"100-200 chars","summary":"  "}]}"#,
    );
    let ext = deepseek_ext(&mock).await;

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "1. T\n   URL: https://x.dev\n   100-200 chars"
    );
    // 缺省 max_results=5 → count=5
    let (_path, body) = mock.request(0);
    let req: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(req["count"], 5);
}

#[tokio::test]
async fn test_deepseek_search_count_clamped_to_50() {
    let mock = MockOllama::new();
    mock.respond("/v1/tools/search", 200, r#"{"results":[]}"#);
    let ext = deepseek_ext(&mock).await;

    let output = call(
        &ext,
        "web_search",
        json!({"query": "x", "max_results": 999}),
    )
    .await;
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "No results found."
    );
    let (_path, body) = mock.request(0);
    let req: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(req["count"], 50);
}

#[tokio::test]
async fn test_deepseek_search_rejects_too_long_or_blank_query() {
    let mock = MockOllama::new();
    mock.respond("/v1/tools/search", 200, r#"{"results":[]}"#);
    let ext = deepseek_ext(&mock).await;

    let too_long = "长".repeat(129);
    let output = call(&ext, "web_search", json!({"query": too_long})).await;
    assert!(output.is_error);
    assert!(
        output.content[0]["text"]
            .as_str()
            .unwrap()
            .contains("at most 128 characters"),
        "{output:?}"
    );

    // 128 字符仍在范围内
    let ok = "a".repeat(128);
    let output = call(&ext, "web_search", json!({"query": ok})).await;
    assert!(!output.is_error, "{output:?}");
    assert_eq!(mock.request_count(), 1);
}

#[tokio::test]
async fn test_deepseek_unauthorized_message() {
    let mock = MockOllama::new();
    mock.respond(
        "/v1/tools/search",
        401,
        r#"{"error":{"message":"bad key"}}"#,
    );
    let ext = deepseek_ext(&mock).await;

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(output.is_error);
    let text = output.content[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Unauthorized.") && text.contains("DEEPSEEK_API_KEY"),
        "unexpected: {text}"
    );
}

#[tokio::test]
async fn test_deepseek_error_status_hints() {
    let cases = [
        (402, "insufficient balance"),
        (403, "safety policy"),
        (404, "could not retrieve usable page content"),
        (408, "timed out reading the request body"),
        (413, "size limit"),
        (429, "rate limit"),
        (500, "server error"),
        (504, "server error"),
    ];
    for (status, expected) in cases {
        let mock = MockOllama::new();
        mock.respond("/v1/tools/search", status, r#"{"error":{"message":"x"}}"#);
        let ext = deepseek_ext(&mock).await;
        let output = call(&ext, "web_search", json!({"query": "x"})).await;
        assert!(output.is_error, "status {status} must be an error");
        let text = output.content[0]["text"].as_str().unwrap();
        assert!(
            text.contains(expected),
            "status {status}: expected '{expected}' in '{text}'"
        );
    }
}

#[tokio::test]
async fn test_deepseek_bad_request_surfaces_api_message() {
    let mock = MockOllama::new();
    mock.respond(
        "/v1/tools/search",
        400,
        r#"{"error":{"message":"'count' must be between 1 and 50","type":"invalid_request_error","param":null}}"#,
    );
    let ext = deepseek_ext(&mock).await;

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "web_search API error (status 400): 'count' must be between 1 and 50"
    );
}

#[tokio::test]
async fn test_deepseek_connection_refused_message() {
    let ext = WebSearchExtension::new().with_deepseek("sk-test", "http://127.0.0.1:1");
    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert_eq!(
        output.content[0]["text"].as_str().unwrap(),
        "Could not connect to DeepSeek at http://127.0.0.1:1. Check your network connection or DEEPSEEK_BASE_URL."
    );
}

#[tokio::test]
async fn test_deepseek_timeout_message_names_deepseek() {
    let mock = MockOllama::new();
    mock.respond_with_delay(
        "/v1/tools/search",
        200,
        r#"{"results":[]}"#,
        Duration::from_millis(500),
    );
    let host = mock.serve().await;
    let ext = WebSearchExtension::new()
        .with_deepseek("sk-test", &host)
        .with_timeout(Duration::from_millis(50));

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(output.is_error);
    let text = output.content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("timed out after 50ms") && text.contains("DeepSeek"),
        "unexpected: {text}"
    );
}

// ============================================================================
// DeepSeek Fetch API
// ============================================================================

#[tokio::test]
async fn test_deepseek_fetch_returns_markdown_content() {
    let mock = MockOllama::new();
    mock.respond(
        "/v1/tools/fetch",
        200,
        r##"{"title":"Example article","content":"# Example article\n\nArticle content."}"##,
    );
    let ext = deepseek_ext(&mock).await;

    let output = call(&ext, "web_fetch", json!({"url": "https://example.com/a"})).await;
    assert!(!output.is_error, "{output:?}");
    let text = output.content[0]["text"].as_str().unwrap();
    assert!(text
        .starts_with("Title: Example article\n\nContent:\n# Example article\n\nArticle content."));
    // DeepSeek Fetch 不返回 links → 0（details 为空数组，不丢字段）
    assert!(text.ends_with("Links found: 0"), "unexpected: {text}");

    let details = output.details.as_ref().expect("details");
    assert_eq!(details["links"], json!([]));
    assert_eq!(details["title"], "Example article");

    let (path, body) = mock.request(0);
    assert_eq!(path, "/v1/tools/fetch");
    let req: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(req["url"], "https://example.com/a");
}

#[tokio::test]
async fn test_deepseek_fetch_404_message() {
    let mock = MockOllama::new();
    mock.respond(
        "/v1/tools/fetch",
        404,
        r#"{"error":{"message":"no content","type":"invalid_request_error"}}"#,
    );
    let ext = deepseek_ext(&mock).await;

    let output = call(&ext, "web_fetch", json!({"url": "https://example.com/a"})).await;
    assert!(output.is_error);
    assert!(
        output.content[0]["text"]
            .as_str()
            .unwrap()
            .contains("could not retrieve usable page content"),
        "{output:?}"
    );
}

// ============================================================================
// settings.json `webSearch` 配置 → 后端
// ============================================================================

#[test]
fn test_config_section_parses_camel_case_and_drives_backend() {
    let config: WebSearchConfig = serde_json::from_value(json!({
        "provider": "deepseek",
        "deepseek": { "baseUrl": "https://gw.example.com/v1", "apiKey": "sk-gw" },
        "ollama": { "host": "http://ollama.lan:11434" }
    }))
    .expect("config parses");

    let backend = BackendSources::from_config(&config).resolve();
    match &backend {
        Backend::DeepSeek { base_url, api_key } => {
            assert_eq!(base_url, "https://gw.example.com");
            assert_eq!(api_key, "sk-gw");
        }
        other => panic!("expected DeepSeek, got {other:?}"),
    }
    assert_eq!(
        backend.endpoint("web_search").unwrap(),
        "https://gw.example.com/v1/tools/search"
    );
}

#[test]
fn test_config_provider_ollama_uses_configured_host() {
    let config = WebSearchConfig {
        provider: Some("ollama".into()),
        deepseek: Some(DeepSeekConfig {
            api_key: Some("sk-unused".into()),
            base_url: None,
        }),
        ollama: Some(OllamaConfig {
            host: Some("http://ollama.lan:11434/".into()),
        }),
    };
    let backend = BackendSources::from_config(&config).resolve();
    assert_eq!(
        backend.endpoint("web_fetch").unwrap(),
        "http://ollama.lan:11434/api/experimental/web_fetch"
    );
}

#[test]
fn test_config_unknown_provider_reports_config_key() {
    let config = WebSearchConfig {
        provider: Some("searx".into()),
        ..Default::default()
    };
    let err = BackendSources::from_config(&config)
        .resolve()
        .endpoint("web_search")
        .unwrap_err();
    // 提示里带上配置项名，用户才知道改哪里
    assert!(err.contains("'searx'"), "unexpected: {err}");
    assert!(err.contains("webSearch.provider"), "unexpected: {err}");
}

#[test]
fn test_default_config_serializes_to_empty_object() {
    assert_eq!(
        serde_json::to_value(WebSearchConfig::default()).unwrap(),
        json!({})
    );
    let parsed: WebSearchConfig = serde_json::from_value(json!({})).expect("default parses");
    assert_eq!(parsed, WebSearchConfig::default());
}

#[tokio::test]
async fn test_from_config_routes_requests_to_configured_ollama() {
    let mock = MockOllama::new();
    mock.respond("/api/experimental/web_search", 200, r#"{"results":[]}"#);
    let host = mock.serve().await;
    let ext = WebSearchExtension::from_config(&WebSearchConfig {
        provider: Some("ollama".into()),
        deepseek: None,
        ollama: Some(OllamaConfig {
            host: Some(host.clone()),
        }),
    });

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(!output.is_error, "{output:?}");
    let (path, body) = mock.request(0);
    assert_eq!(path, "/api/experimental/web_search");
    let req: Value = serde_json::from_str(&body).expect("json body");
    // 配置化不影响 Ollama 的参数契约（仍是 max_results）
    assert_eq!(req["max_results"], 5);
}

#[tokio::test]
async fn test_from_config_routes_requests_to_configured_deepseek() {
    let mock = MockOllama::new();
    mock.respond("/v1/tools/search", 200, r#"{"results":[]}"#);
    let host = mock.serve().await;
    let ext = WebSearchExtension::from_config(&WebSearchConfig {
        provider: Some("deepseek".into()),
        deepseek: Some(DeepSeekConfig {
            base_url: Some(host.clone()),
            api_key: Some("sk-config".into()),
        }),
        ollama: None,
    });

    let output = call(&ext, "web_search", json!({"query": "x"})).await;
    assert!(!output.is_error, "{output:?}");
    assert_eq!(mock.request(0).0, "/v1/tools/search");
    assert_eq!(mock.authorization(0).as_deref(), Some("Bearer sk-config"));
}
