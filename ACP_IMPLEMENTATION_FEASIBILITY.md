# pi-coding-agent 实现 ACP 协议（agent 侧）工作量分析

> 分析日期：2026-08-08
> 范围：**只做 ACP 协议（agent 侧 / 服务端），不做 TUI 集成**
> 目标：让任何 ACP 客户端（grok-build pager、Zed、其他 ACP 客户端）能连上
> pi-coding-agent

---

## 1. 结论摘要

**工作量不大，且协议层是现成的，不用自己写。**

- ACP 的传输层 + 类型定义 + trait 骨架全部在 crates.io 现成 crate 里
  （`agent-client-protocol` 3,380 行 + `agent-client-protocol-schema` 16,323 行），
  直接加依赖即可，**不需要自己实现 JSON-RPC 解析/请求响应关联/流式传输**。
- 实际工作 = **实现 `Agent` trait（16 个方法，核心 5 个）+ 把 pi 的
  AgentSession/AgentEvent 映射成 ACP 方法/通知**。
- 估算：
  - **最小可用**（initialize/authenticate/new_session/prompt/cancel + 事件映射 +
    stdio 传输）：**1~2 周**
  - **完整核心**（+ load_session/list_sessions/set_session_model/
    set_session_config_option/ext_method）：**3~4 周**
  - **全功能**（+ permission 双向审批流 + auth 完整适配 + MCP）：**5~8 周**

---

## 2. ACP 协议层是现成的

| 组件 | 来源 | LOC | 说明 |
| --- | --- | --- | --- |
| `agent-client-protocol` | crates.io v0.10.4 | 3,380 | JSON-RPC 传输层（`rpc.rs`：请求/响应关联、通知、流广播）+ `Agent` trait + `Client` trait |
| `agent-client-protocol-schema` | crates.io v0.11.4 | 16,323 | 全部消息类型（Initialize/Prompt/SessionUpdate/ToolCall/Permission/Auth/Ext…） |

`Agent` trait 共 16 个方法，**只有 5 个是核心**，其余都有默认实现
（返回 `method_not_found`），按需覆盖即可：

| 方法 | 是否核心 | 默认行为 |
| --- | --- | --- |
| `initialize` | ✅ 必须 | —（协议版本协商 + capabilities 声明） |
| `authenticate` | ✅ 必须 | —（auth method 列表 + 认证） |
| `new_session` | ✅ 必须 | —（创建会话） |
| `prompt` | ✅ 必须 | —（整个 prompt turn：流式输出 + 工具调用 + 权限请求） |
| `cancel` | ✅ 必须 | —（取消进行中的 turn） |
| `load_session` | 可选 | `method_not_found` |
| `set_session_mode` | 可选 | `method_not_found` |
| `set_session_config_option` | 可选 | `method_not_found` |
| `list_sessions` | 可选 | `method_not_found` |
| `ext_method` / `ext_notification` | 可选 | 返回空/OK |
| `set_session_model`（unstable） | 可选 | `method_not_found` |
| `fork_session` / `resume_session` / `close_session` / `logout`（unstable） | 可选 | `method_not_found` |

---

## 3. 方法映射：ACP 方法 → pi 现有能力

### 核心 5 个

| ACP 方法 | pi 现有能力 | 新增工作量 |
| --- | --- | --- |
| `initialize` | 无（新写） | 小：声明协议版本 + capabilities（半天） |
| `authenticate` | `core/auth_storage.rs`、`core/auth_guidance.rs` | 小~中：把 pi 的 auth storage 适配成 ACP 的 auth method 模型（1~2 天） |
| `new_session` | `AgentSession::new`、`core/session_manager.rs` | 小：包一层（半天~1 天） |
| `prompt` | `AgentSession::prompt` + `Agent::subscribe` 事件流 | **中（核心）**：把 pi 的 prompt 生命周期接到 ACP 的流式通知上（3~5 天） |
| `cancel` | `Agent::abort` | 小（半天） |

### 可选（按需）

