# MCP 复刻实施分析（pi-rs ← pi TS）

> 状态：**阶段一分析稿（待确认后进入阶段二）**
> 决策日期：2026-11-02
>
> **实施进度（2026-11-02）**
> - ✅ `crates/pi-mcp` 已落地：JSON-RPC/协议类型/content、stdio、streamable HTTP（SSE、
>   session、重连、OAuth 接线）、in-memory、OAuth（types/discovery/flow/provider/
>   callback）、client；28 个单测通过，clippy 零警告。
> - ✅ Phase 0 部分：`ToolDefinition` 新增 `exposure` / `namespace` / `output_schema` /
>   `annotations` / `reaches_tools`；初始 active 集合按 exposure 过滤；`ToolInfo` 暴露
>   exposure/namespace/outputSchema。
> - ✅ `tool_search`：BM25（tokenize/stem/schema text/name+namespace 权重）+ 激活命中工具，
>   含 deferred 工具端到端测试。
> - ✅ Phase 4–7：完整 `mcp.json` 配置/override/校验、`McpServerConnection` 状态机、
>   MCP tool → pi tool、结果截断/临时文件、resources 三工具、OAuth 凭据存储 + 刷新锁 +
>   auth provider、`mcp.log`、system prompt `mcp_servers` section、`/mcp` 菜单序列、
>   `pi-rs mcp login/logout`、静态 `registerMcpServer`、legacy SSE transport。
> - ⏳ 待做：ACP `mcpServers` 切到新 runtime、`tools/list_changed` 动态重注册、
>   `auth.provider`、codemode（不在范围）、CIMD（默认禁用）、Phase 8 文档/全量回归。
>
> TS 参考：`origin/main` 的 `packages/mcp`（v1.0.0 形态）与
> `packages/coding-agent/src/extensions/mcp`。
> 用户已确认的范围：
> - **MCP 全量，不含 codemode**
> - **新建 `crates/pi-mcp`，按 TS `packages/mcp` 逐模块重写**
> - 补齐：resources 三工具、`tools/list_changed`、OAuth 2.1 + DCR/CIMD + 回环回调、
>   `/mcp` TUI 管理器、扩展 `registerMcpServer`、`exposure`（direct/deferred）+ `tool_search`
> - 不做：`packages/codemode`、`codemode` exposure 的实际执行、`searchTools()`/
>   `describeNamespace()` 的脚本运行时

本文只描述"怎么翻、翻成什么、哪里会出错"，不包含实现代码。阶段二按
`类型定义 → 测试 → 实现 → 对抗式复核` 逐模块执行。

---

## 0. 与当前 pi-rs 的关系

pi-rs 目前已经有一套**原生 MCP 客户端**，登记在
`crates/pi-coding-agent/DEVIATIONS.md` #14（状态：已确认保留）：

| 现有文件 | 职责 |
| --- | --- |
| `core/mcp.rs` | rmcp 客户端，stdio + streamable HTTP + legacy SSE，工具转 `ToolDefinition` |
| `core/mcp_config.rs` | `{agent_dir}/mcp.json` + `{cwd}/.pi-rs/mcp.json` 读取/编辑 |
| `core/mcp_sse.rs` | rmcp 缺失的 legacy SSE transport |
| `core/sdk.rs:876-897` | session 创建时连接服务器并注入 `custom_tools` |
| `pi-cli/src/mcp_cli.rs` | `pi-rs mcp list/add/remove/enable/disable` |

这次不是"从零"复刻，而是**用新的 `pi-mcp` crate 替换 rmcp 实现**，并把
coding-agent 侧从"一次性注入工具"升级为 TS v1.0.0 的完整 MCP 扩展行为。
`DEVIATIONS.md` #14 需要改写：原生实现的架构偏差改为"按 TS 重写"，但
legacy SSE、项目始终可信、`.pi-rs` 目录名继续保留。

---

## 1. TS 模块清单与 Rust 目标布局

### 1.1 `packages/mcp`（新建 `crates/pi-mcp`）

| TS 文件 | 行数 | 职责 | Rust 目标 |
| --- | ---: | --- | --- |
| `src/protocol/jsonrpc.ts` | 113 | JSON-RPC 2.0 消息、判别函数、`McpError`/`McpTimeoutError`/`McpAbortError`/`McpConnectionClosedError` | `src/protocol/jsonrpc.rs` |
| `src/protocol/types.ts` | 131 | 协议版本、Initialize、Tool、Resource、ResourceTemplate | `src/protocol/types.rs` |
| `src/protocol/content.ts` | 118 | ContentBlock、CallToolResult、`toLlmContent` | `src/protocol/content.rs` |
| `src/transports/transport.ts` | 64 | `McpTransport` 接口 + 监听器管理 | `src/transport.rs` |
| `src/transports/stdio.ts` | 216 | 子进程 stdio、逐行 JSON、stderr 尾部、关闭序列 | `src/transports/stdio.rs` |
| `src/transports/streamable-http.ts` | 522 | POST/GET、SSE 解析、session、重连、OAuth | `src/transports/streamable_http.rs` |
| `src/transports/in-memory.rs` | 51 | 测试用内存 transport | `src/transports/in_memory.rs` |
| `src/client.ts` | 621 | 请求/通知/进度/取消、分页、校验、生命周期 | `src/client.rs` |
| `src/auth-provider.ts` | 20 | `AuthProvider` 接口 | `src/auth.rs` |
| `src/oauth/types.ts` | 215 | OAuth 元数据类型/解析 | `src/oauth/types.rs` |
| `src/oauth/discovery.ts` | 215 | protected-resource / AS 元数据发现、`WWW-Authenticate` | `src/oauth/discovery.rs` |
| `src/oauth/flow.ts` | 485 | PKCE、DCR、code exchange、refresh、`adaptOAuthProvider` | `src/oauth/flow.rs` |
| `src/oauth/provider.ts` | 168 | `McpOAuthProvider`、状态存储、单 server URL 隔离 | `src/oauth/provider.rs` |
| `src/oauth/callback.ts` | 164 | 回环 HTTP 回调服务器 | `src/oauth/callback.rs` |
| `src/oauth/errors.ts` | 55 | OAuth 错误类型 | `src/oauth/errors.rs` |
| `src/testing/index.ts` | 1 | 测试入口 | `src/testing.rs` |

