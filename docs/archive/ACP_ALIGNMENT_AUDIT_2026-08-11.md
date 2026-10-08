# ACP 处理流程对齐审计 — pi-rs `modes/acp` vs pi-acp

> 审计日期：2026-08-11
> 对照基准：`github.com/svkozak/pi-acp`（`src/acp/*` + `src/pi-rpc/*`，约 4,000 行 TS）
> 审计对象：`crates/pi-coding-agent/src/modes/acp/`（agent.rs / session.rs / translate.rs /
> slash_commands.rs / mod.rs）
> 结论：pi-rs 是**原生 ACP 实现**（不 spawn 子进程），pi-acp 是 **Node 适配器**
> （对外 ACP、对内 spawn `pi --mode rpc`）。两者架构不同，但对外 wire 行为应对齐。

---

## 1. 架构差异（有意为之，非偏差）

| 维度 | pi-acp（TS 适配器） | pi-rs（原生） | 说明 |
| --- | --- | --- | --- |
| 会话载体 | spawn `pi --mode rpc --no-themes` 子进程，NDJSON over stdio | 进程内 `AgentSession` + 每会话一个 **actor task**：actor 独占命令 mailbox 与 `Arc<AgentSession>`，LLM turn 由独立 task 在本地 executor 上运行（turn 完成经 channel 通知 actor 取下一个排队 prompt）；`session/cancel` 走非阻塞 abort + cancelled 标志 | pi-rs 无子进程泄漏问题，`closeAllExcept`（单活子进程策略）不适用 |
| 事件来源 | RPC 事件流（`message_update`/`tool_execution_*`/`agent_settled` 等） | `AgentSessionEvent` 流（同构） | 事件序列对齐见 §3 |
| MCP | 不支持（capabilities 声明 http:false） | 支持 stdio + streamable-HTTP（`mcp` feature） | pi-rs 有意增强，见 DEVIATIONS 讨论 |
| 认证 | 声明 terminal auth（`--terminal-login` 重开二进制） | 不声明 authMethods | pi-rs 是原生二进制，API key 走 env/交互模式，无 `--terminal-login` 入口；见 §4.1 |
| 会话持久化 | pi 自己的 `~/.pi/agent/sessions/**` | `{sessions_dir}/acp/{id}.jsonl` + `session-map.json` | 见 §4.6 |

---

## 2. 已修复的偏差（本次审计）

