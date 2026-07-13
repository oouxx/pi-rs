# pi-coding-agent Extension 系统对齐原版计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 pi-rs 的 `pi-coding-agent` extension 子系统（`crates/pi-coding-agent/src/core/extensions/`）对齐原版 `@earendil-works/pi-coding-agent` 的 TypeScript 扩展系统，使原版 pi 扩展能在嵌入 `deno_core` 运行时下高保真运行。

**Architecture:** 扩展运行在独立的 V8 线程（`ExtensionRuntime`），主线程通过 `RuntimeCommand` 通道与 V8 通信。所有跨边界数据必须 `Send`（`serde_json::Value` / 字符串）。事件分两类：**fire-and-forget**（通知型，串行 await，结果忽略）与 **result-returning**（可短路/合并/替换，影响主流程）。本计划在现有 Phase 1 骨架上补齐缺失的事件接入、Context/Actions 能力、命令/快捷键/标志消费、以及扩展加载语义。

**Tech Stack:** Rust, deno_core (V8), deno_ast (swc TS 转译), tokio, serde_json

---

## 审查结论（当前状态 vs 原版）

### 已实现（Phase 1 产物，可用）

- **发现与加载**：`discover_extensions` 扫描 `{cwd}/.pi-rs/extensions/` + `{agent_dir}/extensions/` + 显式路径；支持 `extension.json` / `package.json` 的 `pi.extensions`；deno_ast 转译 TS/TSX。
- **工具注册与调用**：`registerTool` 元数据回传 Rust，JS handler 留在 V8；`create_extension_agent_tools` 把扩展工具合并进 `AgentSession` 工具列表；first-registration-wins 在 Rust 侧 `get_all_extension_tools` 实现。
- **result-returning 事件**：`tool_call`（短路 block）、`tool_result`（合并 content/details/isError）—— 已接入 `AgentSession` 的 `before_tool_call` / `after_tool_call` 钩子。
- **fire-and-forget 事件**：`agent_start` / `agent_end` / `turn_start` / `turn_end` / `message_start` / `message_update` / `tool_execution_start` / `tool_execution_end` —— 通过 `AgentEvent` 订阅 + `fire_and_forget_from_agent_event` 派发。
- **ops**：`op_pi_register_tool/command/shortcut/flag`、`op_pi_get_flag`、`op_pi_get_commands`、`op_pi_exec`（async，30s 默认超时）、`op_pi_notify`、`op_pi_log`。
- **鲁棒性**：`COMMAND_TIMEOUT=120s` 包裹所有 oneshot await；`ExtensionError::Timeout`；Drop 不阻塞 join；空扩展列表不 spawn V8；`dispose()` 调 `stop()`。

### 缺失或简化（本计划要补的）

