# CONTRACT_ALIGNMENT.md — pi-coding-agent RPC Mode

This file documents the alignment between the TypeScript original
(`packages/coding-agent/src/modes/rpc/rpc-mode.ts`) and the Rust port
(`crates/pi-coding-agent/src/modes/rpc/`) for each public RPC API.

## Conventions

- **"是否一致"** values:
  - `是` — behavior is identical
  - `是（有意偏差，见 DEVIATIONS.md #N）` — behavior differs intentionally
  - `否` — behavior differs unintentionally (bug, must fix)

## RPC Commands

### Prompt

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 发送 prompt | `session.prompt(message, { images, streamingBehavior, source: "rpc", preflightResult })` | `session.prompt(&message, Some(options))` with `AgentEnd` listener | 是（有意偏差，见 DEVIATIONS.md #1） |
| 成功响应 | Emitted via `preflightResult(true)` → `success(id, "prompt")` | Emitted on `AgentEnd` event → `rpc_success(id, "prompt", None)` | 是（有意偏差，见 DEVIATIONS.md #1） |
| 错误响应 | `.catch()` → `error(id, "prompt", e.message)` | After `prompt()` returns, check `AgentEnd` flag → `rpc_error(id, "prompt", msg)` if agent run never started | 是（有意偏差，见 DEVIATIONS.md #1） |
| 图片参数 | Passed as `images` to `session.prompt()` | Passed as `images` through `PromptOptions` | 是（有意偏差，见 DEVIATIONS.md #7） |
| streamingBehavior | Passed as `streamingBehavior` to `session.prompt()` | Passed as `streaming_behavior` through `PromptOptions` | 是（有意偏差，见 DEVIATIONS.md #7） |
| 重复响应防护 | N/A (single Promise) | Listener unsubscribes after first `AgentEnd` | 是 |

### Steer

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 发送 steer | `session.steer(message, images)` | `session.steer(message, images)` | 是 |
| 响应 | `success(id, "steer")` | `rpc_success(id, "steer", None)` | 是 |

### NewSession

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 创建新会话 | `session.newSession()` + `rebindSession()` | `session.new_session()` | 是（有意偏差，见 DEVIATIONS.md #2） |
| 响应 | `success(id, "new_session", { cancelled: false })` | `rpc_success(id, "new_session", { cancelled: false })` | 是 |

### GetState

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| model 字段 | Full `Model<any>` object | Full `Model` object (via `pi_agent_core::pi_ai_types::Model`) | 是 |
| thinkingLevel | `session.thinkingLevel` | `session.get_thinking_level().await` | 是 |
| isStreaming | `session.isStreaming` | `session.is_streaming().await` | 是 |
| isCompacting | `session.isCompacting` | `session.is_compacting()` | 是 |
| steeringMode | `session.steeringMode` | `session.steering_mode().await` → "all" / "one-at-a-time" | 是 |
| followUpMode | `session.followUpMode` | `session.follow_up_mode().await` → "all" / "one-at-a-time" | 是 |
| sessionFile | `session.sessionFile` | `session.get_session_file()` | 是 |
| sessionId | `session.sessionId` | `session.get_session_id()` | 是 |
| sessionName | `session.sessionName` | `session.get_session_name()` | 是 |
| autoCompactionEnabled | `session.autoCompactionEnabled` | `session.auto_compaction_enabled()` | 是 |
| messageCount | `session.messages.length` | `session.get_messages().await.len()` | 是 |
| pendingMessageCount | `session.pendingMessageCount` | `session.pending_message_count()` | 是 |

### SetModel

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 设置模型 | `session.setModel(provider, modelId)` | `session.set_model(provider, model_id)` | 是 |
| 响应 | `success(id, "set_model", model)` | `rpc_success(id, "set_model", model)` | 是 |

### CycleModel

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 循环模型 | `session.cycleModel()` | `session.cycle_model().await` | 是 |
| 响应 | `{ model, thinkingLevel, isScoped } \| null` | `{ model, thinkingLevel, isScoped } \| null` | 是 |

### CycleThinkingLevel

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 循环思考级别 | `session.cycleThinkingLevel()` | `session.cycle_thinking_level().await` | 是 |
| 响应 | `{ level } \| null` | `{ level } \| null` | 是 |

### SetAutoRetry

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 设置自动重试 | `session.setAutoRetryEnabled(enabled)` | `session.set_auto_retry_enabled(enabled)` | 是 |
| 响应 | `success(id, "set_auto_retry")` | `rpc_success(id, "set_auto_retry", None)` | 是 |

