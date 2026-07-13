# pi-coding-agent Extension 对齐方案 — 实现情况审查报告

> 审查对象：`docs/superpowers/plans/2026-07-13-extension-alignment-plan.md` 的实现情况
> 审查范围：commits `8dccf7d..HEAD`（deno_core 迁移 + Phase 2–7 全部）
> 审查方法：8 个独立视角（逐行 / 删除行为 / 跨文件 / 复用 / 简化 / 效率 / altitude / CLAUDE.md 规约）→ 1 票验证（召回优先）
> 审查日期：2026-07-13

---

## 总体结论

Phase 2–7 的 commit message 几乎逐项标记"已完成"，但**大量"已实现"只是壳子**：

- dispatch 函数定义了却没有调用方；
- `HostCommand` 桥接通向一个返回 `Ok({})` 的 stub，从不调用真实 session 方法；
- JS 侧聚合逻辑就绪，但 Rust 侧要么从不触发对应事件，要么**丢弃返回值**。

即：原版 pi 扩展加载后，大多数事件 / 能力对宿主**无实际效果**，但代码与 commit 历史看起来已完成。建议合并前对每个 G 项做**端到端触发验证**（加载真实扩展 → 断言宿主侧可观察到效果），而非依赖 dispatch 函数存在性或单元测试通过来判断完成度。

---

## 严重问题（10 项，按严重度排序）

### 1. `process_host_commands` 全 stub —— 整个 `pi` API 动作面静默 no-op

- **位置**：`crates/pi-coding-agent/src/core/agent_session.rs:541`
- **现象**：`process_host_commands` 的 match 把 `set_model | send_message | send_user_message | set_session_name | set_label | set_thinking_level | register_provider | unregister_provider | new_session | fork | switch_session | reload` 全部 stub 成 `Ok(serde_json::json!({}))`，**不调用任何真实 session 方法**，`_args` 丢弃。
- **附带问题**：
  - `op_pi_append_entry`（`ops.rs:258`）推入的 `"append_entry"` 不在 match 臂里 → 返回 `Err`，而 `process_host_commands` 用 `let _ = cmd.reply.send(result)` 丢弃，op 端 `_rx` 也已立即 drop → `pi.appendEntry(...)` 在 JS 侧看似成功，实际什么都没做。
  - `process_host_commands` 仅在 `prompt()` 内调用一次（全 crate 唯一调用点），**turn 中**发出的 host command 要排队到下一次 `prompt()` 才被处理 → 当前 turn 完全错过。
- **失败场景**：扩展调用 `pi.setModel('claude-opus')` / `pi.sendMessage(...)` / `ctx.newSession()` / `ctx.reload()` → op 立即向 JS 返回 `Ok(())`，HostCommand 在下一次 prompt 才被处理，处理时只回 `Ok({})` 不改任何状态。模型不切换、消息不注入、会话不创建。
- **影响范围**：Phase 5.3 / 5.4 / 5.5 / 5.6、Task 4.3 实际未生效。
- **修复方向**：把 match 各臂路由到既有方法（`AgentSession::set_model` / `send_custom_message` / `send_user_message` / `set_session_name`、`SessionManager::fork/new/switch`、`ExtensionRuntime::reload`）；闭包是 sync，需 `tokio::spawn` 进异步方法或改用 async 通道；补 `append_entry` 臂；考虑在 turn 中也 drain host command。

### 2. `before_provider_request`（G3 / Task 2.2）假实现

- **位置**：`crates/pi-coding-agent/src/core/agent_session.rs:213`
- **现象**：`on_payload` 闭包 `tokio::spawn` 后 `let _ =` **丢弃返回的（可能被改写的）payload**；且 pi-agent-core 全程**从不调用 `on_payload`**（grep 无调用点），闭包签名 `Fn(Value) -> ()` 也结构上无法回传结果。
- **失败场景**：扩展注册 `before_provider_request` 想改写出站请求体（注入 header / 换 model / 脱敏）→ handler 永不触发；即便触发返回值也被丢弃，原始 payload 照发。
- **影响**：G3 未达成，但 `dispatcher.rs:156` 的 `dispatch_before_provider_request` 让它看起来已接入。
- **修复方向**：要么让 `on_payload` 真正 async 并 await+应用返回 payload 后再发 HTTP（需 pi-agent-core 暴露钩子，见 plan 待决策项 #2），要么显式标记 deferred（像 `message_end` 那样），不要假装完成。

### 3. 扩展 slash 命令从未接入（G16 / G23）