| # | 缺失项 | 原版行为 | Rust 现状 |
|---|--------|---------|-----------|
| G1 | `context` 事件 | 每次 LLM 调用前扩展可改 messages | JS 侧 `__piDispatchResult` 有合并逻辑，**Rust 侧从未触发** |
| G2 | `message_end` 事件（result-returning） | 扩展可替换最终消息 | JS 侧有合并逻辑，dispatcher.rs 注释 `phase 2`，跳过 |
| G3 | `before_provider_request` 事件 | 扩展可替换 provider payload | JS 侧有合并逻辑，**Rust 侧从未触发** |
| G4 | `input` 事件 | 扩展可 transform/handle/continue 用户输入 | 完全未实现（JS 侧无、Rust 侧无） |
| G5 | `project_trust` 事件 | 扩展参与项目信任决策（顺序，首个 yes/no 胜出） | 完全未实现 |
| G6 | `session_*` 事件族 | `session_start` / `session_before_switch` / `session_before_fork` / `session_before_compact` / `session_compact` / `session_before_tree` / `session_tree` / `session_info_changed` / `session_shutdown` | 完全未实现 |
| G7 | `resources_discover` 事件 | 扩展贡献 skill/prompt/theme 路径 | 完全未实现 |
| G8 | `after_provider_response` 事件 | 通知型，扩展看 status/headers | 完全未实现 |
| G9 | `model_select` / `thinking_level_select` 事件 | 通知型 | 完全未实现 |
| G10 | `user_bash` 事件 | 扩展可改 `!`/`!!` 命令的 operations/result | 完全未实现 |
| G11 | `tool_execution_update` 事件 | 高频流式更新 | dispatcher 注释 `high-frequency; skip`（**有意延后，本计划不做**） |
| G12 | `ctx.ui` 能力 | `select/confirm/input/editor/notify/setStatus/setWorkingMessage/...` | 仅 `notify`（缓冲）+ `setStatus`（no-op）；对话框/主题/widget 全无 |
| G13 | `ctx` 动作方法 | `isIdle/isProjectTrusted/abort/hasPendingMessages/shutdown/getContextUsage/compact/getSystemPrompt` | 全部 no-op/stub |
| G14 | `pi` API 动作 | `sendMessage/sendUserMessage/appendEntry/setSessionName/setLabel/setModel/setThinkingLevel/setActiveTools/registerProvider/unregisterProvider/navigateTree/registerMessageRenderer/registerEntryRenderer` | 全部 `notSupported` throw 或硬编码空 |
| G15 | `pi.events` 跨扩展事件总线 | emit/on | no-op |
| G16 | slash 命令消费 | 扩展命令进 `/` 补全 + 可执行 | Rust 注册了元数据，`slash_commands.rs` **未消费扩展命令** |
| G17 | 快捷键消费 | 扩展 shortcut 与内置 keybinding 合并 + 冲突诊断 | `op_pi_register_shortcut` 是 stub，无 Rust 侧消费 |
| G18 | flag 持久化与消费 | CLI 解析 `--flag`，扩展可读 | `op_pi_get_flag` 返回 None；CLI 未把 flag 值回灌 |
| G19 | `ExtensionCommandContext` 能力 | `waitForIdle/newSession/fork/navigateTree/switchSession/reload/getSystemPromptOptions` | 完全未实现（命令 handler 还跑不起来） |
| G20 | mode/hasUI 语义 | `tui/rpc/json/print` 四态 | JS 硬编码 `mode:"rpc", hasUI:false` |
| G21 | 错误监听与诊断 | `ExtensionError` + `errorListeners` + shortcut/command `ResourceDiagnostic` | 无错误监听通道；无诊断上报到宿主 |
| G22 | 扩展加载语义偏差 | 目录约定 `.pi/extensions/`（全局 `~/.pi/agent/extensions/`），`-e` 扩展不可热重载，jiti 缓存 + watch 热重载 | Rust 用 `.pi-rs/extensions/`（**目录名与原版不一致**）；无热重载；无 `-e` 不可重载标记 |
| G23 | 命令名冲突解析 | 同名命令分配 `cmd:1`/`cmd:2` invocationName | 无 |
| G24 | 工具渲染继承 | 覆盖内置工具省略 `renderCall`/`renderResult` 时回退内置 | 无（Rust 侧无渲染层概念，TUI 未接） |
| G25 | `types.rs` 旧死代码 | — | 旧 `load_extensions`/`LoadedExtension` 被 `resource_loader.rs` 调用但结果丢弃，与新 JS 系统重复（Phase 2 待清理） |

---

## 全局约束

- **只动 `crates/pi-coding-agent`**（+ 必要时 `pi-agent-core` 的小手术，如 `message_end` 事件暴露）。不碰 `pi-tui`、`pi-ai` 的对外 API（`on_message_end` 的 pi-agent-core 改动需单独评估，见 Phase 4）。
- **跨边界只传 `Send` 数据**：所有进入 V8 或从 V8 返回的数据走 `serde_json::Value` / `String`，禁止 V8 handle 跨线程。
- **每个 deno_core API 签名查 0.404 源码确认**（`/Users/xinxing/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/deno_core-0.404.0/`），不靠记忆。
- **错误必须显式传播**：禁 `.unwrap()`/`.expect()`（测试除外）；扩展系统错误用 `thiserror` 的 `ExtensionError`；宿主接入点 fail-open 时必须 `eprintln!` 记录，不静默吞。
- **TDD**：每个新事件接入 / 新 op / 新能力，先写测试（`tests/extensions_test.rs` 或新测试文件）→ 实现 → 测试通过。
- **行为对齐不改结构**：result-returning 事件的串联/短路/合并语义必须与原版 `runner.js` 对应 `emitXxx` 逐字对齐（顺序、短路条件、合并字段）。
- **JS 侧聚合逻辑已就绪**：`runtime.js` 的 `__piDispatchResult` 已实现 `tool_call`/`tool_result`/`message_end`/`context`/`before_provider_request` 五种聚合。**本计划主要是补 Rust 侧的"触发"与"解析"**，JS 侧仅需为新事件类型补聚合分支（G4/G5/G6/G7/G10）。
- **不逐行复刻 TUI 渲染**：`ctx.ui` 的对话框/widget/主题能力（G12）在本计划中只做 **RPC/JSON 模式下的语义通道**（payload 透传到宿主事件流），TUI 实际渲染留给 `pi-tui` 后续任务。

