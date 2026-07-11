# 进度：用 deno_core 嵌入运行时替换 Bun sidecar 扩展系统

**最后更新**: 2026-07-11
**计划文件**: `/Users/wxx/.claude/plans/nested-bouncing-tulip.md`
**当前阶段**: Task A — 已完成。pi-coding-agent 在新的嵌入式 deno_core 运行时下编译通过（仅剩 pre-existing edit.rs 错误）。

---

## 总体目标

把 pi-coding-agent 的 extension 系统从 Bun 子进程 sidecar（`rpc-host/` + `extensions/rpc.rs`）换成同进程 `deno_core` (V8) 嵌入运行时，使原版 pi (TypeScript) 扩展能高保真运行（同步 result-returning 事件、共享状态）。

**Phase 1 范围（已确认）**: 核心 + 事件先上，其余 `ExtensionApi` 方法桩化。`on_message_end` 的 pi-agent-core 小手术 **延后**（本任务不做了）—— `message_end` 事件暂不派发到扩展，可接受。

---

## 已完成（在磁盘上，未提交）

### 新文件（全部 `??` 未跟踪）
- `crates/pi-coding-agent/src/core/extensions/runtime.rs` — `ExtensionRuntime`（Clone handle）+ 专线程 current_thread tokio + `JsRuntime` + `RuntimeCommand` 通道 + `handle_load`/`handle_call_tool`/`handle_dispatch`/`read_json_value`/`path_to_specifier` + `create_extension_agent_tools`。
- `crates/pi-coding-agent/src/core/extensions/ops.rs` — `#[op2]` ops（注册类 + exec/notify/log）+ `extension!(pi_extension, ...)` 宏 + serde 类型（`ToolInfoSerde` 等）+ `PiOpState`。
- `crates/pi-coding-agent/src/core/extensions/loader.rs` — `TsModuleLoader` impl `ModuleLoader`（deno_ast 转 TS）+ `discover_extensions`（从 rpc-host/index.ts::discoverExtensions 移植）。
- `crates/pi-coding-agent/src/core/extensions/dispatcher.rs` — event payload 构造 + 结果解析（`tool_call_payload`/`dispatch_tool_call`、`tool_result_payload`/`dispatch_tool_result`、`fire_and_forget_from_agent_event`）。
- `crates/pi-coding-agent/src/core/extensions/runtime.js` — JS shim（`globalThis.pi` + 注册 Map + `__piDispatch`/`__piDispatchResult`/`__piCallTool`/`__piGetToolInfos`/`__piGetCommands`/`__piClearRegistries`/`__piLoadExtension`）。
- `crates/pi-coding-agent/tests/extensions_test.rs` — 未跟踪（早期 phase 产物，待 Task A 后评估）。

### 已修改（`M` 已跟踪）
- `Cargo.toml`（workspace root）— pi-tui 暂移出 `members`（解耦 unicode-width 冲突）。
- `Cargo.lock` — 加 deno_core/ast/error。
- `crates/pi-coding-agent/Cargo.toml` — 移除 pi-tui/crossterm（移到空 `interactive` feature 后面），加 `deno_core=0.404`/`deno_ast=0.53`（transpiling）/`deno_error=0.7`。
- `crates/pi-coding-agent/src/cli/run.rs` — interactive 模式 gated 在 `#[cfg(feature = "interactive")]` 后，无 feature 时返回 EXIT_FAILURE + 提示。
- `crates/pi-coding-agent/src/modes/mod.rs` — `pub mod interactive` 同样 gated。
- `crates/pi-coding-agent/src/core/extensions/mod.rs` — **当前状态有问题**：`pub mod rpc;` 仍在 + `pub use rpc::{create_extension_agent_tools as create_rpc_extension_agent_tools, ExtensionsRpcClient, ToolInfo as RpcToolInfo};`（破坏了 agent_session.rs，需改成新模块）。

### 待删除
- `crates/pi-coding-agent/src/core/extensions/rpc.rs`
- 整个 `rpc-host/` 目录（repo 根）

---

## 当前编译状态：0 errors（`cargo build -p pi-coding-agent`）

仅剩 pre-existing `edit.rs` 错误（2 errors，与本次迁移无关）。

### 已修复的问题

**loader.rs:**
- `ModuleLoaderError` 改用 `deno_core::error::ModuleLoaderError`（type alias for `JsErrorBox`）
- `ModuleSource::new` 修正为 4 参数签名：`(ModuleType, ModuleSourceCode, &ModuleSpecifier, Option<SourceCodeCacheInfo>)`
- `ModuleLoaderError::from(String)` 改用 `JsErrorBox::generic(String)`
- `resolve_import` 返回 `ModuleResolutionError`，用 `JsErrorBox::generic(e.to_string())` 转换