| # | 位置 | pi-acp 行为 | pi-rs 修复前 | 修复 |
| --- | --- | --- | --- | --- |
| 1 | `initialize` | `sessionCapabilities: { list, delete }` | 只声明 `list` | 补声明 `close`（SDK 0.11.4 的 unstable 能力，对应 `session/close`） |
| 2 | `new_session` | 校验 cwd 为绝对路径，否则 `invalidParams` | 不校验 | 补校验 |
| 3 | `new_session` 响应 | 返回 `configOptions`（model + thought_level 选择器） | 只返回 sessionId | 补 `config_options` |
| 4 | `load_session` | 校验 cwd 绝对；`opts.cwd ?? stored.cwd` | 不校验；忽略 args.cwd | 补校验 + cwd 覆盖（`reg.load(..., cwd_override)`） |
| 5 | `load_session` 响应 | 返回 `configOptions` | 空响应 | 补 `config_options` |
| 6 | `list_sessions` | 按 cwd 过滤（默认 lastSessionCwd）；返回 title/updatedAt；50/页分页 | 无过滤、无 title/updatedAt、无分页 | 补过滤 + title/updatedAt + 分页 |
| 7 | `set_session_model` | 接受 `provider/model` 或裸 `model`（经 available models 解析）；改后发 `config_options_update` | 只接受 `provider/model`；不发通知 | 补裸 id 解析 + 通知 |
| 8 | `set_session_mode` | 校验 thinking level ∈ {off,minimal,low,medium,high,xhigh} | 不校验 | 补校验 |
| 9 | `set_session_config_option` | config id 为 `thought_level` | 用 `thinking_level` | 改为 `thought_level`（并兼容旧 `thinking_level` 输入） |
| 10 | `cancel` | 未知 session 静默 no-op | 返回错误 | 改 no-op |
| 11 | `prompt` | `message.trimStart().startsWith('/')` 才走 slash 命令 | 不 trim | 补 trim |
| 12 | 事件翻译 | `auto_retry_start/end`、`auto_compaction_start/end` → 消息 chunk | 丢弃 | 补翻译（pi-rs 对应 `AutoRetryStart/End`、`CompactionStart/End`） |
| 13 | 事件翻译 | tool call 状态单调（pending→in_progress→completed），已 surface 过的工具在 `tool_execution_start` 只发 `tool_call_update` | 重复发 `tool_call` | 补状态跟踪 |
| 14 | `/name` | 改完发 `session_info_update`（新 title） | 只发文本 | 补 `session_info_update` |
| 15 | `/export` | 导出到 `{cwd}/pi-session-{id}.html`，发 `resource_link` | 导出到进程 cwd，只发文本 | 补 cwd 路径 + `resource_link` |
| 16 | `available_commands_update` | 用 `get_commands`（含 skills/扩展/prompt 模板）+ builtin 合并 | 只有 file-based + builtin | 补 `get_commands` 来源 |
| 17 | `load_session` 历史回放 | toolResult 的 bash 渲染成 terminal | 纯文本 | 补 bash terminal 渲染 |
| 18 | prompt 排队 | 排队时发 "Queued message (position N)" + queueDepth 元数据 | 静默排队 | 补通知 |
| 19 | `list_sessions`/`load_session`/`delete` | 能发现/加载/删除 pi 自己 CLI 建的会话（扫 `~/.pi/agent/sessions`） | 只认 ACP map | 补 pi 会话目录回退（`scan_pi_sessions` 递归扫 base_dir，含 CLI 会话） |
| 20 | turn 期间命令处理 | 流式 turn 期间 `set_model`/`config` 等照常处理（RPC 请求与事件流并行） | turn 期间非 prompt 命令 `reject_busy`（GetConfigOptions/GetCommands 返回空表） | 改 actor 架构：只有 prompt 排队，其余命令立即处理（2026-08-12，SessionTask 重写） |
| 21 | cancel 非阻塞 | cancel 不阻塞会话命令处理 | cancel 在 select! 内 await `abort()`（等 idle） | `abort()` 移入独立 task（fire-and-forget），turn task 经 cancelled 标志报告 `cancelled` |
| 22 | turn 中 Shutdown | —（pi-acp 无对应） | turn 期间 Shutdown 被内层 select 消费，task 永不退出 | Shutdown 在主循环直接 break 并 abort 在飞 turn（bug 修复） |
| 23 | `translate.rs` `tool_execution_end`（bash） | pi-acp 在 `tool_execution_end` 对 bash 调 `emitBashOutputUpdate`：把未流式送出的剩余输出作为 `terminal_output` delta 冲刷 + `terminal_exit`，再 `cleanupToolCall` 清理状态 | 只发 `terminal_exit`，`bash_outputs` 永不清理，且 fields 带 `rawOutput` | 补剩余输出冲刷（与 exit 合并进同一个 meta，`meta()` 是替换不是合并）+ 清理状态 + bash 更新不带 rawOutput（2026-08-12） |
| 24 | `core/tools/bash.rs` 结束路径 | TS `finishOutput` 把剩余输出以 onUpdate 冲刷给客户端；非零退出时 throw 的 Error message 是**完整输出 + 状态行**（`appendStatus(outputText, "Command exited with code N")`），ToolResultMessage 因此含完整输出 | 成功路径不冲刷最终更新；错误 message 只有 `"Command failed with exit code N"`（输出丢失、措辞不同） | 对齐 TS：结束时按 dirty 标志冲刷原始 snapshot 内容；非零退出 error message 改为完整输出 + `Command exited with code N`（2026-08-12） |

---

## 3. 事件序列对齐（阶段三要求）

给定同一输入（prompt + mock LLM 响应），pi-acp 与 pi-rs 的 ACP 通知序列：

| 阶段 | pi-acp 通知 | pi-rs 通知 | 一致 |
| --- | --- | --- | --- |
| 流式文本 | `agent_message_chunk`(text_delta) | `AgentMessageChunk`(TextDelta) | ✅ |
| 流式思考 | `agent_thought_chunk`(thinking_delta) | `AgentThoughtChunk`(ThinkingDelta) | ✅ |
| 工具参数流式 | `tool_call`(pending) → `tool_call_update`(pending) | 同（修复 #13 后不再重复发 `tool_call`） | ✅ |
| 工具执行 | `tool_call_update`(in_progress) → `tool_call_update`(completed/failed) | 同 | ✅ |
| bash 终端 | `tool_call`(terminal content+meta) → `tool_call_update`(terminal_output 增量) → `tool_call_update`(terminal_exit) | 同 | ✅ |
| edit/write | `tool_call`(locations) → `tool_call_update`(diff) | 同 | ✅ |
| 自动重试 | `agent_message_chunk`("Retrying (attempt X/Y, waiting Zs)...") → ("Retry finished, resuming.") | 修复 #12 后同 | ✅ |
| 自动压缩 | `agent_message_chunk`("Context nearing limit...") → ("Automatic compaction finished...") | 修复 #12 后同 | ✅ |
| turn 结束 | `agent_settled` → resolve prompt(end_turn) | `AgentEnd` → resolve prompt(end_turn) | ✅（机制不同，语义一致） |
| 取消 | `cancel` → abort → resolve prompt(cancelled) | 同 | ✅ |