---

## 文件结构

### 修改：核心接入
```
crates/pi-coding-agent/src/core/extensions/
├── dispatcher.rs       ── 补 context/message_end/before_provider_request/input/user_bash/session_*/resources_discover/project_trust 分发函数
├── runtime.rs          ── RuntimeCommand 扩展（若需新命令类型）；dispatch_result/fire_and_forget 复用现有
├── ops.rs              ── 补 op_pi_get_flag 真实实现、op_pi_register_shortcut/flag 真实存储、op_pi_set_flag_value、op_pi_get_shortcuts/flags、op_pi_send_message/send_user_message/append_entry/set_session_name/set_label（接入宿主）
├── runtime.js          ── 补 input/project_trust/session_*/resources_discover/user_bash 的聚合分支；补 pi API 真实实现（非 notSupported）
├── types.rs            ── 补事件 payload/result 类型；Phase 2 删旧死代码
└── mod.rs              ── re-export 新增类型
```

### 修改：宿主接入点
```
crates/pi-coding-agent/src/core/
├── agent_session.rs        ── 接入 context/message_end/before_provider_request（result-returning）；补 fire-and-forget 事件族触发
├── agent_session_runtime.rs ── provider 请求前触发 before_provider_request
├── sdk.rs                  ── 启动时触发 project_trust/session_start/resources_discover；关闭时 session_shutdown
├── session_manager.rs      ── new/fork/switch/compact/tree 触发对应 before_* 事件
├── slash_commands.rs       ── 消费扩展命令（G16）；命令名冲突解析（G23）
├── compaction.rs            ── 触发 session_before_compact / session_compact
└── resource_loader.rs       ── 接入 resources_discover 结果合并 skill/prompt/theme 路径（G7）；清理旧死代码（G25）
```

### 修改：CLI / 设置
```
crates/pi-coding-agent/src/
├── cli/args.rs            ── 扩展 flag 解析回灌（G18）
├── cli/run.rs             ── flag 值注入 runtime
└── core/settings_manager.rs ── extensions/packages 字段接入发现逻辑
```

### 新增测试
```
crates/pi-coding-agent/tests/
├── extensions_test.rs       ── 扩展（既有，补新事件用例）
├── extension_events_test.rs ── 新增：事件序列对齐黄金用例（result-returning 串联/短路/合并）
└── extension_integration_test.rs ── 新增：端到端扩展加载 + 事件触发（离线 fixture）
```

---

## Phase 2：result-returning 事件接入（高价值，低风险）

> JS 侧聚合逻辑已就绪，本阶段纯补 Rust 触发 + 解析。每个任务独立可提交。

### Task 2.1 — `context` 事件接入（G1）
- [ ] 2.1.1 在 `dispatcher.rs` 新增 `context_payload(messages: &[AgentMessage]) -> Value` 与 `dispatch_context(&ExtensionRuntime, &ctx, messages) -> Result<Option<Vec<AgentMessage>>>`，解析返回 `{ messages: [...] }`。
- [ ] 2.1.2 在 `agent_session.rs` 的 LLM 调用前（context 组装后）调用 `dispatch_context`，用返回值替换 messages（若 `Some`）。
- [ ] 2.1.3 测试：扩展返回替换 messages → Rust 侧拿到替换后的数组；扩展返回 null → messages 不变；多扩展串联（后者看到前者修改后的）。
- [ ] 2.1.4 验证 `cargo test -p pi-coding-agent` + `cargo clippy --all-targets -- -D warnings`。

### Task 2.2 — `before_provider_request` 事件接入（G3）
- [ ] 2.2.1 `dispatcher.rs` 新增 `dispatch_before_provider_request(&rt, &ctx, payload: &mut Value) -> Result<()>`，内部调 `dispatch_result("before_provider_request", ...)`，用返回 `{ payload }` 原地替换。
- [ ] 2.2.2 在 `agent_session_runtime.rs`（或 http_dispatcher 调用点）provider 请求构建后、发送前调用。注意：payload 是 `unknown`，原版直接替换整个请求体。
- [ ] 2.2.3 测试：扩展返回新 payload → 请求体被替换；返回 undefined → 不变；串联合并。
- [ ] 2.2.4 clippy + test。