### 1.2 `packages/coding-agent` 的 MCP 部分

| TS 文件 | 行数 | 职责 | Rust 目标 |
| --- | ---: | --- | --- |
| `core/mcp-servers.ts` | 358 | 配置类型、校验、exposure 解析、namespace、`McpServerRegistry` | `core/mcp_servers.rs` |
| `extensions/mcp/config.ts` | 243 | 读/写 `mcp.json`、全局/项目合并、override | `core/mcp/config.rs` |
| `extensions/mcp/runtime.ts` | 454 | 连接状态机、懒连接、重连、OAuth 接线、`list_changed` | `core/mcp/runtime.rs` |
| `extensions/mcp/tools.ts` | 391 | MCP tool → pi tool、结果转换/截断/临时文件 | `core/mcp/tools.rs` |
| `extensions/mcp/resources.ts` | 339 | 三个 resources 工具 | `core/mcp/resources.rs` |
| `extensions/mcp/oauth.ts` | 579 | `mcp-auth.json`、刷新锁、登录流程 | `core/mcp/oauth.rs` |
| `extensions/mcp/log.ts` | 69 | `notifications/message` → `mcp.log` | `core/mcp/log.rs` |
| `extensions/mcp/ui.ts` | 268 | `/mcp` TUI 管理器 | `core/mcp/ui.rs` + `pi-tui` 接线 |
| `extensions/mcp/cli.ts` | 625 | `pi mcp` 子命令 | 扩展 `pi-cli/src/mcp_cli.rs` |
| `extensions/mcp/index.ts` | 1347 | 内置扩展生命周期、exposure、系统提示、`/mcp` 命令 | `core/mcp/mod.rs`（或 `core/mcp/extension.rs`） |
| `extensions/tool-search/tool.ts` | 337 | BM25 排序 + `tool_search` | `core/tool_search.rs` |

`packages/codemode/*` **不移植**；`codemode` exposure 只保留配置解析与
"不可达"告警，不注册 codemode 工具。

---

## 2. 核心数据结构与 Rust 映射

### 2.1 JSON-RPC

TS：

```ts
type JsonRpcId = string | number;
interface JsonRpcRequest { jsonrpc: "2.0"; id: JsonRpcId; method: string; params?: unknown }
interface JsonRpcNotification { jsonrpc: "2.0"; method: string; params?: unknown }
type JsonRpcResponse = Success | Error;
```

Rust：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "jsonrpc", rename = "2.0")] // 或手写 enum + untagged 判别
pub enum JsonRpcMessage {
    Request { id: JsonRpcId, method: String, params: Option<Value> },
    Notification { method: String, params: Option<Value> },
    Success { id: JsonRpcId, result: Value },
    Error { id: JsonRpcId, error: JsonRpcErrorObject },
}
```

注意：
- TS 的 `isJsonRpcResponse` 先看 `result` 再看 `error`；Rust 用
  `#[serde(untagged)]` 时字段缺失/额外字段的判别顺序必须与 TS 一致。
- `isJsonRpcNotification` 要求**没有** `id` 键（不是 `id: null`）。
- `parse_json_rpc_message` 对非法消息返回 `McpError::invalid_request`，
  transport 收到后发 `on_error`，不是断开连接。

### 2.2 MCP 协议类型

按 `protocol/types.ts` 直译，全部 `serde(rename_all = "camelCase")`：

```rust
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub struct Implementation { name, version, title: Option<String> }
pub struct Tool {
    name, title: Option<String>, description: Option<String>,
    input_schema: Map<String, Value>,
    output_schema: Option<Map<String, Value>>,
    annotations: Option<ToolAnnotations>,
    execution: Option<ToolExecution>,
    meta: Option<Map<String, Value>>,
}
pub struct Resource / ResourceTemplate / ReadResourceResult / List*Result ...
```

`Tool.inputSchema` 在 TS 是必填对象；Rust 用 `Map<String, Value>`，
反序列化缺失时由 client 校验报 `Invalid MCP tools/list result`。

### 2.3 Content

TS 是 discriminated union；Rust：

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String, annotations: Option<ContentAnnotations>, meta: Option<Map<String, Value>> },
    Image { data: String, mime_type: String, ... },
    Audio { data: String, mime_type: String, ... },
    ResourceLink { uri, name, title, description, mime_type, size, ... },
    Resource { resource: EmbeddedResource, ... },
}
```

`structuredContent` 是**任意 JSON 值**（包括数组、`null`），Rust 用
`Option<Value>`，不能用 `Option<Map<..>>`，否则会丢掉数组/标量。

`to_llm_content` 的输出在 `pi-mcp` 内定义为：

```rust
pub enum LlmContent {
    Text { text: String },
    Image { data: String, mime_type: String },
}
```

由 `pi-coding-agent` 转换为 `pi-ai` 的 `text_block` / `image_block`。
这样 `pi-mcp` 不依赖 `pi-ai`，符合 TS 包的独立边界。

### 2.4 Client

```rust
pub struct McpClientOptions {
    pub name: String,
    pub version: String,
    pub title: Option<String>,
    pub capabilities: ClientCapabilities,
    pub protocol_version: Option<String>,
    pub request_timeout: Option<Duration>,
    pub roots: Option<RootsProvider>,
}

pub struct McpRequestOptions {
    pub cancel: Option<CancellationToken>,
    pub timeout: Option<Duration>,
    pub on_progress: Option<Arc<dyn Fn(ProgressNotification) + Send + Sync>>,
}

