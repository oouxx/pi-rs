//! web_search 扩展测试：本地 mock Ollama HTTP 服务，验证请求体、输出格式、
//! details 结构与错误映射（401 / 非 2xx / 连接失败 / 超时）。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use super::*;
use pi_extension_api::{
    create_builtin_source_info, ExtensionContext, ExtensionUIContext, RuntimeHandle,
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

/// 本地 mock Ollama 服务：按路径返回配置的响应，记录收到的请求体。
struct MockOllama {
    responses: Arc<Mutex<HashMap<String, MockResponse>>>,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl MockOllama {
    fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(HashMap::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 配置某路径的响应（status + body）。
    fn respond(&self, path: &str, status: u16, body: &str) {
        self.responses
            .lock()
            .unwrap()
            .insert(path.to_string(), MockResponse {
                status,
                body: body.to_string(),
                delay: None,
            });
    }

    /// 配置某路径的延迟响应（用于超时测试）。
    fn respond_with_delay(&self, path: &str, status: u16, body: &str, delay: Duration) {
        self.responses
            .lock()
            .unwrap()
            .insert(path.to_string(), MockResponse {
                status,
                body: body.to_string(),
                delay: Some(delay),
            });
    }

    /// 启动监听，返回可用于扩展的 host（`http://127.0.0.1:{port}`）。
    async fn serve(&self) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let responses = Arc::clone(&self.responses);
        let requests = Arc::clone(&self.requests);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let responses = Arc::clone(&responses);
                let requests = Arc::clone(&requests);
                tokio::spawn(async move {
                    let Some((path, body)) = read_http_request(&mut socket).await else {
                        return;
                    };
                    requests
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push((path.clone(), body));
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
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())[i]
            .clone()
    }

    fn request_count(&self) -> usize {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }
}

/// 读一个完整 HTTP/1.1 请求（headers 到 `\r\n\r\n`，body 按 Content-Length）。
async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
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
    Some((path, body))
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
    ext.handle_tool_call(tool, params, &test_ctx())
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

    let search = tools.iter().find(|t| t.name == "web_search").expect("web_search");
    assert_eq!(search.definition.label.as_deref(), Some("Web Search"));
    let params = search.definition.parameters.as_ref().expect("parameters");
    assert_eq!(params["type"], "object");
    assert_eq!(params["properties"]["query"]["type"], "string");
    assert_eq!(params["required"], json!(["query"]));
    assert_eq!(params["properties"]["max_results"]["default"], 5);
    assert!(params["properties"]["max_results"].get("description").is_some());

    let fetch = tools.iter().find(|t| t.name == "web_fetch").expect("web_fetch");
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
        .handle_tool_call("read", json!({"path": "/x"}), &test_ctx())
        .await;
    assert!(result.is_none());
}