### SetSteeringMode

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 设置转向模式 | `session.setSteeringMode(mode)` | `session.set_steering_mode(mode)` | 是 |
| 响应 | `success(id, "set_steering_mode")` | `rpc_success(id, "set_steering_mode", None)` | 是 |

### SetFollowUpMode

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 设置跟进模式 | `session.setFollowUpMode(mode)` | `session.set_follow_up_mode(mode)` | 是 |
| 响应 | `success(id, "set_follow_up_mode")` | `rpc_success(id, "set_follow_up_mode", None)` | 是 |

### SetSessionName

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 设置会话名称 | `session.setSessionName(name.trim())` with empty validation | `session.set_session_name(name.trim())` with empty validation | 是 |
| 空名称验证 | Returns error if name is empty after trim | Returns error if name is empty after trim | 是 |
| 响应 | `success(id, "set_session_name")` | `rpc_success(id, "set_session_name", None)` | 是 |

### GetEntries

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 获取所有条目 | `session.getEntries()` → full `SessionEntry[]` | `mgr.get_entries()` → full `SessionEntry[]` | 是 |
| `since` 过滤 | `findIndex` + `slice(sinceIndex + 1)` | `position` + `idx + 1..` | 是 |
| 无效 `since` ID | Returns error | Returns error | 是 |
| 响应格式 | `{ entries: SessionEntry[], leafId: string }` | `{ entries: SessionEntry[], leafId: string }` | 是 |

### GetTree

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 获取会话树 | `session.getTree()` | `session.get_session_manager().get_tree()` | 是 |
| 响应格式 | Tree structure with entries | Tree structure with entries | 是 |

### GetForkMessages

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 获取 fork 消息 | `session.getUserMessagesForForking()` | `session.get_user_messages_for_forking()` | 是 |
| 响应格式 | `{ entries: { entryId, text }[] }` | `{ entries: { entryId, text }[] }` | 是 |

### Fork

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| Fork 会话 | `session.forkSession(entryId)` | `session.fork_session(entry_id)` | 是 |
| 响应 | `{ text: string, cancelled: boolean }` | `{ text: "", cancelled: false }` | 是（有意偏差，见 DEVIATIONS.md #3） |

### SwitchSession

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 切换会话 | `session.switchSession(path)` + `rebindSession()` | `session.switch_session(path)` | 是（有意偏差，见 DEVIATIONS.md #4） |
| 响应 | `success(id, "switch_session")` | `rpc_success(id, "switch_session", None)` | 是 |

### Clone

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 克隆会话 | `session.cloneSession()` | `session.clone_session()` | 是 |
| 响应 | `{ cancelled: boolean }` | `{ cancelled: false }` | 是 |

### Abort

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 中止操作 | `session.abort()` | `session.abort()` | 是 |
| 响应 | `success(id, "abort")` | `rpc_success(id, "abort", None)` | 是 |

### AbortBash

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 中止 bash | `session.abortBash()` | `session.abort_bash()` | 是 |
| 响应 | `success(id, "abort_bash")` | `rpc_success(id, "abort_bash", None)` | 是 |

### AbortRetry

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 中止重试 | `session.abortRetry()` | `session.abort_retry()` | 是 |
| 响应 | `success(id, "abort_retry")` | `rpc_success(id, "abort_retry", None)` | 是 |

### Bash

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 执行 bash | `session.executeBash(command, { excludeFromContext })` | `session.execute_bash(command, exclude_from_context)` | 是（有意偏差，见 DEVIATIONS.md #8） |
| 响应 | `success(id, "bash", result)` | `rpc_success(id, "bash", result)` | 是 |

### Compact

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 执行压缩 | `session.compact(customInstructions)` | `session.compact(custom_instructions.as_deref())` | 是 |
| 成功响应 | `CompactionResult { summary, firstKeptEntryId, tokensBefore, estimatedTokensAfter?, details? }` | `CompactionResult { summary, first_kept_entry_id, tokens_before, estimated_tokens_after, details }` | 是 |
| 失败响应 | `session.compact()` throws → caught by outer try-catch → `error(id, "compact", e.message)` | `rpc_error(id, "compact", reason)` | 是 |

### SetAutoCompaction

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 设置自动压缩 | `session.setAutoCompactionEnabled(enabled)` | `session.set_auto_compaction_enabled(enabled)` | 是 |
| 响应 | `success(id, "set_auto_compaction")` | `rpc_success(id, "set_auto_compaction", None)` | 是 |