### Task 2.3 — `message_end` 事件接入（G2，需 pi-agent-core 评估）
- [ ] 2.3.1 确认 `pi-agent-core` 的 `AgentEvent::MessageEnd` 是否携带可变 message 引用或可订阅 result。若不可变，记录到 spec：本事件需 pi-agent-core 暴露"消息结束时可改写"的钩子。
- [ ] 2.3.2 `dispatcher.rs` 新增 `dispatch_message_end(&rt, &ctx, message: &AgentMessage) -> Result<Option<AgentMessage>>`，解析 `{ message }`，校验 `role` 一致（不一致则跳过 + `eprintln!`，对齐原版 runner.js 行为）。
- [ ] 2.3.3 接入 `agent_session.rs` 的 `MessageEnd` 事件订阅（当前 dispatcher.rs:139 是 `None // phase 2`）。
- [ ] 2.3.4 测试：role 一致 → 替换；role 不一致 → 保留原消息 + 记录错误；串联。
- [ ] 2.3.5 clippy + test。

### Task 2.4 — `user_bash` 事件接入（G10）
- [ ] 2.4.1 `runtime.js` 的 `__piDispatchResult` 新增 `user_bash` 分支：顺序执行，首个返回非 undefined 的 handler 胜出（短路），返回 `{ operations, result }`。
- [ ] 2.4.2 `dispatcher.rs` 新增 `dispatch_user_bash(&rt, &ctx, command, cwd) -> Result<Option<UserBashResult>>`。
- [ ] 2.4.3 接入 `!`/`!!` 命令执行路径（`slash_commands.rs` 或 `cli/initial_message.rs` 的 bash 处理）。
- [ ] 2.4.4 测试 + clippy。

---

## Phase 3：fire-and-forget 事件族接入（中价值，低风险）

> 纯通知，不返结果。复用现有 `dispatch_fire_and_forget`。

### Task 3.1 — session 生命周期事件（G6）
- [ ] 3.1.1 定义事件 payload 构造函数（`session_start{reason}` / `session_shutdown{reason,targetSessionFile?}` / `session_info_changed{name}`）。
- [ ] 3.1.2 `sdk.rs` 启动时触发 `session_start { reason: "startup" }`；`dispose`/退出时触发 `session_shutdown { reason: "quit" }`。
- [ ] 3.1.3 `session_manager.rs` 在 new/resume/fork 前触发对应 `session_before_*`（result-returning，可 `cancel`）+ 后置 `session_start { reason }`。
- [ ] 3.1.4 `compaction.rs` 触发 `session_before_compact`（result-returning，可 cancel/提供 compaction）+ `session_compact`。
- [ ] 3.1.5 测试：`cancel:true` 短路后续 handler 且阻止操作；正常流程事件序列对齐。

### Task 3.2 — `resources_discover` 事件（G7）
- [ ] 3.2.1 `runtime.js` 新增 `resources_discover` 聚合分支：全部执行，合并 `skillPaths`/`promptPaths`/`themePaths`。
- [ ] 3.2.2 `dispatcher.rs` 新增 `dispatch_resources_discover(&rt, cwd, reason) -> Result<ResourcesDiscoverResult>`。
- [ ] 3.2.3 `resource_loader.rs` 在启动 / reload 后调用，把返回路径合并进 skill/prompt/theme 加载列表。
- [ ] 3.2.4 测试：多扩展贡献路径合并；空 handler 返回空。

### Task 3.3 — `project_trust` 事件（G5）
- [ ] 3.3.1 `runtime.js` 新增 `project_trust` 聚合分支：顺序执行，首个返回 `trusted:"yes"|"no"` 的 handler 胜出（短路），`"undecided"` 继续；全 undecided 返回 null。
- [ ] 3.3.2 `dispatcher.rs` 新增 `dispatch_project_trust(&rt, cwd) -> Result<Option<ProjectTrustResult>>`。
- [ ] 3.3.3 `sdk.rs` / `project_trust.rs` 在内置信任流程前调用；若有扩展返回 yes/no，跳过内置流程；`remember` 字段透传。
- [ ] 3.3.4 测试：yes/no 短路；undecided 回退内置；串联顺序。