**ops.rs:**
- 加 `use deno_core::OpState;`
- `op_pi_register_command` 的 `name: String` 加 `#[string]`
- `op_pi_notify` 的 `r#type: Option<String>` 改 `#[string]`
- `op_pi_log` 改 `#[op2(fast)]`
- `op_pi_exec` 加 `.await`，去掉 `state: &mut OpState`（async op 不需要）

**runtime.rs:**
- `run_script` → `execute_script`
- `handle_scope` → `deno_core::scope!` macro + `v8::Local::new` + `serde_v8::from_v8`
- `JsRuntime::new` 去掉 `.expect()`（0.404 不返回 Result）
- 删掉 `content_block_marker` hack module
- `execute_script` 传 `String`（`&String` 不满足 `IntoModuleCodeString`）
- `JsError` 用 `deno_core::error::JsError`

**mod.rs:**
- 删 `pub mod rpc;` 和 rpc re-export
- 加 `ExtensionRuntime`/`ToolInfoSerde`/`CommandInfoSerde`/`LoadResult` re-export

**agent_session.rs:**
- `rpc_client: Option<Arc<ExtensionsRpcClient>>` → `extension_runtime: Option<Arc<ExtensionRuntime>>`
- `extension_tools: Vec<ToolInfo>` → `Vec<ToolInfoSerde>`

**sdk.rs:**
- `ExtensionsRpcClient::new()` + `start()` + `load_extensions()` → `ExtensionRuntime::new()` + `load()`

**已删除:**
- `extensions/rpc.rs`
- 整个 `rpc-host/` 目录

---

## 已确认事实

- **版本**: deno_core 0.404.0 / deno_ast 0.53.3 / deno_error 0.7.1（已在 Cargo.toml + Cargo.lock）。
- **deno_core 源码**: `/Users/wxx/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/deno_core-0.404.0/`
- **`exec_command` 是 async**: `pub async fn exec_command(command, args, cwd, options: Option<ExecOptions>) -> ExecResult`（直接返回 `ExecResult`，不是 `Result`）。`op_pi_exec` 必须 `.await`。
- **pi-tui 已解耦**: 移出 workspace `members`，pi-tui/crossterm 移到空 `interactive` feature 后。不要碰 pi-tui。
- **类型**: `AgentMessage` 派生 Serialize+Deserialize。`BeforeToolCallResult { block: bool, reason: Option<String> }`（无 args 字段，Phase 1 只支持 block）。`AfterToolCallResult { content, details, is_error, terminate }`。`ExecOptions { signal, timeout: Option<Duration>, cwd }`（无 env 字段）。`ExecResult { stdout, stderr, code: i32, killed }`。
- **deno_core 0.404 已知 API**（待 subagent 源码核实）: `#[op2]` on async fn（不是 `#[op2(async)]`）；`#[op2(fast)]` 给 fast-compatible ops；`extension!` 宏 `esm_entry_point` 要在 `esm` 之前；`ModuleResolveResponse = Result<ModuleSpecifier, ModuleLoaderError=JsErrorBox>`；`resolve_import`/`resolve_path` 在 root 导出。

---

## 接线已完成（Task A）

1. ✅ **mod.rs**: 已替换 rpc 模块为新模块 + re-export
2. ✅ **DELETE** `extensions/rpc.rs` — 已删除
3. ✅ **DELETE** 整个 `rpc-host/` — 已删除
4. ✅ **agent_session.rs**: 已替换字段和 import
5. ✅ **sdk.rs**: 已替换 ExtensionsRpcClient 为 ExtensionRuntime
6. ✅ **`ExtensionRuntime::new()`** 保持无参

### 未完成（Phase 2+）
- `before_tool_call`/`after_tool_call` hook（用 dispatcher）— 暂不接（dispatcher.rs 已写好但未接线进 AgentSession 事件订阅）
- `on_message_end` 事件派发 — 明确延后
- `types.rs` 旧扩展系统（~560 行）死代码清理 — 旧 `load_extensions`/`ToolInfo`/`LoadedExtension` 被 resource_loader.rs 调用但结果被丢弃，与新 JS 系统重复

---

## Review-driven 修复（2026-07-11，code-review 后）

针对 `/code-review` 发现的 10 个问题，已修复 9 个：