---

## 4. 已知偏差（有意保留 / 待确认）

### 4.1 认证（authMethods）— 待确认
pi-acp 在 `initialize` 声明 terminal auth method（`pi-acp --terminal-login` 重开二进制进交互终端）。
pi-rs 不声明。原因：pi-rs 是原生二进制，无 `--terminal-login` 入口；API key 通过 env 或
交互 TUI 配置。**影响**：Zed 在未配置 key 时不会显示 "Authenticate" 按钮。
**建议**：若需要，可在 pi-cli 加 `--terminal-login`（等价于跑交互模式）并声明该 auth method。

### 4.2 prompt 失败语义 — 有意保留
pi-acp：RPC prompt 失败 → stopReason 映射为 `end_turn`（除非已取消）。
pi-rs：prompt 未产生 `AgentEnd` → 返回 ACP internal error。
pi-rs 行为更正确（客户端能看到失败），保留。

### 4.3 `/queue` 内置命令 — 有意保留
pi-rs 额外提供 `/queue all|one-at-a-time`（同时设 steering + follow-up），pi-acp 没有。
pi-rs 特有命令，保留。

### 4.4 `/changelog` — 有意保留
pi-acp 读 npm 安装的 CHANGELOG.md；pi-rs 无捆绑 changelog，打印版本号并说明。
保留（代码注释已注明）。

### 4.5 startup info 内容 — 部分对齐
pi-acp 的 startup info 含 pi 版本 + Context + Skills + Prompts + Extensions + 更新提示；
pi-rs 目前只有版本 + cwd。Skills/Prompts/Extensions 枚举已补（见 session.rs
`build_startup_info`），更新提示（npm 检查）不适用（Rust 无 npm）。

### 4.6 会话持久化位置 — 有意保留
pi-acp 让 pi 把会话写在 `~/.pi/agent/sessions/**`（与 CLI 共享）；pi-rs 写在
`{sessions_dir}/acp/` 子目录 + `session-map.json`。pi-rs 的 `list_sessions`/`load_session`
已能回退扫描 pi 自己的会话目录（修复 #19），但 ACP 新建的会话仍落在 `acp/` 子目录，
普通 `pi` CLI 的 `/resume` 看不到。**待确认**：是否改为直接写 `{sessions_dir}` 根目录
（与 CLI 共享，但会与 CLI 会话混在一起）。

### 4.7 单活子进程策略 — 不适用
pi-acp 每个 ACP 连接只保留一个 pi 子进程（`closeAllExcept`）。pi-rs 是进程内会话，
无子进程泄漏，保留所有会话。有意保留。

### 4.8 MCP — 有意增强
pi-acp 声明 `mcpCapabilities: {http:false, sse:false}`；pi-rs 声明 `http: cfg!(feature="mcp")`
并实际连接 stdio/streamable-HTTP MCP server、注入工具。有意增强，保留。

### 4.9 权限流（extension_ui_request → requestPermission）— 待确认
pi-acp 把扩展的 `ui.select`/`ui.confirm` 桥接成 ACP `request_permission`（客户端弹权限
对话框）。pi-rs 的 ACP mode 未接线 `ExtensionUIContext`（`CreateAgentSessionOptions.ui_context`
为 None），扩展 UI 请求被静默丢弃。**影响**：依赖 `ui.select`/`ui.confirm` 的扩展在 ACP
客户端里无法交互。**建议**：接线 ACP 版 `ExtensionUIContext`（见 §5 方案）。

### 4.10 `load_session` 会话 cwd 与文件记录 cwd — 已对齐

pi-acp 用请求 cwd 覆盖存储 cwd（`opts.cwd ?? stored.cwd`）；pi-rs 已实现同样的覆盖
（`reg.load(..., cwd_override)`），但会话文件仍记录原始 cwd（与 pi-acp 一致——pi-acp 的
sessionFile 也保留原始 cwd）。

---

## 5. 后续建议（未实施）

1. **权限流**：在 `mod.rs` 加 `(RequestPermissionRequest, oneshot)` 通道，session task 持有
   `ExtensionUIContext`（select/confirm → `request_permission`，notify → 消息 chunk），
   通过 `CreateAgentSessionOptions.ui_context` 注入。工作量约 1~2 天。
2. **terminal auth**：pi-cli 加 `--terminal-login`（跑交互模式），`initialize` 声明
   `AuthMethodTerminal`。工作量约半天。
3. **会话目录共享**：评估把 ACP 会话直接写 `{sessions_dir}` 根目录（与 CLI `/resume` 共享）。