pub struct McpClient { /* state, transport, pending, listeners ... */ }
```

方法对照：

| TS | Rust |
| --- | --- |
| `connect(transport)` | `async fn connect(&mut self, transport: Box<dyn McpTransport>) -> Result<InitializeResult, McpError>` |
| `request<T>(method, params, opts)` | `async fn request(&self, method: &str, params: Option<Value>, opts: McpRequestOptions) -> Result<Value, McpError>` |
| `notify` | `async fn notify(&self, method, params) -> Result<(), McpError>` |
| `listTools` | `async fn list_tools(&self, opts) -> Result<Vec<Tool>, McpError>` |
| `listResources` / `listResourcesPage` | `list_resources` / `list_resources_page` |
| `listResourceTemplates` | `list_resource_templates` / `..._page` |
| `readResource` | `read_resource(uri, opts)` |
| `callTool` | `call_tool(name, args, opts)` |
| `ping` | `ping(opts)` |
| `onNotification` | `on_notification(method, listener) -> Subscription` |
| `onError` / `onClose` | 同名 |
| `setRequestHandler` | `set_request_handler(method, handler)` |
| `connectionState` / getters | `state()` / `server_info()` / ... |

关键行为（必须逐条对齐）：
- `initialize` 不允许取消（TS `cancellable: method !== "initialize"`）。
- 请求超时后发 `notifications/cancelled`，除非是 `initialize`。
- `notifications/progress` 会**重新计时**；`onProgress` 抛错走 `emitError`。
- 收到未知 id 的 response 只报 error，不 panic、不断开。
- response 的 `result`/`error` 同时存在时按 TS 的判别顺序处理。
- 分页最多 1000 页；重复 cursor 报错；`nextCursor` 为 `null`/`""`
  视为结束。
- `callTool` 的 `content` 缺失时补空数组（某些 server 只返回
  `structuredContent`）。
- 传输关闭时，所有 pending 请求以 `McpConnectionClosedError` 结束，
  并中止正在处理的 server→client 请求。
- `notifications/cancelled` 中止对应的 incoming request handler。

### 2.5 Transport

```rust
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn start(&mut self) -> Result<(), McpError>;
    async fn send(&self, message: JsonRpcMessage, opts: McpTransportSendOptions)
        -> Result<(), McpError>;
    async fn close(&mut self) -> Result<(), McpError>;
    fn on_message(&self, listener: Arc<dyn Fn(JsonRpcMessage) + Send + Sync>) -> Subscription;
    fn on_error(&self, listener: Arc<dyn Fn(McpError) + Send + Sync>) -> Subscription;
    fn on_close(&self, listener: Arc<dyn Fn() + Send + Sync>) -> Subscription;
    fn set_protocol_version(&self, _version: &str) {}
}
```

`Subscription` 在 `Drop` 时注销监听（对应 TS 返回的 disposer）。
`TransportEvents` 用 `std::sync::RwLock<Vec<...>>` 保存监听器，emit 时
克隆一份避免持锁回调。

`McpTransportSendOptions.signal` 语义：只在"还没 dispatch 时"阻止重试；
Rust 用 `CancellationToken` 或 `watch::Receiver<bool>`。建议
`pi-mcp` 用 `tokio_util::sync::CancellationToken`，在 coding-agent 接线
时与 `ToolInvocation.abort` 互转。

### 2.6 错误

```rust
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP request timed out after {0:?}")]
    Timeout(Duration),
    #[error("MCP request aborted")]
    Aborted,
    #[error("MCP connection closed: {0}")]
    ConnectionClosed(String),
    #[error("MCP JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String, data: Option<Value> },
    #[error("MCP HTTP error {status}: {body}")]
    Http { status: u16, body: String },
    #[error("MCP server requires authentication")]
    AuthRequired { www_authenticate: Option<String> },
    #[error("MCP session expired")]
    SessionExpired { body: String },
    #[error("Invalid MCP response: {0}")]
    InvalidResponse(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    OAuth(#[from] OAuthError),
}
```

`McpHttpError` / `McpAuthRequiredError` / `McpSessionExpiredError` 在
Rust 里合并为 variant，但 `status`/`www_authenticate`/`body` 必须保留，
供 runtime 的 `needsSignIn` 与重试判断使用。

---

## 3. 传输实现要点

### 3.1 stdio

TS 行为：

- 按 `\n` 切分 JSON-RPC；`\r` 结尾兼容；空行跳过。
- 单条消息上限 16 MiB；超限清空缓冲并发 error，不杀进程。
- stderr 只保留尾部 64 KiB，同时交给 `onStderr`。
- 关闭顺序：`stdin.end()` → 500ms 宽限 → SIGTERM → 2s → SIGKILL；
  用进程组 `kill(-pid, sig)`，确保 `npx`/`uvx` 的子进程一起退出。
- spawn 失败（命令不存在）reject；spawn 后进程退出只触发 close。
- 退出时若有残留未闭合 JSON，报 `incomplete JSON-RPC message`。

Rust：

- `tokio::process::Command` + `Stdio::piped()`；Unix 用
  `std::os::unix::process::CommandExt::process_group(0)` 建进程组。
- 读 stdout 用 `BufReader::read_until(b'\n')`；注意单行超过上限时不能
  无限增长缓冲，要按块丢弃直到换行。
- stderr 用独立 task 读入 `VecDeque<u8>`，超过 64 KiB 从头部丢弃。
- close 用 `tokio::time::timeout` 实现宽限/强制；Unix 用
  `libc::kill(-pid as i32, signal)`；失败回退 `child.kill()`。
- Windows 分支可先 `#[cfg(windows)]` 用 `taskkill /pid <pid> /T /F`；
  本项目主要 macOS/Linux，但代码不能无条件用 Unix API。

### 3.2 streamable HTTP

TS 行为（最容易翻错的地方）：

- `POST` 头：`accept: application/json, text/event-stream`、
  `content-type: application/json`；消息体就是 JSON-RPC。
- 通知/response 的 POST 返回 202/204，忽略 body；收到
  `notifications/initialized` 后开始 GET 流。
- 请求的 POST：
  - `application/json`：body 可能是单条或数组，逐条 `emitMessage`。
  - `text/event-stream`：后台消费流；收到 response 即完成。
  - 202/204 对 request 是错误。
  - 其他 content-type 报错。
- 记录 `Mcp-Session-Id`；之后每个请求带 `MCP-Protocol-Version`。
- 401 或 403 + `insufficient_scope` 交给 `AuthProvider.onUnauthorized`
  一次；重试时重新取 token。`signal` 只阻止 dispatch，不中止共享刷新。
- 404 且已有 session → `McpSessionExpiredError`，runtime 换 session 重试。
- GET 流 405 表示 server 不提供，直接返回 `None`，不报错。
- SSE 解析：
  - `data` 多行用 `\n` 拼接；空行 dispatch。
  - `:` 开头是注释；`event`/`id`/`retry` 字段。
  - `id` 含 `\0` 忽略；`retry` 只接受纯数字。
  - 单事件上限 16 MiB；未完成事件按字节累计。
  - 只有 `event === None` 或 `"message"` 且 data 非空才解析 JSON-RPC。
- 响应流断开：只有**收到过 id** 且未回答时，才用 GET +
  `Last-Event-ID` 续传；最多 5 次，指数退避（1s 起，30s 上限）。
- GET 流断开：同样重连；405 直接结束；非瞬时错误报 error 后结束。
- close：abort 所有请求；已有 session 时尽力 `DELETE`（1s 超时），
  不向 auth provider 要新 token。

Rust：

- `reqwest::Client` 复用；`Response::bytes_stream()` 需要 `stream` feature。
- SSE 解析器单独函数 `consume_sse_stream(stream, options)`，用
  `tokio::io::AsyncBufRead` 或手写字节缓冲；注意 UTF-8 跨 chunk 的
  解码（用 `String` 累积 + `from_utf8_lossy` 会破坏多字节，最好用
  `encoding_rs` 或 `String::from_utf8` 处理不完整尾部）。
- `CancellationToken` 用于 close/abort；`select!` 处理 sleep 与 close。
- `Last-Event-ID`、`Mcp-Session-Id`、`MCP-Protocol-Version` 用
  `reqwest::header` 常量或字符串。

### 3.3 in-memory

给 client/oauth 测试用。Rust 用 `Arc<Mutex<Option<Weak<...>>>>` 连接
peer，`send` 用 `tokio::spawn` 异步投递，模拟 TS 的 `queueMicrotask`。

---

## 4. OAuth 实现要点

### 4.1 类型与发现

- `parse_protected_resource_metadata` / `parse_authorization_server_metadata`
  / `parse_oauth_tokens` / `parse_client_information`：TS 的 `absent()`
  把 `undefined`/`null`/`""` 都当缺失；Rust 用
  `Option<String>::filter(|s| !s.is_empty())`。
- `safe_url` 只接受 `http:`/`https:`；用于 `resource`、issuer、
  authorization/token/registration endpoint。
- `parseWwwAuthenticate`：只处理 `Bearer`/`DPoP`；`resource_metadata`、
  `scope`、`error`、`error_description`；空值视为缺失。
- `discoverProtectedResourceMetadata`：
  - 先试 `/.well-known/oauth-protected-resource{path}`；
  - path 非 `/` 且 4xx/502 时再试根路径；
  - 非 2xx 报 `HTTP {status} loading OAuth protected resource metadata`。
- `buildAuthorizationServerDiscoveryUrls` 顺序：
  1. `/.well-known/oauth-authorization-server{path}`
  2. `/.well-known/openid-configuration{path}`
  3. `{path}/.well-known/openid-configuration`
- issuer 校验：去掉尾部 `/` 后必须等于传入的 authorization server URL。
- `discoverOAuthServerInfo`：protected resource 失败只有
  `TypeError`（网络错误）才抛出；其他错误忽略并继续。
- `selectResource`：origin 必须相同，path 前缀匹配；返回 metadata.resource。

### 4.2 Flow

- PKCE：32 字节随机 → base64url verifier；SHA-256 → base64url challenge。
- `startAuthorization`：
  - metadata 不支持 `code` / `S256` 时报错；
  - `response_type=code`、`client_id`、`code_challenge`、
    `code_challenge_method=S256`、`redirect_uri`、可选 `state`/`scope`；
  - scope 含 `offline_access` 时加 `prompt=consent`；
  - 可选 `resource`。
- `tokenRequest`：
  - `token_endpoint` 必须 https 或 loopback；
  - client auth 方法选择：hint → supported → secret 有无；
  - body 先检查 `error` 字段，再检查 HTTP status；
  - `refreshAuthorization` 返回时保留原 refresh_token（server 未返回时）。
- `runFlow`：
  - 有 metadata URL 时不读缓存；否则先读 discovery cache。
  - `scope = options.scope || resourceMetadata.scopes_supported.join(" ") ||
    provider.clientMetadata.scope`（**`||` 不是 `??`**，空串要落到下一个）。
  - 没有 client 信息时：有 `clientMetadataDocument` 用 document URL 作为
    `client_id`；否则 DCR 并保存。
  - code exchange 前做 RFC 9207 `iss` 校验。
  - 有 refresh token 时先刷新；`invalid_grant` 才重跑流程，`invalid_client`/
    `unauthorized_client` 清空全部凭据后重跑。
  - 刷新/换取成功时，若响应没有 `scope`，保存时补上请求的 scope。
- `adaptOAuthProvider`：
  - 并发 401 共享一次刷新；
  - 如果 context.token 与当前 token 不同，说明别人已刷新，直接重试；
  - `insufficient_scope` 用 `stepUpScope(granted, challenged)`，并
    `skipRefresh = true`；
  - 结果为 `REDIRECT` 时抛 `McpOAuthAuthorizationRequiredError`。

### 4.3 Provider 与存储

- `McpOAuthProvider`：
  - 状态只认 `state.serverUrl == 本 provider 的 serverUrl`，防止跨 server
    泄凭据；
  - `saveTokens` 计算 `tokensExpireAt = now + expires_in*1000`；
  - 所有写入串行化（TS `writes = writes.then(...)`）；Rust 用
    `tokio::sync::Mutex` 或 `Arc<Mutex<...>>`。
- `McpOAuthCredentialStore`（coding-agent 侧）：
  - `{agent_dir}/mcp-auth.json`；
  - key = `mcp__<server>|<normalized url>`，兼容 legacy key = url；
  - 刷新锁：每个 key 一个锁文件，stale 20s，等待 25s，100ms 重试，
    `onCompromised` 不抛错；
  - `with_refresh_lock` 期间从读 token 到保存新 token，防止 refresh
    token 轮换被并发刷新弄丢。
  - Rust 用 `fs2::FileExt::try_lock_exclusive` 做文件锁；`fs2` 已在
    `pi-coding-agent` 依赖中。锁的 stale/等待语义用轮询实现。

### 4.4 回环回调

- 默认 `127.0.0.1`，`/callback`；`callbackUrl` 的 host 若是 `localhost`
  则监听 `127.0.0.1`，redirect host 仍写 `localhost`。
- `callbackPort`/端口冲突：固定端口失败且非 required 时退回随机端口。
- `waitForCallback(state, path)`：
  - 非本 server path 404；state 未知 400；
  - `pending.path` 不匹配时 reject "arrived on another redirect URI"；
  - `error` 参数 reject；缺 code reject；
  - 成功返回 `{code, state, iss?}`。
- 超时 5 分钟；关闭时 reject 所有 pending。

Rust：
- `tokio::net::TcpListener`，每次连接读请求行 + headers 到
  `\r\n\r\n`；只处理 `GET`；返回固定 `text/plain` 或 HTML。
- `wait_for_callback` 返回 `oneshot::Receiver`；用 `Mutex<HashMap<String,
  Pending>>` 保存 state；close 时 drain。
- 两个并发等待（浏览器 vs 粘贴 URL）用 `tokio::select!`；取消时
  abort 另一侧。

---

## 5. coding-agent 集成设计

### 5.1 配置类型（`core/mcp_servers.rs`）

对齐 `core/mcp-servers.ts`：

```rust
pub enum McpExposure { Codemode, Deferred, Direct, Hidden }

pub struct McpServerConfigBase {
    pub exposure: Option<McpExposure>,       // default Codemode
    pub description: Option<String>,
    pub tool_exposure: IndexMap<String, McpExposure>,
    pub enabled: Option<bool>,               // default true
    pub timeout: Option<f64>,                // seconds, default 60
}

pub struct McpStdioServerConfig { base, command, args, env, cwd }
pub struct McpHttpServerConfig { base, url, headers, oauth: Option<McpOAuthConfig>, auth: Option<AuthProviderRef> }
pub enum McpServerConfig { Stdio(..), Http(..) }
```

校验规则必须逐条保留：
- server 名 `^[A-Za-z0-9_-]+$`；
- `exposure`/`toolExposure` 取值；`codemode-deferred` 别名 → `codemode`；
- `enabled` bool；`description` string；`timeout > 0`；
- `type: "sse"` **拒绝**（TS v1.0.0 已移除 legacy SSE）。pi-rs 保留 SSE
  作为有意偏差：接受 `type: "sse"`，但登记到 `DEVIATIONS.md`；
- `url` 必须 http/https；`headers` 字符串映射；
- `oauth.callbackPort` 1..=65535；`callbackUrl` 必须是 loopback http，
  无 query/fragment；`callbackUrl` 端口与 `callbackPort` 一致；
- `clientRegistration` 只能是 auto/dcr/cimd；cimd 不能与 clientId/
  clientName 同用，且 callbackUrl 必须是 localhost/127.0.0.1 的 `/callback`；
- `authServerMetadataUrl` 必须 https，或 loopback http；
- `auth.provider` 非空；`auth` 只能出现在全局文件；
- 项目文件里没有 `command`/`url`/`type` 的条目是 override，只能设置
  `enabled`/`exposure`/`toolExposure`；
- 命名空间冲突：`-`/`_` 归一后相同报错。

### 5.2 namespace 与工具名

```rust
pub fn mcp_namespace(server: &str) -> String {
    format!("mcp__{}", server.replace('-', "_"))
}

pub fn create_mcp_tool_name(server: &str, tool: &str, is_taken: impl Fn(&str) -> bool) -> String
```

规则：
- 原始名 `mcp__{server}__{tool}` 中非 `[A-Za-z0-9_]` → `_`；
- 超过 64 字符或重名 → `sha256(server\0tool)[0..8]` 后缀；
- 同一 server 内多个工具 sanitize 后重名时，**所有**冲突工具都带 hash
  后缀（TS 的 `plain.indexOf(candidate) !== plain.lastIndexOf(candidate)`）。

### 5.3 连接状态机（`core/mcp/runtime.rs`）

状态：`Connecting | Connected | Disconnected | NeedsAuth | Failed | Closed`。

- 懒连接：第一次 `get_client()` 才真正连接。
- 连接失败：
  - HTTP 瞬时错误（408/429/5xx 除 501）和网络错误重试，延迟 250ms、
    1000ms；
  - stdio 不重试；
  - 失败保存 stderr 尾部 2000 字符；
  - 需要登录 → `NeedsAuth`，错误消息提示 `/mcp` 或 `/login <provider>`。
- 调用：
  - 只读操作遇到瞬时 HTTP 错误重试一次；
  - 404 session expired → 丢弃 client，重连一次；
  - auth 失败 → `drop_client` + `NeedsAuth`；
  - 工具调用不因瞬时错误重试（可能已经执行）。
- `notifications/tools/list_changed` → `list_tools()` + `on_tools`，
  重新注册。
- transport 关闭 → `Disconnected`，下次调用重连。
- `close()`：abort shutdown，关闭 client，等待 OAuth 刷新 settled。

### 5.4 工具转换（`core/mcp/tools.rs`）

- `readableResources`：资源链接文本里提示 `read_mcp_resource`。
- 结果转换：
  - `resource_link` → 文本（URI/title/mime/size/description + 读取提示）；
  - `resource` 且 blob 非图片：
    - text/json/xml MIME 直接解码为文本；
    - 其他保存到临时文件（0600），返回路径；
  - 其他走 `to_llm_content`；
  - `isError` 且没有文本 → 补 `MCP tool {server}/{tool} returned an error`；
  - 文本超过 20 KiB → 中间截断，保存全文到临时文件，提示
    `[Full output: <path> (read it with offset/limit)]`；
  - `structuredContent` 原样放进 `details`/脚本结果（去掉 `_meta`）。
- `parameters`：MCP schema 缺 `type` 补 `object`，缺 `properties` 补 `{}`。
- `annotations`：只保留四个 boolean hint。
- 工具 `exposure`：`codemode` → `deferred`；其余原样。
- `namespace`：`{ name: mcp__server, aliases: [server], description?, instructions? }`。

pi-rs 的 `ToolCallOutput` 目前只有 `content/details/is_error/terminate`，
需要新增 `structured_content` 或把整个 `CallToolResult` 放进 `details`。
建议给 `ToolDefinition`/`ToolCallOutput` 增加 `structured_content:
Option<Value>`，以支持 codemode 之外的脚本/后续特性。

### 5.5 Resources 三工具

完全按 `resources.ts`：

- `list_mcp_resources`、`list_mcp_resource_templates`、
  `read_mcp_resource`；
- 参数 `server`/`cursor`，输出 JSON schema 见 TS；
- 无 `server` 时遍历所有有资源的 server，按名字排序，用
  `Promise.allSettled` → Rust `futures::future::join_all` 收集错误；
- `cursor` 只在指定 server 时允许；
- 过滤 `ui://` 和 `mimeType` 含 `profile=mcp-app` 的 MCP App 资源；
- `read` 多个 contents 时每个前加 `{uri}:`；
- 二进制资源保存临时文件；
- 输出超过 20 KiB 同样截断。

### 5.6 Exposure / tool_search

Rust 核心扩展点需要新增：

1. `ToolDefinition` 增加：
   - `exposure: ToolExposure`（`Direct | ModelOnly | Codemode | Deferred | Hidden`）
   - `namespace: Option<ToolNamespace>`
   - `output_schema: Option<Value>`
   - `annotations: Option<ToolAnnotations>`
   - `reaches_tools: Option<Arc<dyn Fn(&Value) -> ReachesTools + Send + Sync>>`
2. Session/agent 维护 active tool 集合（现有 `selected_tools` 可复用，
   但需要暴露 `get_all_tools` / `get_active_tools` / `set_active_tools`）。
3. `tool_search`：
   - BM25 排序（`tokenize`/`stem`/`schema_text`/`Bm25Ranker` 直译）；
   - 只搜索 `codemode`/`deferred` 且不在 active 集合的工具；
   - 命中后加入 active 集合，返回 `{ loaded: [names] }`；
   - `reaches_tools() -> All`。
4. MCP extension 的 `ensureDiscoveryActive`：
   - 有 `codemode` exposure 时尝试激活 codemode（本 port 不实现
     codemode，只告警）；
   - 有 `deferred` 时激活 `tool_search`；
   - 两者都不可用时只告警一次。
5. 系统提示 `mcp_servers` section：
   - 只列 enabled 且有 codemode/deferred 的 server；
   - 格式 `- mcp__server (codemode|tool_search): summary`；
   - 总长 4096 字符，单 server 描述 250 字符，超出截断/省略；
   - 无内容时删除该 section。

### 5.7 扩展 `registerMcpServer`

TS `pi.registerMcpServer(name, config)` + `getMcpServers()` +
`mcp_servers_change` 事件。Rust 需要：

- `McpServerRegistry`（`register`/`unregister`/`get`/`list`/`set_change_listener`）；
- `HookHandler` 新增 `register_mcp_servers(&self, &mut McpServerRegistry)`，
  注册时调用一次；
- 运行期动态注册：在 `RuntimeHandle` 增加
  `register_mcp_server: Arc<dyn Fn(Value) -> Result<(), String>>` 与
  `get_mcp_servers: Arc<dyn Fn() -> Vec<Value>>`，由 session 持有
  registry 并触发 MCP extension 的 change handler；
- `mcp_servers_change` 事件：可作为新的 hook `on_mcp_servers_change`
  或在 ExtensionRegistry 里用广播；MCP extension 收到后重新连接/断开。

### 5.8 Session 生命周期

MCP extension 需要在以下时机行动：

| 时机 | Rust hook | 行为 |
| --- | --- | --- |
| session start | `on_session_start` | 读配置、合并扩展注册的 server、启动后台连接 |
| before agent start | 现有 hook? | 渲染 `mcp_servers` section；首次 prompt 等待 direct server（10s 上限） |
| tool call 前 | `on_tool_call` 或包装 | 等对应 server ready；`tool_search`/resource 工具等全部 |
| turn start | `on_turn_start` | 检测外部 `pi mcp login` 写入凭据后重连 |
| mcp servers change | 新增 hook | 连接/断开新增/移除的 server |
| session shutdown | `on_session_shutdown` | abort、关闭所有连接、等待后台任务 |

注意：Rust 扩展 API 目前没有 `before_agent_start` 的 system prompt
section 注入，也没有 `ctx.signal`。需要补：
- `HookHandler::before_agent_start` 返回/修改 system prompt sections；
- `ExtensionContext` 增加 session abort signal 或 `RuntimeHandle::abort_signal()`。

### 5.9 `/mcp` TUI 与 CLI

- `/mcp` 子命令：`/mcp`、`/mcp login [server]`、`/mcp logout [server]`、
  `/mcp reconnect [server]`。
- TUI 管理器：TS 是自定义 `McpManagerView`。pi-rs 的
  `ExtensionUIContext` 只有 `select/input/confirm/notify/status`，
  没有 `ui.custom`。两条路：
  1. 先实现等价的菜单序列（select 列表 + input 粘贴 URL + notify
     状态），覆盖 login/logout/reconnect/enable/disable/exposure；
  2. 在 pi-tui 增加 Extension 自定义组件/模式（工作量大）。
  建议本期走 1，并在 `DEVIATIONS.md` 登记"TUI 管理器为菜单序列，
  不是自定义组件"。
- `pi-rs mcp` 子命令：现有 `list/add/remove/enable/disable` 保留，
  补 `login`/`logout`，`list` 增加 exposure/tools/resources/state/JSON
  输出，`add` 增加 `--exposure`/`--description`/OAuth 参数。
- `pi-rs mcp login`：连接 → 判断 needs-auth → 启动回环回调 →
  打开浏览器 → 允许粘贴 redirect URL → 保存 → reconnect。

### 5.10 OAuth 凭据存储

- `{agent_dir}/mcp-auth.json`，结构 `{ "<mcp__server>|<url>": McpOAuthState }`。
- 兼容 legacy key（只有 URL）。
- 文件权限 0600。
- 刷新锁文件：`{agent_dir}/mcp-auth-refresh-<sha256(key)[0..16]>`。
- `tokens()` 只读、不接管 legacy；`remove()` 删除对应 key。
- `mcp.log`：`notifications/message` 一行一条，5 MiB 轮转到 `.1`，
  写失败静默。

---

## 6. 必须改动的核心（Phase 0）

以下不是 MCP 本身，但不做就无法实现"全量 MCP"：

1. **ToolDefinition 扩展**（`pi-extension-api`）：
   `exposure`/`namespace`/`output_schema`/`annotations`/`reaches_tools`/
   `structured_content`。
2. **Active tools 暴露**（`pi-agent-core`/`pi-coding-agent`）：
   `get_all_tools`/`get_active_tools`/`set_active_tools`；确保 active
   set 变化会写入 session（`ActiveToolsChange`）并影响下一次模型请求。
3. **`tool_search`**（`pi-coding-agent`）：BM25 + active set 更新。
4. **系统提示 section**：`before_agent_start` 可写
   `systemPromptOptions.sections`。
5. **扩展注册表动态能力**：`registerMcpServer` / `getMcpServers` /
   `mcp_servers_change`。
6. **Session abort signal 传给扩展**：MCP 后台连接/登录需要能随
   session 取消。
7. **`pi-tui` 扩展 UI**：至少 `select`/`input`/`notify` 可用；
   理想情况增加自定义组件。
8. **`/login` provider token**：`auth.provider` 需要
   `ModelRegistry::get_api_key_for_provider()` 等价能力。

---

## 7. 高危翻译陷阱（补充到 `PORTING.md`）

| TS 模式 | 坑 | Rust 正确做法 |
| --- | --- | --- |
| `scope = a \|\| b \|\| c` | 空串要落到下一个，`??` 不会 | 用 `filter(!is_empty)` 链式 `or_else` |
| `nextCursor === null \|\| nextCursor === ""` 视为结束 | serde `Option<String>` 会把 `null` 变 `None`，但 `""` 仍是 `Some("")` | 显式 `filter(|s| !s.is_empty())` |
| `structuredContent?: unknown` | 数组/标量/`null` 都合法 | `Option<Value>`，不能 `Option<Map>` |
| `response.body?.cancel()` | Rust 没有"取消 body"概念 | 丢弃 `Response`/`ByteStream` 即可；DELETE 用短 timeout |
| `AbortSignal.any([...])` | Rust 没有组合 signal | `CancellationToken` child token + `select!`，或 `tokio::select!` 监听多个 token |
| `setTimeout(...).unref()` | Rust task 不阻止 runtime 退出，无需 unref | 直接用 `tokio::time::sleep` |
| `Promise.allSettled` | Rust `join_all` 不会自动捕获错误 | 每个 future 返回 `Result`，再收集 `Ok/Err` |
| `JSON.stringify` 作为 refresh 锁 key | Rust 直接用规范化 URL 字符串 | 先 `url::Url` 规范化，再 hash |
| `Buffer` 字节长度 vs JS 字符串长度 | 截断/上限必须按 UTF-8 字节 | 用 `str::len()`/`as_bytes()`，不要按 `chars().count()` |
| `structuredClone` | Rust 用 `serde_json::Value::clone` | 深拷贝；不要共享可变引用 |
| 进程组 kill | `child.kill()` 只杀直接子进程 | Unix `process_group(0)` + `kill(-pid, sig)` |
| SSE 多字节跨 chunk | `String::from_utf8_lossy` 逐块会破坏 UTF-8 | 保留未完成字节，或用增量解码器 |
| JSON-RPC id 数字/字符串 | Rust `serde(untagged)` 可能把 `1` 解析成 `String` | 显式 `Value` 判别或自定义 Deserialize |
| `FileAuthStorageBackend.withLock` | 同步文件锁在 async 中阻塞 | 用 `spawn_blocking` 或 `fs2` 短临界区；不要持锁 await 网络 |

---

## 8. 测试与验收

### 8.1 `pi-mcp` 单元测试（从 TS test 翻译）

- `client.test.ts`：请求/响应、超时、取消、进度、关闭、分页、
  非法消息、初始化校验。
- `stdio.test.ts`：逐行 JSON、stderr 尾部、关闭序列、命令不存在。
- `streamable-http.test.ts`：JSON/SSE 响应、session、401 重试、
  404 session expired、GET 流、reconnect、SSE 解析边界。
- `oauth.test.ts`：PKCE、DCR、discovery、issuer 校验、refresh、
  `insufficient_scope`、callback server、state 隔离。
- `content.test.ts`：`to_llm_content` 的 text/image/audio/resource link/
  embedded resource/structured fallback。

### 8.2 coding-agent 契约测试

- `mcp.json` 加载/合并/override/校验/命名冲突。
- `createMcpToolName` 的长度/重名/非法字符。
- `convertMcpResult` 的截断/图片/二进制/错误。
- resources 三工具的 server 过滤/分页/错误聚合。
- exposure 解析、`mcp_servers` section 的 4096/250 限制。
- BM25 排序的 exact-name/coverage/name-weight 行为。
- OAuth store 的 legacy key 迁移、刷新锁、跨 server 隔离。

### 8.3 事件序列对齐

给定 mock server（in-memory transport）：
- `initialize` → `notifications/initialized` → `tools/list`；
- `tools/list_changed` → 重新 `tools/list` → 工具重新注册；
- OAuth 401 → `onUnauthorized` → 重试；
- 关闭 → pending 全部 reject，`onClose` 只触发一次。

### 8.4 集成测试

- `cargo test --workspace`；
- `cargo clippy --all-targets -- -D warnings`；
- `pi-rs mcp list/add/remove/login/logout` 的离线 fixture；
- stdio fixture server（Rust 或 Node 脚本）不依赖网络。

---

## 9. 分阶段实施顺序

### Phase 0 — 核心扩展点（前置）
1. `ToolDefinition` 新字段 + `ToolExposure`/`ToolNamespace`/`ToolAnnotations`。
2. active tools 的 get/set 与 session 记录。
3. `before_agent_start` 的 system prompt sections。
4. Extension `registerMcpServer` / `getMcpServers` / change hook。
5. Session abort signal 暴露给扩展。
6. `tool_search`（BM25）。

### Phase 1 — `crates/pi-mcp` 协议层
- protocol/jsonrpc/types/content、transport trait、in-memory、client。
- 测试：client + content。

### Phase 2 — `crates/pi-mcp` 传输层
- stdio、streamable HTTP（含 SSE 解析/重连/session/OAuth 接线）。
- 测试：stdio + streamable HTTP。

### Phase 3 — `crates/pi-mcp` OAuth
- types/discovery/flow/provider/callback/errors。
- 测试：oauth。

### Phase 4 — coding-agent 配置与连接
- `core/mcp_servers.rs`、`core/mcp/config.rs`、`runtime.rs`、`log.rs`。
- 替换 `core/mcp.rs`/`mcp_config.rs`/`mcp_sse.rs`（或保留 SSE 适配层）。
- 测试：config + runtime。

### Phase 5 — 工具与 resources
- `tools.rs`、`resources.rs`、临时文件/截断、structured content。
- 测试：tool name + result conversion + resources。

### Phase 6 — exposure/tool_search/系统提示
- `mcp_servers` section、`ensureDiscoveryActive`、direct/deferred。
- 测试：exposure + section + BM25。

### Phase 7 — `/mcp` TUI + CLI
- `/mcp` 菜单序列、login/logout/reconnect、`pi-rs mcp` 扩展。
- 测试：CLI fixture + TUI 状态机（不要求逐组件复刻）。

### Phase 8 — 收尾
- 更新 `DEVIATIONS.md`、`PORTING_MISTAKES.md`、`CONTRACT_ALIGNMENT.md`、
  `MILESTONE.md`；
- 全量测试 + clippy；
- 对抗式复核（干净上下文只看 diff）。

---

## 10. 需要登记的有意偏差

| 偏差 | 原因 | 状态 |
| --- | --- | --- |
| 不实现 codemode / `codemode` exposure 执行 | 用户明确排除；codemode 是独立大特性 | 已确认保留 |
| 保留 legacy SSE transport | 现有 DEVIATIONS #14 已确认；TS v1.0.0 已移除 | 已确认保留 |
| 配置目录 `.pi-rs` 而非 `.pi` | 项目既有命名 | 已确认保留 |
| 项目 `mcp.json` 始终加载（不做 trust 门控） | 项目既有"全部可信"偏差 | 已确认保留 |
| `/mcp` TUI 用菜单序列而非自定义组件 | `ExtensionUIContext` 无 `ui.custom`；可后续补 | 待确认（本期默认） |
| CIMD 的 `pi.dev` Client ID Metadata Document | pi-rs 不是 `pi`，是否继续用 `pi.dev` 文档需确认 | 待确认（默认禁用 CIMD，仅 DCR） |

---

## 11. 进入阶段二前的确认点

1. 是否按本文的 Phase 0→8 顺序执行？
2. `/mcp` TUI 首期是否接受"菜单序列"而不是自定义 `McpManagerView`？
3. OAuth CIMD 是否默认禁用（只用 DCR + 预注册 client）？pi-rs 的
   `APP_NAME` 不是 `pi`，直接复用 `https://pi.dev/oauth` 会误导用户。
4. 现有 `core/mcp.rs`/`mcp_config.rs`/`mcp_sse.rs` 是否允许在 Phase 4
   直接替换（保留 `mcp_sse` 到 `pi-mcp` 或适配层）？

确认后进入阶段二：先给 `crates/pi-mcp` 的类型骨架 + 测试，再写实现。