1. ✅ **通知丢失**（runtime.js + ops.rs）：`ctx.ui.notify` 现在同时推入 JS `pendingNotifications` 数组（`__piCallTool` 返回它）和 Rust OpState 镜像。
2. ✅ **pi.exec cwd 默认值**（runtime.js）：新增 `execWithDefaultCwd` + 模块级 `sessionCwd`；`ctx.exec` 默认 `ctx.cwd`，`pi.exec` 默认 `sessionCwd`（Rust 在 `handle_load` 调 `__piSetCwd`）。
3. ✅ **无超时挂起**（runtime.rs）：新增 `COMMAND_TIMEOUT=120s` + `await_reply` 包装所有 oneshot await；新增 `ExtensionError::Timeout`。
4. ✅ **op_pi_exec 无超时**（ops.rs）：默认 30s 超时（匹配旧 sidecar），防止 V8 线程被子进程永久占住。
5. ✅ **Drop 死锁**（runtime.rs）：Drop 不再 `handle.join()`（阻塞），改为 detach — 通道关闭后线程自行退出。
6. ✅ **dispose() 空操作**（agent_session.rs）：`AgentSession` 新增 `extension_runtime` 字段，`dispose()` 调 `rt.stop()`。
7. ✅ **prompt_guidelines 丢弃**（sdk.rs）：聚合所有扩展工具的 `prompt_guidelines` 传入 `AgentSessionConfig.prompt_guidelines`。
8. ✅ **启动 run_event_loop 吞错**（runtime.rs）：初始化失败时 `eprintln!` 而非 `let _` 静默。
9. ✅ **eager V8 spawn + .expect panic**（sdk.rs + runtime.rs）：先 `discover_extensions`，空则不 spawn；`new()` 改为 `Result<Self, ExtensionError>`，失败降级而非 panic。

### 仍待修复
- ~~`dispatcher.rs` 未接线进 AgentSession~~ — ✅ 已接线（见下方）
- `types.rs` 旧扩展系统死代码清理 — Phase 2

---

## dispatcher.rs 接线 + edit.rs 修复（2026-07-11，第二轮）

### edit.rs pre-existing 错误修复
- `crates/pi-coding-agent/src/core/tools/edit.rs:401` — `String + &String` 不合法 → 改用 `format!("{}{}", bom, restored)`。整个 crate 现在 0 编译错误，535 测试全过。

### dispatcher.rs 接线进 AgentSession
1. ✅ **before_tool_call hook**：`AgentSession::new` 构造 `BeforeToolCallFn`，调 `dispatcher::dispatch_tool_call(&rt, &ctx)`，传入 `AgentOptions.before_tool_call`。agent_loop 在每次工具执行前 await 它，扩展可 block。
2. ✅ **after_tool_call hook**：构造 `AfterToolCallFn`，调 `dispatcher::dispatch_tool_result(&rt, &ctx)`，传入 `AgentOptions.after_tool_call`。agent_loop 在工具结果后 await 它，扩展可改 content/details/is_error。
3. ✅ **fire-and-forget agent 事件**：新增 subscribe listener，用 `fire_and_forget_from_agent_event` 映射 AgentEvent → 事件名+payload，`tokio::spawn` detached 派发（不阻塞 agent 事件循环）。
4. ✅ **cwd 注入**：fire-and-forget listener 把 `session.cwd` 注入每个 payload，避免 `__piDispatch` 拿到 `ctx.cwd = "/"`。
5. ✅ **fail-open 不再静默**：`dispatch_tool_call`/`dispatch_tool_result` 的 runtime 错误改为 `eprintln!` 记录，不再 `let _ = .ok()?` 静默吞掉。

### 验证
- `cargo build -p pi-coding-agent` → 0 errors（edit.rs 已修）
- `cargo test -p pi-coding-agent` → 535 passed, 6 ignored, 0 failed
- `cargo build -p pi-agent-core` → clean
- 变更文件无新增 warning（仅剩 pre-existing `modes/rpc/mod.rs:9` unused import）

---

## 验证标准（Task A 完成状态）

- ✅ `cargo build -p pi-coding-agent` → 0 errors（仅剩 pre-existing edit.rs 错误）
- ⏳ `cargo test -p pi-coding-agent` — 因 pre-existing edit.rs 错误无法编译测试二进制
- ✅ `cargo build -p pi-agent-core` — 仍 clean
- ✅ **不**跑新 `embedded_runtime_test`（后续任务）

---

## 约束

- 只动 `pi-coding-agent`（+ 删 `rpc-host/`）。不碰 `pi-tui`/`pi-ai`/`pi-agent-core`（`on_message_end` 明确延后）。
- 每个 deno_core API 签名都查 0.404 源码确认，不靠记忆。
- 提交时 `git status` 检查，只 stage 相关文件，不提交 scratch 文件（`fibonacci*`/`pi-rust-port-spec.md` 若存在）。

---

## 下一步

1. 修复 pre-existing `edit.rs` 编译错误（`String + &String` 和 `str` size 问题）
2. 运行测试确认现有测试通过
3. Phase 2: 接 `before_tool_call`/`after_tool_call` hook（dispatcher）
4. Phase 2: `AgentSession.dispose()` 调 `stop()`
5. Phase 3: `on_message_end` 事件派发

---

## 旧的 SDD ledger（stale，指 sidecar 那一轮，已完成）

`.superpowers/sdd/progress.md` 里 Task 1-13 是旧的 Bun sidecar 扩展系统那轮 SDD 的记录，全部 complete。那一轮的产物（rpc.rs + rpc-host/）现在正被本任务替换。ledger 需要在 Task A 完成后追加新条目。