| ACP 方法 | pi 现有能力 | 新增工作量 |
| --- | --- | --- |
| `load_session` | `session_manager` 加载 + `load_messages_from_session` | 小~中（1~2 天） |
| `list_sessions` | `session_manager` | 小（半天） |
| `set_session_model` | `AgentSession::set_model` / `cycle_model` | 小（半天） |
| `set_session_config_option` | `set_thinking_level` / `get_available_thinking_levels` / model 列表 | 小~中（1~2 天） |
| `set_session_mode` | pi 无"mode"概念（只有 steering/follow_up 队列模式） | 中：要么映射到队列模式，要么声明不支持（1~2 天） |
| `ext_method` / `ext_notification` | `core/extensions/`（ExtensionRegistry、dispatcher） | 中（2~3 天） |

---

## 4. 事件映射：pi 事件 → ACP 通知（核心工作量）

ACP 的 agent → client 通知是 `SessionNotification { session_id, update }`，
`SessionUpdate` 枚举：

| ACP SessionUpdate variant | pi 对应事件 | 映射难度 |
| --- | --- | --- |
| `AgentMessageChunk(ContentChunk)` | `AgentEvent::MessageUpdate` → `AssistantMessageEvent::TextDelta/ThinkingDelta` | 低（字段一一对应） |
| `AgentThoughtChunk(ContentChunk)` | `AssistantMessageEvent::ThinkingDelta` | 低 |
| `ToolCall(ToolCall)` | `AgentEvent::ToolExecutionStart` | 低 |
| `ToolCallUpdate(ToolCallUpdate)` | `AgentEvent::ToolExecutionUpdate/End` | 中：ACP 有 running/completed/failed/cancelled 状态机 + 结构化输出，pi 的 start/update/end 需要映射 |
| `Plan(Plan)` | pi 无 plan 概念 | 高：pi 没有执行计划（plan）事件，要么不声明该能力，要么补 |
| `AvailableCommandsUpdate` | `get_commands` / slash_commands | 中 |
| `CurrentModeUpdate` | pi 无 mode 概念 | 中（同 set_session_mode） |
| `ConfigOptionUpdate` | model/thinking 变更事件 | 中 |
| `SessionInfoUpdate` | session 名称/元数据变更 | 中 |
| `UserMessageChunk` | 用户消息回显 | 低 |

pi 的 `ContentBlock`（Text/Thinking/ToolCall/Image）与 ACP 的 `ContentBlock`
结构几乎一致，`AssistantMessageEvent`（text_delta/thinking_delta/toolcall_*）
与 ACP 的 `ContentChunk` 也基本对应——**类型映射本身不难，难的是事件序列
对齐**（阶段三要求：同一输入下事件顺序必须一致）。

---

## 5. pi 现在没有、需要新增的部分

| 缺口 | 说明 | 工作量 |
| --- | --- | --- |
| **Permission 双向审批流** | ACP 的 `request_permission`（agent → client 请求，client → agent 响应）。pi 现在是内部 `trust_manager`/`project_trust`/`output_guard` 自动决策，**没有"外部客户端参与审批"的通道** | 中（3~5 天）：新增一个 permission 请求/响应通道，接上 pi 的 before_tool_call 钩子 |
| **Auth 完整适配** | ACP 的 `auth_required` 错误流 + `authenticate`。pi 有 auth_storage 但需要适配成 ACP 的 auth method 模型 | 小~中（1~2 天） |
| **MCP** | ACP 有 MCP server 管理（`mcp/servers`、`mcp/tools`）。**pi-coding-agent 完全没有 MCP** | 大（1~2 周+）：如果目标是 grok-build pager 的 MCP 视图才需要；只做协议可以**在 capabilities 里不声明 MCP 能力**，跳过 |
| **stdio 传输入口** | ACP 的 `rpc.rs` 支持任意 AsyncRead/AsyncWrite，需要写一个 `pi --acp` 入口（类似现有 RPC 模式的 stdin/stdout 循环） | 小（1 天） |

---

## 6. 工作量估算汇总

| 档位 | 内容 | 估算（单人全职） |
| --- | --- | --- |
| **最小可用** | core 5 方法 + 事件映射（text/thinking/tool）+ stdio 入口 | **1~2 周** |
| **完整核心** | + load/list/set_session_model/config options/ext + 权限流 | **3~4 周** |
| **全功能** | + auth 完整适配 + MCP + plan/mode 等高级能力 | **5~8 周** |

