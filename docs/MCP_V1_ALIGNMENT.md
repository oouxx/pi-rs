# MCP v1.0.0 对齐分析（pi-rs ← pi TS）

> 状态：**阶段一分析稿**（待用户确认范围后再进入阶段二）。
> 基准：TS `packages/coding-agent`（MCP 在 v0.82.1 之后加入，commit `8562bcf66`
> `feat(coding-agent): codemode and MCP`，v1.0.0 形态）。pi-rs 当前基线仍为
> v0.82.1，本文是用户明确要求的「超前对齐」分析。

## 实施状态（2026-10-09 更新）

用户决定**不照搬 TS，只按 pi-rs 现状做一个可用的原生 MCP 客户端**，因此本文
只作分析参考。已落地（见 `crates/pi-coding-agent/DEVIATIONS.md` #14）：

- ✅ `{agent_dir}/mcp.json` + `{cwd}/.pi-rs/mcp.json` 配置加载与合并
- ✅ stdio + streamable-HTTP + **legacy SSE**（`core/mcp_sse.rs` 自实现）
- ✅ 所有模式经 `create_agent_session` 注入 MCP 工具
- ✅ `pi-rs mcp list/add/remove/enable/disable`
- ❌ 未做：exposure/codemode/deferred、`tool_search`、MCP resources 工具、
  OAuth、`/mcp` TUI、扩展 `registerMcpServer`、`tools/list_changed` 刷新

下面第 2–7 节保留作为 TS v1.0.0 的差距参考。

---

## 1. 背景

pi-rs 现有的 MCP 只是 **ACP 模式的附带能力**：ACP 客户端在
`session/new`/`session/load` 传 `mcpServers`，`core/mcp.rs`（292 行，
`rmcp`，feature `mcp` 默认开）连接并注入 `custom_tools`。没有配置文件、
没有 `/mcp`、没有 `pi-rs mcp` 子命令，也没有 exposure/codemode/OAuth/resources。

TS v1.0.0 的 MCP 是一个**内置扩展**（`createMcpExtension()`），
约 4200 行，且深度依赖扩展系统与另外两个 v1.0.0 特性（codemode、tool_search）。

---

## 2. 模块列表和职责（TS 逐文件）

| 文件 | 行数 | 主要导出 | 职责 | 是否公开 API |
| --- | --- | --- | --- | --- |
| `core/mcp-servers.ts` | 303 | `McpServerConfig`/`McpStdioServerConfig`/`McpHttpServerConfig`/`McpOAuthConfig`/`McpExposure`、`validateMcpServerConfig`、`getMcpToolExposure`、`mcpNamespace`、`McpServerRegistry`、`RegisteredMcpServer` | 配置类型、校验、exposure 解析、扩展注册表 | ✅ 被扩展系统引用 |
| `extensions/mcp/config.ts` | 199 | `loadMcpConfig`、`updateMcpServerConfig`、`addMcpServerConfig`、`removeMcpServerConfig`、`McpServerEntry`、`LoadedMcpConfig` | 读/写 `mcp.json`（全局 + 项目），冲突检测 | 扩展内部 |
| `extensions/mcp/runtime.ts` | 473 | `McpServerConnection`、`createDefaultTransport`、`McpServerLog` | 连接/重连/懒连接、stdio+streamable-HTTP 传输、超时、会话过期重试、OAuth 接线 | 扩展内部 |
| `extensions/mcp/tools.ts` | 326 | `createMcpToolDefinition`、`createMcpToolName`、`convertMcpResult`、`limitMcpContent`、`toToolExposure` | MCP tool → pi tool 适配、结果转换/截断、临时文件 | 扩展内部 |
| `extensions/mcp/resources.ts` | 342 | `createMcpResourceToolDefinitions`、`LIST_MCP_RESOURCES_TOOL` 等三个资源工具 | `list_mcp_resources`/`list_mcp_resource_templates`/`read_mcp_resource` | 扩展内部 |
| `extensions/mcp/oauth.ts` | 461 | `createMcpAuthProvider`、`McpOAuthCredentialStore`、`signInMcpServer` | OAuth 2.1 + DCR + 回环回调 + token 存储 | 扩展内部 |
| `extensions/mcp/ui.ts` | 249 | `showMcpManager`、`McpUi`、`McpMenu` | TUI `/mcp` 管理器 | 扩展内部 |
| `extensions/mcp/log.ts` | 69 | `McpServerLog` | `notifications/message` → `mcp.log` | 扩展内部 |
| `extensions/mcp/cli.ts` | 610 | `runMcpCommand` | `pi mcp list/login/logout/enable/...` 子命令 | CLI |
| `extensions/mcp/index.ts` | 1181 | `createMcpExtension`、`renderServersSection`、`MCP_SERVERS_SECTION` | 内置扩展：生命周期、工具注册、exposure 决策、`/mcp`、`mcp_servers_change` | ✅ 经 resource-loader 装载 |