- **位置**：`crates/pi-coding-agent/src/core/slash_commands.rs:131`、`crates/pi-coding-agent/src/core/extensions/ops.rs:202`
- **现象**：
  - `resolve_extension_commands`（含 G23 命令名冲突 `cmd:1`/`cmd:2` 解析）定义且有单测，但**全 crate 零调用方**。
  - `sdk.rs` 调 `rt.load(...)` 后只消费 `result.tools`，`result.commands`（`LoadResult.commands`，由 `__piGetCommands` 填充）被静默丢弃。
  - `op_pi_get_commands`（`ops.rs:202`）仍返回空 `Vec`，注释自称 "JS owns the command registry; return empty here"。
- **失败场景**：扩展 `pi.registerCommand({name:'summarize',...})` → 命令元数据存进 JS、甚至回读到 `LoadResult.commands`，但无代码并入 slash 补全表，也无路径把 `/summarize` 路由到 JS handler。用户输入 `/summarize` 被当普通 prompt。
- **影响**：Phase 4.2 仅靠 `resolve_extension_commands` 单测标记完成，端到端路径不存在。

### 4. 6 个新 dispatch 函数零调用方 —— 事件表面存在但永不触发

- **位置**：`crates/pi-coding-agent/src/core/extensions/dispatcher.rs`
- **现象**：以下函数定义齐全、JS 侧聚合分支就绪，但**全 crate 无调用方**：
  - `dispatch_user_bash`（:188，G10）
  - `dispatch_resources_discover`（:269，G7）
  - `dispatch_project_trust`（:314，G5）
  - `dispatch_after_provider_response`（:347，G8）
  - `dispatch_thinking_level_select`（:389，G9）
  - `dispatch_session_info_changed`（:242，G6 子项）
- `session_before_switch` / `session_before_fork` / `session_before_compact` / `session_compact` **连 dispatch 函数都没有**；`session_manager.rs` 与 `compaction.rs` 中 0 个 `dispatch_*` 引用。
- **失败场景**：扩展实现 `project_trust` 自动信任、`resources_discover` 贡献 skill 路径、`user_bash` 沙箱化 `!ls` → 均永不触发。内置信任流独自跑、skill 路径不合并、`!ls` 绕过扩展。
- **影响**：G5 / G7 / G8 / G9 / G10 及 G6 大半未接入，但 Phase 3 各 task 已标记完成。

### 5. `tool_result` 聚合传陈旧 payload，多扩展串联断裂

- **位置**：`crates/pi-coding-agent/src/core/extensions/runtime.js:243`
- **现象**：`tool_result` 分支 `const r = await h(payload, ctx)` 把**原始 `payload`** 传给每个 handler，而非累计的 `cur`。同函数其他聚合分支都正确传 `cur`：`message_end`(:260) `{...payload, message:cur}`、`context`(:275) `{messages:cur}`、`before_provider_request`(:288) `{payload:cur}`、`input`(:344) 传累计 `curText`/`curImages`。
- **失败场景**：扩展 A 把 `tool_result.content` 改成 `'[redacted]'`，扩展 B 逻辑 `if(payload.content.includes('secret'))` 仍收到原始 content（含 secret）→ A 的修改被覆盖 / B 误判。
- **修复**：`const r = await h({...payload, content: cur.content, details: cur.details, isError: cur.isError}, ctx)`。

### 6. `dispatch_input` 丢弃 images —— input 事件图片能力未实现

- **位置**：`crates/pi-coding-agent/src/core/extensions/dispatcher.rs:425`
- **现象**：
  - `dispatch_input(runtime, text, source)` **无 images 参数**，payload 不含 `images` 键。
  - `InputEventResult::Continue { text }` 只带 text；结果解析只读 `text`，丢弃 JS 回传的 `images`。
  - 调用点 `agent_session.rs:587-604` 只构造 `ContentBlock::text(&effective_text)`，不加图片块。
  - 而 `runtime.js:339-356` 正确把 `curImages` 串联并回传 `{action, text, images}`。
- **失败场景**：扩展 input handler 返回 `{action:'transform', text, images:[...]}` 附图 → 图片在 Rust 侧被静默丢弃，永不达 LLM。
- **影响**：Plan Task 4.1.2 / 4.1.4（传递并尊重 images）未实现。

### 7. `mode`/`hasUI` 硬编码 `rpc`/`false`（G20 / Task 6.5 未达成）