参考基准：
- 现有 RPC 模式（`modes/rpc/`，1,307 行）已经证明"把 AgentSession 接到一个
  wire 协议"这条路走得通，ACP 实现可以复用同样的 session 驱动模式。
- grok-build 的 shell 侧 ACP 实现（`session/acp_session*` 36,492 行）是
  "agent 本体 + ACP + xAI 私有扩展"的总和，不能直接类比；纯协议层
  （`agent-client-protocol`）只有 3,380 行且是现成的。

---

## 7. 建议

1. **先做最小可用版验证链路**：加 `agent-client-protocol` 依赖，实现 core 5
   方法 + stdio 入口，用 crate 自带的 `examples/agent.rs` 或 grok-build pager
   的 headless 模式连一次，验证 prompt/stream/abort 主链路。
2. **权限流是第二个优先级**：grok-build pager 的 permission 视图依赖它，
   没有权限流，pager 只能跑"全自动"模式。
3. **MCP 先声明不支持**：在 capabilities 里不声明 MCP 能力，客户端会优雅降级，
   不用一开始就做。
4. 事件序列对齐按 CLAUDE.md 阶段三做：用 mock LLM 响应，对比 pi RPC 事件序列
   和 ACP 通知序列。

---

## 附：数据来源

- `agent-client-protocol` v0.10.4 / `agent-client-protocol-schema` v0.11.4：
  从 static.crates.io 下载解包（`/tmp/acp-src/`、`/tmp/acp-schema-src/`）
- grok-build tarball：`/Users/wxx/Desktop/github/grok-build-main`
- pi-rs 分析 worktree：`/Users/wxx/Desktop/github/pi-rs-tui-analysis`

---

## 8. 补充：Zed / VSCode 可用性 + 更省事的路径（2026-08-08 实测）

### 8.1 有了 ACP 之后能不能直接用

**Zed：能，原生支持。** Zed 原生支持 ACP 外部 agent（`zed.dev/docs/ai/external-agents`），
在 `settings.json` 加 `agent_servers` 配置即可，agent 会出现在 Agent Panel /
Threads Sidebar。Zed 官方文档把 "Pi Coding Agent" 列为常见外部 agent。
pi-coding-agent 实现 ACP 后配置形如：

```json
"agent_servers": {
  "pi-rs": { "type": "custom", "command": "pi", "args": ["--acp"], "env": {} }
}
```

**VSCode：不能原生，但可以。** VSCode 没有内置 ACP，需要装社区扩展：
ACP Client（formulahendry/vscode-acp）、ACP Patchbay、ACP Pro、Multicoder、
Poolside Assistant 等（见 agentclientprotocol.com/get-started/clients.md）。
这些扩展通过 stdio 连 ACP agent。

### 8.2 重要发现：原版 pi 也不说 ACP，靠第三方适配器 pi-acp

原版 TS pi 在 ACP 官方 agent 列表里是 "Pi (via pi-acp adapter)"——
它靠第三方 Node 包 **pi-acp**（github.com/svkozak/pi-acp）桥接：
pi-acp 对外说 ACP（JSON-RPC over stdio），对内 spawn `pi --mode rpc`，
把 ACP 方法翻译成 pi 的 RPC 命令、把 pi 的 RPC 事件翻译成 ACP 通知。
pi-acp 用到的 16 个 RPC 命令（prompt/abort/get_state/get_available_models/
set_model/set_thinking_level/set_follow_up_mode/set_steering_mode/compact/
set_auto_compaction/get_session_stats/set_session_name/export_html/
switch_session/get_messages/get_commands）**pi-rs 的 RPC 模式全部已实现**。

### 8.3 实测发现：pi-rs 的 RPC 事件 wire 格式与 TS 不兼容（需先修）

用真实序列化测试验证（`cargo test` 实测输出）：