**依赖的下游 v1.0.0 特性（pi-rs 当前均无）：**

- `extensions/codemode/tool.ts` — `CODEMODE_TOOL_NAME`、`isCodemodeTool`（`packages/codemode` 包）
- `extensions/tool-search/tool.ts` — `TOOL_SEARCH_TOOL_NAME`、`isToolSearchTool`
- 扩展 API 的 `ToolExposure` / `ToolNamespace` / `outputSchema` / `annotations` /
  `prepareLoadout` / `getExposure` / `getNamespace`（`core/extensions/types.ts:509-640`）
- `pi.registerMcpServer()` / `pi.getMcpServers()` / `mcp_servers_change` 事件
- `ModelRegistry.getApiKeyForProvider()`（`auth.provider` 服务器取 token）

---

## 3. 核心数据结构

### 3.1 服务器配置（`core/mcp-servers.ts`）

- `McpExposure = "codemode" | "deferred" | "direct" | "hidden"`
  （别名 `codemode-deferred` → `codemode`；默认 `codemode`）。
- `McpServerConfigBase`：`exposure?`、`description?`、`toolExposure?`
  （精确名优先于含 `*` 的模式，模式按对象顺序首命中）、
  `enabled?`（默认 true）、`timeout?`（默认 60s）。
- `McpStdioServerConfig`：`command`、`args?`、`env?`（`${NAME}` / `!cmd`）、`cwd?`。
- `McpHttpServerConfig`：`url`、`headers?`、`oauth?`、`auth?{provider}`。
- `mcpNamespace(server) = "mcp__" + server.replace(/-/g,"_")`。
- 服务器名 `^[A-Za-z0-9_-]+$`；命名空间冲突（`-`/`_` 归一后相同）报错。

**Rust 映射**：`enum McpServerConfig { Stdio(..), Http(..) }`（serde
tagged by 有无 `command`/`url`，与 TS 的鸭子判别一致）+ `enum McpExposure`。

### 3.2 工具名（`extensions/mcp/tools.ts`）

- `createMcpToolName(server, tool, isTaken)`：`mcp__<server>__<tool>`，
  非 `[A-Za-z0-9_]` → `_`；超 64 字符或重名 → `sha256(server\0tool)[0..8]` 后缀。
- 结果：文本超 20KB 中部截断 + 存临时文件（0600）；图片透传；二进制资源存文件；
  `isError` → error result 但 codemode 脚本仍拿到 structuredContent。

**Rust 映射**：字符串处理函数直译；`rand`/`sha2` 已有或补齐。

### 3.3 连接状态机（`runtime.ts`）

`ServerState = connecting | connected | disconnected | needs-auth | failed | closed`。
- 懒连接（首次调用触发）；读操作对瞬时 HTTP 错误重试一次；
  `McpSessionExpiredError` 换 session 重试一次；
  stdio 断开 → `disconnected`，下次调用重连。
- `notifications/tools/list_changed` → 重列工具并重新注册。

### 3.4 扩展 API 扩展点（pi-rs 需新增）

`ToolDefinition` 需新增：`exposure`、`namespace`、`outputSchema`、
`annotations`、`prepareLoadout`；并让 session 维护
`getExposure(name)`/`getNamespace(name)` 与 active 集合。

---

## 4. 模块间依赖关系

```
config.ts ──uses──> core/mcp-servers.ts (校验/命名空间)
index.ts  ──uses──> config.ts, runtime.lazy→runtime.ts, tools.ts, resources.ts, oauth.ts, ui.ts
runtime.ts──uses──> @earendil-works/pi-mcp (McpClient/transports), resolve-config-value.ts, oauth.ts, log.ts
tools.ts  ──uses──> pi-ai(TextContent/ImageContent), pi-agent-core(AgentToolResult), pi-mcp(toLlmContent)
resources.ts ─uses─> tools.ts
cli.ts    ──uses──> config.ts, runtime.ts, log.ts
resource-loader.ts ──loads──> createMcpExtension()  (内置扩展)
runner.ts ──emits──> mcp_servers_change
system-prompt.ts ──reads──> MCP_SERVERS_SECTION (由 index.ts 写入 sections)
main.ts   ──dispatch──> `pi mcp` → cli.ts
```

无循环依赖。`runtime.lazy.ts` 只做动态 import，避免无 MCP 服务器时加载 MCP 客户端。

---

## 5. 对外接口

### 5.1 扩展 API（新增）