- **位置**：`crates/pi-coding-agent/src/core/extensions/runtime.rs:464`
- **现象**：`handle_load` 硬编码 `let set_mode = "globalThis.__piSetContextMode(\"rpc\", false)"`，每次 load/reload 都发这个常量。`ExtensionRuntime::new` / `handle_load` 都不接收 mode 参数。注释声称 "mode 由宿主传入"，代码却写死；`runtime.js` 的 `__piSetContextMode` 钩子被接到一个常量上。
- **失败场景**：pi-rs 以 print / json / tui 模式运行时，扩展仍观察 `ctx.mode==='rpc' && ctx.hasUI===false`。按 mode 分支的扩展走错分支。

### 8. `ExtensionErrorEvent` 通道出厂即死（G21 / Task 6.4）

- **位置**：`crates/pi-coding-agent/src/core/extensions/runtime.rs:158`
- **现象**：
  - `new()` 中 `let (error_tx, _error_rx) = mpsc::unbounded_channel()`，`_error_rx` 立即 drop → `error_tx` 是无接收方的 sender，全仓无 `error_tx.send`。
  - `on_error()`（:176）新建一个 `(new_tx, rx)`，`forward_tx` 被 `let _ = forward_tx;` 丢弃 → 返回的 receiver 未连接到 `error_tx`，永不 yield。
  - `on_error()` 全仓零调用方。
  - JS handler 异常被 `runtime.js` catch 后只走 `op_pi_log`（stderr 行）。
- **失败场景**：宿主调 `rt.on_error()` 想收集结构化扩展错误做诊断 → receiver 永不 yield；坏扩展只表现为零星 stderr，API 假装可观测。
- **修复方向**：要么删掉 `error_tx`/`on_error` 直到 JS 异常路径真正喂通道（加 `op_pi_emit_error`），要么接通 fan-out 多路复用。

### 9. `op_pi_exec` 超时未向 `COMMAND_TIMEOUT` 钳制

- **位置**：`crates/pi-coding-agent/src/core/extensions/ops.rs:593`、`runtime.rs:43`
- **现象**：`COMMAND_TIMEOUT = 120s`（`runtime.rs:43`），在 `await_reply`（:48）应用于主线程等待 V8 回复。`op_pi_exec` 取 `options.timeout` 直接 `Duration::from_secs`，**无 `min`/钳制**；缺失时回退 30s。V8 命令循环严格串行（`runtime.rs:351`）。
- **失败场景**：扩展调 `pi.exec('make', [], {timeout:180})` 跑 150s 构建 → 主线程 `call_tool` 在 120s 返回 `ExtensionError::Timeout`（工具报失败），但 V8 线程仍被占 60s；期间排队的 `agent_end`/`turn_end`/`tool_call` 等 `CallTool`/`DispatchEvent` 全部堵在 mpsc 队列后面，陆续也过 120s 超时。`Drop` 注释（:322-325）已承认此场景。
- **修复**：`op_pi_exec` 的 timeout 应钳制到 `COMMAND_TIMEOUT`（或更小）。

### 10. `reloadable` 字段死写 + reload 机器无入口（Task 6.2 / 6.3）

- **位置**：`crates/pi-coding-agent/src/core/extensions/loader.rs:162`、`runtime.rs:258`
- **现象**：
  - `DiscoveredExtension.reloadable` 在 `discover_extensions` 中被写（project/global=true、`-e` 显式路径=false），但**全仓 grep 0 读**。
  - `ExtensionRuntime::reload`（:258）与 `Reload` 命令 handler 把 `paths` 原样透传 `handle_load`，**不过滤 `reloadable`** → `-e` 不可重载项会被重载，违反 Task 6.2.2。
  - reload **无任何触发路径**：
    - `FsWatcher`（`utils/fs_watch.rs`）存在但全仓仅自身定义与单测引用，**未接扩展目录**；
    - `/reload` 未注册为 slash 命令（`interactive.rs` 走 catch-all 当普通消息）；
    - `op_pi_reload` 推入的 `"reload"` 被 `process_host_commands` stub 成 `Ok({})`。
- **失败场景**：一旦 `reload()` 被任何调用方触发，`-e` 扩展会被重载（违反原版冻结语义）；而当前 reload 机器**整体不可达**——改扩展文件磁盘无反应，`/reload` 当普通消息，`pi.reload()` 进 stub。Phase 6.3 热重载 commit 标记完成但实为死代码。

---

## CLAUDE.md 规约违反（事实确凿，可直接定位）

> 规则原文（repo-root CLAUDE.md）：「禁止 `.unwrap()` / `.expect()`（测试代码除外），错误必须通过 `Result<T, E>` 显式传播」「禁止静默 fallback：… Rust 侧要显式判断并返回 Err 或 panic（不能默默吞掉）」；plan 全局约束：「宿主接入点 fail-open 时必须 `eprintln!` 记录，不静默吞」。