### Task 3.4 — `after_provider_response` / `model_select` / `thinking_level_select`（G8/G9）
- [ ] 3.4.1 三个 fire-and-forget 事件，payload 分别为 `{status,headers}` / `{model,previousModel,source}` / `{level,previousLevel}`。
- [ ] 3.4.2 接入 provider 响应点、`/model` 切换点、思考级别切换点。
- [ ] 3.4.3 测试 + clippy。

---

## Phase 4：`input` 事件与命令系统消费（高价值，中风险）

### Task 4.1 — `input` 事件接入（G4）
- [ ] 4.1.1 `runtime.js` 新增 `input` 聚合分支：顺序串联 transform（修改 text/images 后传给下一个）；首个 `action:"handled"` 立即短路；全 `continue` 返回 `{action:"continue"}`。
- [ ] 4.1.2 `dispatcher.rs` 新增 `dispatch_input(&rt, text, images, source, streaming_behavior) -> Result<InputEventResult>`。
- [ ] 4.1.3 接入用户输入路径（`cli/initial_message.rs` / `modes/interactive.rs` / `modes/print_mode.rs`），在 skill/template 展开前调用。
- [ ] 4.1.4 `handled` → 丢弃输入；`transform` → 用新 text/images 继续；`continue` → 原样。
- [ ] 4.1.5 测试：transform 链式；handled 短路；continue 透传。

### Task 4.2 — slash 命令消费（G16 / G23）
- [ ] 4.2.1 `ops.rs` 的 `op_pi_get_commands` 改为真实返回 JS 注册的命令列表（当前是 stub 空数组）。
- [ ] 4.2.2 `slash_commands.rs` 启动时从 `ExtensionRuntime` 拉取命令列表，合并进 `/` 补全 + 命令分发。
- [ ] 4.2.3 实现命令名冲突解析：同名命令分配 `cmd:1`/`cmd:2` invocationName（对齐 runner.js `resolveRegisteredCommands`）。
- [ ] 4.2.4 命令 handler 通过 `ExtensionRuntime` 调用 JS handler，传入 `ExtensionCommandContext`（先做 `cwd` + `getSystemPrompt`，其余 G19 能力按 Phase 5 进度接入）。
- [ ] 4.2.5 测试：扩展命令出现在补全；同名冲突分配后缀；handler 执行返回通知。

### Task 4.3 — `ExtensionCommandContext` 能力（G19）
- [ ] 4.3.1 `runtime.js` 的 `makeContext` 增加命令上下文方法（`waitForIdle/newSession/fork/navigateTree/switchSession/reload/getSystemPromptOptions`），通过新 op 回调宿主。
- [ ] 4.3.2 `ops.rs` 新增对应 ops（`op_pi_new_session` / `op_pi_fork` / `op_pi_switch_session` / `op_pi_reload` / `op_pi_wait_for_idle`），通过 oneshot 把请求传回主线程执行。
- [ ] 4.3.3 宿主侧（`agent_session.rs` / `session_manager.rs`）实现这些 handler。
- [ ] 4.3.4 测试 + clippy。

---

## Phase 5：`pi` API 与 `ctx` 能力补齐（中价值，中风险）

> 逐个把 `notSupported` / no-op 换成真实实现。每个独立可提交，按使用频率排序。

### Task 5.1 — flag 真实化（G18）
- [ ] 5.1.1 `op_pi_register_flag` / `op_pi_get_flag` 改为真实存储（Rust `PiOpState` 持 flag 元数据 + 值 Map，或 JS `flagValues` 已有但 `op_pi_get_flag` 改读 JS）。
- [ ] 5.1.2 `cli/args.rs` 解析扩展 flag（`--flag` / `--flag=value`），`cli/run.rs` 启动时调 `runtime.set_flag_value(name, value)` 回灌。
- [ ] 5.1.3 测试：默认值生效；CLI 覆盖；扩展读取。

### Task 5.2 — shortcut 真实化（G17）
- [ ] 5.2.1 `op_pi_register_shortcut` 真实存储到 Rust；新增 `op_pi_get_shortcuts`。
- [ ] 5.2.2 实现 keybinding 合并 + 保留键保护（`RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS`）+ 冲突诊断 `ResourceDiagnostic`。
- [ ] 5.2.3 接入 TUI（gated `#[cfg(feature = "interactive")]`，无 feature 时跳过）。
- [ ] 5.2.4 测试 + clippy。