### GetCommands

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 获取命令列表 | Includes extension commands, prompt templates, skills | Includes extension commands (`resolve_extension_commands`), prompt templates, skills | 是 |
| 命令顺序 | Extension → prompt → skill | Extension → prompt → skill | 是 |
| 响应格式 | Array of command objects | Array of command objects | 是 |

### ExportHtml

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 导出 HTML | `session.exportToHtml(outputPath)` | `session.export_html_to_file(output_path)` | 是 |
| 响应 | `{ path: string }` | `{ path: string }` | 是 |

### ExtensionUIResponse

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 处理扩展 UI 响应 | Handles `extension_ui_response` type on stdin | Handles `extension_ui_response` type on stdin via `pending_extension_requests` | 是 |
| 扩展启用状态 | Extensions enabled in RPC mode | Extensions enabled in RPC mode (`enable_extensions: true`) | 是 |

## RPC Protocol

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 输入格式 | JSONL on stdin | JSONL on stdin | 是 |
| 输出格式 | JSONL on stdout | JSONL on stdout (single channel) | 是 |
| 信号处理 | SIGHUP → exit code 129, SIGTERM → exit code 143 | SIGHUP → exit code 129, SIGTERM → exit code 143 | 是 |
| 关闭防护 | `shuttingDown` guard | `shutting_down` AtomicBool guard | 是 |
| 事件流 | Agent events streamed as JSONL | Agent events streamed as JSONL | 是 |
| 解析错误 | Invalid JSON → `error(undefined, "parse", "Failed to parse command: ...")` | `rpc_error(None, "parse", "Failed to parse command: {e}")` | 是 |
| 未知命令 | Unknown command type in `handleCommand()` switch default → `error(command.id, command.type, "Unknown command: ...")` | Parse JSON as generic `Value` first to extract `id`/`type`; on `RpcCommand` deserialization failure → `rpc_error(cmd_id, cmd_type, "Unknown command: {cmd_type}")` | 是 |
| 扩展 UI 响应 | Checks `parsed.type === "extension_ui_response"` before parsing as `RpcCommand` | Checks `cmd_type == "extension_ui_response"` before parsing as `RpcCommand` | 是 |

## JS Extension Runtime (`#[cfg(feature = "js-runtime")]`)

Public APIs introduced by the V8-based extension loader. These are gated
behind the `js-runtime` feature and have no TS counterpart as Rust APIs
(the TS original uses a JS-native extension runner).

### JsExtensionManager

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| `spawn()` | N/A (JS runtime is in-process) | Spawns a dedicated std::thread with a current-thread tokio runtime + LocalSet for V8 (which is `!Send`) | 是（有意偏差，见 DEVIATIONS.md — extension system internal deviation） |
| `load_extension(path, cwd)` | `jiti.import(path, { default: true })` → `factory(api)` | `JsCommand::LoadExtension` → V8 thread executes shim that imports the default export and invokes `factory(globalThis.__pi)` | 是 |
| `bind_core(actions)` | `ExtensionRunner.bindCore()` — flushes pending providers, installs live action closures | `JsCommand::BindCore` → V8 thread calls `js_runtime.bind_core(actions)`; pending providers are flushed by the caller (sdk.rs) before sending BindCore | 是 |
| `shutdown()` | N/A | Sends `JsCommand::Shutdown`, drops real sender, joins thread | 是 |

### JsExtensionAdapter

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| `new(path, load_result, cmd_tx)` | N/A | Creates adapter from captured `ExtensionLoadResult` + V8 channel sender | 是 |
| `source_info()` | `extension.sourceInfo` | Returns per-extension `SourceInfo` derived from file path | 是 |
| `pending_providers()` | `runtime.pendingProviderRegistrations` | Returns `&[PendingProviderRegistration]` captured pre-bind | 是 |
| `register_tools()` | `registerTool(name, ...)` | `register_tools(&mut ToolRegistry)` via `HookHandler` | 是 |
| `register_commands()` | `registerCommand(name, ...)` | `register_commands(&mut CommandRegistry)` via `HookHandler` | 是 |
| tool execution | `execute(toolCallId, params, signal, onUpdate, ctx)` | `JsCommand::ExecuteTool` → V8 thread runs `execute_async_and_get_json` with 5-arg signature | 是 |
| event dispatch | `runtime.emit(event, data)` | `JsCommand::FireEvent` → V8 thread invokes registered JS handlers | 是 |