| 位置 | 违反点 | 规则 |
|------|--------|------|
| `agent_session.rs:534` | `self.session_manager.lock().unwrap().refresh_config().await` | 禁 `.unwrap()`（非测试）；mutex 中毒会 panic 打断 session loop |
| `agent_session.rs:603` | `self.session_manager.lock().unwrap().set_run_prompt(&effective_text)` | 同上（本次 input 接入新增行） |
| `runtime.rs:347` | `runtime_thread_main` 中 `.expect("failed to build extension runtime tokio runtime")` | 禁 `.expect()`；应回 `ExtensionError` |
| `runtime.rs:451/460/465` | `let _ = js.execute_script(...)`（pi-clear / pi-set-cwd / pi-set-mode） | fail-open 必须 `eprintln!`；该文件自身注释（:351-354）要求 surface，否则后续 `__pi*` 报 cryptic ReferenceError |
| `runtime.rs:458` | `serde_json::to_string(cwd).unwrap_or_else(|_| "\"/\"".into())` | 禁静默 fallback；cwd 序列化失败被默默替换成 `/` |
| `agent_session.rs:541` | 12 个 host function 全 `Ok({})` 不调用方法也不 log | 禁静默 fallback + fail-open 必须 log（与严重问题 #1 同点） |

---

## 清理类（altitude / 复用 / 简化 / 效率）

- **13 个 op 复制粘贴**（`ops.rs:213+`）：`op_pi_send_message` / `send_user_message` / `append_entry` / `set_session_name` / `set_label` / `set_model` / `set_thinking_level` / `register_provider` / `unregister_provider` / `new_session` / `fork` / `switch_session` / `reload` 是同一 ~15 行 `borrow_mut<PiOpState> → lock → push HostCommand` 块的逐字复制。应抽 `fn push_host_command(state: &mut OpState, function: &str, args: Value)`，每个 op 收缩到 2-3 行。
- **`HostCommand.reply` 结构性死重**：每个 op `let (reply, _rx) = oneshot::channel()` 后 `_rx` 立即 drop，`process_host_commands` 的 `cmd.reply.send` 恒落空通道，无 op 等待回复。应删 `reply` 字段，或拆 `FireAndForget` 变体。
- **两套并存的 V8→main 机制**：`RuntimeCommand::CallHost` + `pub async call_host()` + 线程 handler arm（恒返回 "not yet wired"）是死代码（`call_host` 零调用方），与 `HostCommand` Vec 重复。应删 `CallHost` 路径。
- **`dispatcher.rs` 重复样板**：10 个 `dispatch_*` 各自重复 fail-open 块 + `.get("x").and_then(...).unwrap_or_default()` JSON 提取。应抽 `fail_open()` / `get_array<T>()` 助手。
- **`poll_host_command` O(n²)**（`runtime.rs:298`）：用 `guard.remove(0)`（O(n) 移位）排空共享 Vec，n 条命令排空为 O(n²) 且持锁。应换 `VecDeque::pop_front` 或 `std::mem::take` 整批取出后释放锁。
- **热路径深克隆**：`dispatch_context`（`dispatcher.rs:87`）与 `fire_and_forget_from_agent_event`（:252）在每 turn 对整段消息历史做 serde 深克隆并跨 V8 往返，**即便无扩展注册对应 handler 也照付**。应在加载时缓存 handler 存在性位图（reload 时刷新），无 listener 时整段跳过 round-trip。

---

## 建议的合并前动作

1. **修或回退** 严重问题 #1（`process_host_commands` stub）与 #2（`before_provider_request` 丢弃返回值）——这两处直接让"已标记完成"的 Phase 5 / G3 名不副实。
2. **端到端验证清单**：对每个 G 项加载一个最小真实扩展，断言宿主侧可观察到效果（而非依赖 dispatch 函数存在性 / 单测通过）。优先验证 G1/G2/G3/G4/G5/G6/G7/G10/G16/G18。
3. **修规约违反**：6 处 `.unwrap()`/`.expect()`/静默 fallback 改为 `Result` 传播或 `eprintln!`。
4. **钳制 `op_pi_exec` 超时** 到 `COMMAND_TIMEOUT`。
5. **接通或回退 reload**：要么接 `FsWatcher` + 注册 `/reload` + 路由到 `ExtensionRuntime::reload` 并按 `reloadable` 过滤，要么把 Task 6.2/6.3 显式标 deferred。
6. **补 `tool_result` 串联**（`runtime.js:243` 传 `cur`）与 `dispatch_input` images 透传。