### Task 5.3 — 消息注入 API（G14 子集：sendMessage/sendUserMessage/appendEntry）
- [ ] 5.3.1 `ops.rs` 新增 `op_pi_send_message` / `op_pi_send_user_message` / `op_pi_append_entry`，oneshot 回主线程。
- [ ] 5.3.2 `runtime.js` 替换 `notSupported` 为真实调用。
- [ ] 5.3.3 宿主 `agent_session.rs` 的 `send_custom_message` / `send_user_message` 已存在，接上。
- [ ] 5.3.4 测试 + clippy。

### Task 5.4 — session 元信息 API（G14 子集：setSessionName/getSessionName/setLabel）
- [ ] 5.4.1 新增 ops，接入 `session_manager.rs`。
- [ ] 5.4.2 测试 + clippy。

### Task 5.5 — model/thinking API（G14 子集：setModel/setThinkingLevel/getThinkingLevel）
- [ ] 5.5.1 新增 ops，接入 `model_resolver.rs` / `agent_session.rs`。
- [ ] 5.5.2 测试 + clippy。

### Task 5.6 — provider 注册 API（G14 子集：registerProvider/unregisterProvider）
- [ ] 5.6.1 新增 ops，接入 `model_registry.rs`；处理 `pendingProviderRegistrations` 队列（原版 runtime 初始化时 flush）。
- [ ] 5.6.2 测试 + clippy。

### Task 5.7 — `ctx` 动作方法真实化（G13）
- [ ] 5.7.1 `isIdle` / `isProjectTrusted` / `hasPendingMessages` / `getSystemPrompt` / `getContextUsage` / `compact` / `abort` / `shutdown` 改为通过 op 回调宿主真实状态。
- [ ] 5.7.2 测试 + clippy。

### Task 5.8 — `ctx.ui` 非对话框能力（G12 子集：setStatus/setWorkingMessage/setTitle 等）
- [ ] 5.8.1 定义 UI 事件通道（payload 透传到宿主事件流 / RPC），TUI 渲染留给 pi-tui。
- [ ] 5.8.2 `notify` 已就绪；补 `setStatus`/`setWorkingMessage`/`setTitle` 的 op + 宿主事件。
- [ ] 5.8.3 对话框（`select/confirm/input/editor`）：RPC 模式下走 JSON 协议请求-响应；无 UI 模式返回默认值（对齐 `noOpUIContext`）。
- [ ] 5.8.4 测试 + clippy。

### Task 5.9 — `pi.events` 跨扩展总线（G15）
- [ ] 5.9.1 JS 侧实现 in-process EventEmitter（不跨 V8 边界）。
- [ ] 5.9.2 测试 + clippy。

---

## Phase 6：加载语义与错误处理对齐（中价值，低风险）

### Task 6.1 — 目录名对齐（G22）
- [ ] 6.1.1 决策：Rust 用 `.pi-rs/extensions/` 是有意偏离（避免与原版 pi 共用），还是 bug？**需与维护者确认**。若要对齐原版，改 `loader.rs` 的 `discover_extensions` 用 `.pi/extensions/` + 全局 `~/.pi/agent/extensions/`；否则在 `loader.rs` 顶部注释明确"有意偏离"。
- [ ] 6.1.2 同步 `docs/extensions.md`（若 Rust 侧有等价文档）。

### Task 6.2 — `-e` 扩展不可热重载标记（G22）
- [ ] 6.2.1 `ExtensionSource::Path` 标记为不可热重载；`User`/`Project` 可热重载。
- [ ] 6.2.2 `/reload` 时只重载可热重载的扩展。

### Task 6.3 — 热重载（G22，可选）
- [ ] 6.3.1 `fs_watch.rs` 已存在；接入扩展目录 watch → `clearExtensionCache` 等价 + `session_shutdown { reason:"reload" }` → 重新 `load()` → `session_start { reason:"reload" }` → `resources_discover { reason:"reload" }`。
- [ ] 6.3.2 测试 + clippy。

### Task 6.4 — 错误监听 + 诊断上报（G21）
- [ ] 6.4.1 `ExtensionRuntime` 暴露 `on_error(listener) -> ()`（或事件流 variant），把 JS handler 异常（当前 `op_pi_log` 吞掉）结构化为 `ExtensionError { extensionPath, event, error, stack }`。
- [ ] 6.4.2 宿主订阅错误 → 记录到 diagnostics / 显示。
- [ ] 6.4.3 shortcut/command 冲突诊断（`ResourceDiagnostic[]`）从 Rust 侧收集后上报。
- [ ] 6.4.4 测试 + clippy。