| 事件 | TS / pi-acp 期望格式 | pi-rs 实际输出 |
| --- | --- | --- |
| agent_start | `{"type":"agent_start"}` | `"agent_start"`（裸字符串） |
| message_update | `{"type":"message_update",...}` | `{"message_update":{...}}`（外部标签） |
| tool_execution_start | `{"type":"tool_execution_start",...}` | `{"tool_execution_start":{...}}` |

根因：pi-rs 的 `AgentSessionEvent` 枚举用 serde 默认外部标签
（`#[serde(rename_all = "snake_case")]` 但没有 `#[serde(tag = "type")]`），
而 TS 是 `type` 判别联合。**pi-acp 读 `event.type`，会拿到 undefined，直接不兼容。**
修复很小：给枚举加 `#[serde(tag = "type")]`（或写自定义 Serialize），
并核对 `message_update` 内 `assistantMessageEvent` 的字段名（camelCase）。
此问题未登记在 `DEVIATIONS.md`，按 CLAUDE.md 阶段四属于"未识别的回归 bug"。

### 8.4 三条路径对比（目标：在 Zed/VSCode 里用 pi-coding-agent）

| 路径 | 内容 | 工作量 |
| --- | --- | --- |
| A. 修 RPC 事件格式 + 用现成 pi-acp（Node） | 修 `#[serde(tag="type")]`，把 pi-rs 二进制指给 pi-acp 的 `PI_ACP_PI_COMMAND` | **1~2 天** |
| B. 把 pi-acp 移植成 Rust | pi-acp 约 2~3k 行 TS，翻译成 Rust 小 crate | **1~2 周** |
| C. pi-coding-agent 原生实现 ACP | 见本文第 6 节 | **1~2 周（最小可用）** |

路径 A 是"今天就能用"的捷径（前提：pi-rs 的 RPC 命令/响应格式与 TS 一致，
只有事件格式需要修）；路径 C 是长期正解（不依赖 Node、无第三方适配器、
可进 ACP Registry）。

---

## 9. 实现状态（2026-08-08）

**已完成并验证：**

- `modes/acp/` 新增原生 ACP mode（`pi-cli --acp`）：
  - `agent.rs` — 实现 `agent-client-protocol` 的 `Agent` trait（initialize/authenticate/
    new_session/load_session/prompt/cancel/list_sessions/set_session_model/
    set_session_config_option）
  - `session.rs` — session task 持有 AgentSession，`select!` 同时监听 prompt future、
    事件流、命令通道，`session/cancel` 可打断运行中的 prompt（无死锁）
  - `translate.rs` — pi AgentSessionEvent → ACP SessionUpdate（text/thinking/tool）
  - `mod.rs` — stdio 接线（AgentSideConnection + 通知转发）
- RPC 事件格式修复（`#[serde(tag = "type")]` + `to_json_event()` 剥离 partial）
- 测试：5 个 ACP 单测/集成测试 + 全量 729 测试通过
- 手动端到端验证：initialize / session/new / session/list / session/cancel /
  prompt 错误路径全部按 ACP 规范响应

**Zed 配置：**

```json
"agent_servers": {
  "pi-rs": { "type": "custom", "command": "pi-cli", "args": ["--acp"], "env": {} }
}
```

**已知限制（见 DEVIATIONS.md #10/#12/#13）：**

- 权限流（request_permission）未实现——pi 内部 trust 模型自动决策
- MCP 已实现（`mcp` feature 默认开启）：ACP `session/new`/`session/load` 的 `mcpServers`（stdio + streamable-HTTP）会连接并注入工具，调用转发回服务器。SSE 传输未实现（capabilities 声明 sse=false）；工具集在 session 创建时固定，本版本 ACP 规范 `prompt` 不再携带 mcpServers
- session/load 已实现跨进程持久化：每个 ACP session 写入 `{sessions_dir}/acp/{id}.jsonl` + `session-map.json`（sessionId→文件/cwd 映射），重启后可按 ID 恢复。已知约束：同一时刻只有一个 ACP 进程写入 session map（无并发锁）
- prompt 图片已接入：ACP `prompt` 的 `ContentBlock::Image` 提取后经 `PromptOptions.images` 传给 pi 的 `prompt()`（同 RPC 模式路径）