### ModelRegistry — Provider Registration

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| `register_provider(name, config)` | `modelRegistry.registerProvider(name, config)` — inserts into `registeredProviders` map | `model_registry.register_provider(&name, config)` — inserts into `Arc<RwLock<HashMap>>` | 是 |
| Clone shares provider map | N/A (single instance) | `ModelRegistry::clone()` shares `registered_providers` via `Arc` (models and models_json_providers are deep-copied — they are read-only after construction) | 是 |
| Post-bind live registration | `runtime.registerProvider = (name, config) => modelRegistry.registerProvider(...)` | `RuntimeActions.register_provider` closure captures a `ModelRegistry` clone (shares the Arc) → calls `register_provider` | 是 |

### ResolvedCommand

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| `source_info` | `RegisteredCommand.sourceInfo` (always present) | `SourceInfo` (non-optional, carried from `RegisteredCommand`) | 是 |
| `invocation_name` dedup | `resolveRegisteredCommands()` — `:N` suffix on name collision | `resolve_extension_commands()` — identical dedup logic | 是 |

### GetCommands (updated)

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 获取命令列表 | Extension commands → prompt templates → skills | Extension commands (`resolve_extension_commands`) → prompt templates → skills | 是 |
| Extension command source | `source: "extension"` | `SlashCommandSource::Extension` | 是 |
| 响应格式 | Array of `RpcSlashCommand` | Array of `SlashCommandInfo` (same wire format) | 是 |

### PackageManager — Settings Persistence

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| `installAndPersist(source, {local})` | `await install(source)` → `addSourceToSettings(source, {local})` | `install(source, local)?` → `add_source_to_settings(source, local)` | 是 |
| `removeAndPersist(source, {local})` | `await remove(source)` → `removeSourceFromSettings(source, {local})` | `remove(source, local)?` → `remove_source_from_settings(source, local)` | 是 |
| `addSourceToSettings` — new source | Appends `PackageSource.String(normalizedSource)` to `packages` array (global or project scope) | Appends `PackageSource::String(normalized)` to `get_packages()` / `get_project_packages()` → `set_packages()` / `set_project_packages()` | 是 |
| `addSourceToSettings` — existing match | Updates the matching entry's source string (preserves object fields if `PackageSource::Object`) | Same: finds match via `package_sources_match`, updates `source` field, preserves other fields | 是 |
| `addSourceToSettings` — no change | Returns `false` when existing source string equals normalized source | Returns `false` when `existing_str == normalized` | 是 |
| `removeSourceFromSettings` — match | Filters out matching entries, persists, returns `true` | Same: `package_sources_match` filter, `set_packages`/`set_project_packages`, returns `true` | 是 |
| `removeSourceFromSettings` — no match | Returns `false` | Returns `false` | 是 |
| `parseSource` | Parses `npm:spec`, git URLs, local paths into typed `ParsedSource` | `parse_source()` — same logic: `npm:` prefix → Npm, git/github/http/https/ssh → Git (simplified), else Local | 是 |
| Source match key — npm | `npm:{name}` (name extracted from spec via regex) | `npm:{name}` (name extracted via manual `parse_npm_spec`) | 是 |
| Source match key — git | `git:{host}/{path}` | `git:{host}/{path}` | 是 |
| Source match key — local | `local:{resolvePath(path)}` vs `local:{resolvePathFromBase(path, baseDir)}` | `local:{resolve_path(path, cwd)}` vs `local:{resolve_path(path, base_dir)}` | 是 |
| No SettingsManager | N/A (TS always has SettingsManager) | Returns `false` (no-op) when `settings_manager` is `None` | 是（有意偏差：Rust 允许无 SettingsManager 运行） |

## find 工具（`core/tools/find.rs`）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 隐藏文件 | fd `--hidden` 包含隐藏文件 | `ignore` walker `hidden(false)` 包含隐藏文件 | 是 | 修复见 PORTING_MISTAKES.md #15 |
| `.gitignore` 尊重 | fd 尊重 `.gitignore`/`.ignore`/`.git/info/exclude` | `ignore` crate 同样尊重 | 是 | 修复见 PORTING_MISTAKES.md #15 |
| 嵌套 git 仓库边界 | git 仓库内父 `.gitignore` 规则在嵌套仓库边界停止（#5960） | `require_git(inside_git)` 实现同语义 | 是 | 修复见 PORTING_MISTAKES.md #15 |
| 仓库外搜索 | fd `--no-require-git`（仍尊重 `.gitignore`） | `require_git(false)` 同语义 | 是 | 修复见 PORTING_MISTAKES.md #15 |
| 默认忽略 | `**/node_modules/**`、`**/.git/**` | 同左（ignore 列表保留） | 是 | — |