### Task 6.5 — mode/hasUI 语义（G20）
- [ ] 6.5.1 `runtime.js` 的 `makeContext` 接收 Rust 传入的 `mode`（`tui`/`rpc`/`json`/`print`）与 `hasUI`，不再硬编码。
- [ ] 6.5.2 `ExtensionRuntime::new` / `dispatch_*` 传入当前 mode。
- [ ] 6.5.3 测试 + clippy。

### Task 6.6 — `types.rs` 旧死代码清理（G25）
- [ ] 6.6.1 确认 `types.rs` 的旧 `load_extensions`/`LoadedExtension`/`ToolInfo` 是否还被 `resource_loader.rs` 依赖。
- [ ] 6.6.2 若依赖，迁移到新 JS 系统的元数据来源；若否，删除。
- [ ] 6.6.3 `cargo test` + clippy 确认无回归。

---

## Phase 7：验证与验收

### Task 7.1 — 黄金用例对齐
- [ ] 7.1.1 选 5 个典型原版示例扩展（`commands.ts` / `input-transform.ts` / `custom-footer.ts` / `project-trust.ts` / `qna.ts`），在 Rust 侧加载运行，逐个核对其事件触发与行为与原版一致。
- [ ] 7.1.2 记录对照表（行为场景 / TS 行为 / Rust 行为 / 一致性 / 差异原因）到 `docs/superpowers/specs/2026-07-13-extension-alignment-spec.md`。

### Task 7.2 — 事件序列对齐
- [ ] 7.2.1 给定同一 prompt + mock LLM 响应，记录 Rust 版产出的事件序列（event type 顺序）。
- [ ] 7.2.2 与原版序列逐项比对，差异列入 spec。

### Task 7.3 — 集成回归
- [ ] 7.3.1 `cargo test --workspace` 全过。
- [ ] 7.3.2 `cargo clippy --all-targets -- -D warnings` 零警告。
- [ ] 7.3.3 网络相关测试用录制 fixture（请求/响应 JSON）离线回放，不依赖真实 API key。

### Task 7.4 — 验收清单（每模块合并前打勾）
- [ ] 类型定义与分析文档一致，无遗漏字段
- [ ] 翻译测试用例 100% 通过
- [ ] 边界条件测试通过
- [ ] `cargo clippy --all-targets -- -D warnings` 无警告
- [ ] 无 `.unwrap()`/`.expect()`（测试除外）
- [ ] 公开 API 文档注释（`///`）覆盖所有 pub 项

---

## 优先级与依赖建议

- **先做 Phase 2（result-returning 接入）**：JS 侧已就绪，纯补 Rust 触发，ROI 最高。Task 2.1/2.2 不依赖 pi-agent-core 改动，可立即开工；2.3 需先评估 pi-agent-core。
- **Phase 3（fire-and-forget）** 与 Phase 2 可并行，互不依赖。
- **Phase 4（input + 命令消费）** 依赖 Phase 3 的 `session_*` 事件（命令的 `newSession`/`fork` 等会触发 session 事件）。
- **Phase 5（API 补齐）** 各子任务独立，可穿插在任何 Phase 之后按需做。
- **Phase 6（加载语义）** 可在任何时候做，但 6.1（目录名）越早定越好，影响所有测试 fixture 路径。
- **Phase 7（验证）** 贯穿全程，每个 Task 完成后即做对应的 7.1/7.2 子项。

## 待维护者决策的开放问题

1. **G22 目录名**：`.pi-rs/extensions/` vs 原版 `.pi/extensions/` —— 是否有意偏离？
2. **G2 message_end**：是否允许动 `pi-agent-core` 暴露"消息结束时可改写"钩子？（原进度文档明确延后过，需重新确认）
3. **G11 tool_execution_update**：高频流式事件，是否接入？（当前有意跳过）
4. **G12 对话框/widget/主题**：TUI 渲染层是否在本计划范围，还是全部留给 pi-tui 后续任务？本计划默认只做 RPC/JSON 语义通道。
5. **热重载（6.3）**：是否纳入本计划，还是作为后续独立任务？