- `pi.registerMcpServer(name, config)` / `pi.getMcpServers()`
- `pi.registerTool(def)` 支持 `exposure`/`namespace`/`outputSchema`/`annotations`
- 事件 `mcp_servers_change`（`{ servers: RegisteredMcpServer[] }`）
- `ToolLoadout.getExposure/getNamespace`（codemode/tool_search 用）

### 5.2 命令

- `/mcp`（TUI 管理器）/ `/mcp login|logout|reconnect [server]`
- `pi-rs mcp` 子命令（`list`、`login`、`logout`、`enable`、`disable`、`add`、`remove`…）

### 5.3 系统提示

- `mcp_servers` section：列出**非 direct** 服务器的命名空间、摘要、
  到达方式（codemode / tool_search）；上限 4096 字符，单服务器描述上限 250 字符。

### 5.4 配置

- 全局 `{agentDir}/mcp.json`；项目 `{cwd}/.pi-rs/mcp.json`
  （pi-rs 已确认"全部可信任"，故项目文件始终加载——与 TS 的 trust 门控不同，
  这条要并入既有「项目信任」偏差）。
- 项目文件不允许 `auth`（只能全局）。
- `autoEnableCodemode`（默认 true）。

---

## 6. pi-rs 现状与差距

| 能力 | pi-rs 现状 | 差距 |
| --- | --- | --- |
| ACP `mcpServers` | ✅ `core/mcp.rs` | stdio+http、SSE 不支持（已登记 DEVIATIONS #14） |
| `mcp.json` 配置 | ❌ | 全缺 |
| `/mcp` / `pi-rs mcp` | ❌ | 全缺 |
| MCP resources 工具 | ❌ | 全缺 |
| OAuth | ❌ | 全缺 |
| exposure / codemode / deferred | ❌（ToolDefinition 无 exposure） | 依赖 codemode、tool_search |
| `env`/`header` `${VAR}`/`!cmd` 解析 | ⚠️ 有 `resolve_config_value.rs` 可复用 | 需接入 |
| 扩展 `registerMcpServer` | ❌ | 缺失 |

---

## 7. 分阶段实施计划（建议）

> 阶段划分按依赖顺序；每阶段走阶段二（类型→测试→实现→对抗式复核）+ 阶段三验证。

**Phase 0 — 基础设施（前置于 MCP）**
- 扩展 `ToolDefinition`：`exposure`、`namespace`、`outputSchema`、`annotations`、
  `prepareLoadout`；session 维护 exposure/namespace/active 集合。
- 实现 `tool_search` 工具（`deferred` 依赖）。
- （可选）实现 `codemode` 工具（`codemode` exposure 依赖；`packages/codemode` 独立包，量大）。

**Phase 1 — MCP 核心（无 codemode 也可用）**
- `core/mcp-servers.rs`（配置类型/校验/exposure 解析/命名空间/注册表）。
- `extensions/mcp/config.rs`（读写 `mcp.json`）。
- `extensions/mcp/runtime.rs`（连接状态机、stdio+http、超时、重连、日志）。
- `extensions/mcp/tools.rs`（工具适配、结果转换/截断、临时文件、`direct`/`deferred`）。
- system prompt `mcp_servers` section；启动报告。
- 内置扩展装载。

**Phase 2 — resources + CLI + `/mcp`**
- `resources.rs` 三个工具。
- `pi-rs mcp` 子命令（list/login/logout/enable/disable/add/remove/…）。
- `/mcp` TUI 管理器 + `mcp_servers_change` + `registerMcpServer` API。

**Phase 3 — OAuth**
- `oauth.rs`：OAuth 2.1 + DCR + 回环回调 + `mcp-auth.json` + `/mcp login`。

**Phase 4 — codemode exposure（若纳入）**
- 依赖 Phase 0 的 codemode 工具；`searchTools`/`describeNamespace` 等。

---

## 8. 待确认问题（阻塞阶段二）

1. **范围**：是否要求完整对齐（含 codemode、OAuth、resources、`/mcp` TUI），
   还是先做 Phase 1（config + 连接 + `direct`/`deferred` 工具 + CLI list）？
   codemode 本身是独立大特性（`packages/codemode`），建议单独排期。
2. **默认 exposure**：TS 默认 `codemode`。若暂不做 codemode，pi-rs 的默认
   exposure 需要改成 `direct` 或 `deferred`（属有意偏差，需登记 DEVIATIONS）。
3. **ACP 与配置 MCP 的关系**：保留现有 ACP `mcpServers` 通道（两条来源并存），
   还是合并到统一注册表？
4. **项目 `mcp.json` 信任**：沿用已确认的"全部可信任"，即项目 `mcp.json` 始终加载？
5. **OAuth**：是否需要（很多远程 MCP 依赖它），还是先只支持 header/`auth.provider`？
