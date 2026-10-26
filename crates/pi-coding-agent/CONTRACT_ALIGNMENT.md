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
| 扩展启用状态 | Extensions enabled in RPC mode（由 `--no-extensions` 传入） | Extensions enabled in RPC mode（`enable_extensions` = CLI `--no-extensions` 取反；内置 registry 在 `create_agent_session` 内按 settings `extensionsEnabled` 构建，**默认关闭、需显式 `true`**） | 是（有意偏差，见 DEVIATIONS.md #24） |

## RPC Protocol

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 输入格式 | JSONL on stdin | JSONL on stdin | 是 |
| 输出格式 | JSONL on stdout | JSONL on stdout (single channel) | 是 |
| 信号处理 | SIGHUP → exit code 129, SIGTERM → exit code 143 | SIGHUP → exit code 129, SIGTERM → exit code 143 | 是 |
| 关闭防护 | `shuttingDown` guard | `shutting_down` AtomicBool guard | 是 |
| 事件流 | Agent events streamed as JSONL, `type`-discriminated union (`{"type":"message_update",...}`), camelCase fields, `partial` stripped from `assistantMessageEvent` (TS `toJsonEvent`) | Agent events streamed as JSONL, `type`-discriminated union, camelCase fields, `partial` stripped via `to_json_event()` | 是 |
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
| `register_provider(name, config)` | `modelRegistry.registerProvider(name, config)` — merges config into `extensionProviders`, validates (`validateExtensionProvider` throws) | `model_registry.register_provider(&name, config) -> Result<(), String>` — validates `api`/`baseUrl`, stores config, returns `Err` instead of throwing | 是 |
| 注册携带 `models` | `applyExtension`: 有 `models` 时**替换**该 provider 全部模型（无 `models` 但有 `baseUrl` 时改写现有模型 baseUrl）；`api`/`baseUrl` 依次取 model → provider config → `findModelDefaults`（same id → same api → first openai-completions → first model） | 同：替换现有模型；无 `models` 时 `baseUrl` 改写现有模型 baseUrl；`api`/`baseUrl` 依次取 model → provider config → `find_model_defaults`（same id → same api → openai-completions → 首个模型）；模型级 `samplingParams` 保留 | 是 |
| `unregister_provider(name)` | `unregisterProvider`: 移除扩展层，`recomposeProvider` 恢复 built-in/models.json 模型（含被 baseUrl 改写的原始 baseUrl） | 移除注册配置并恢复注册前的模型快照（`replaced_models`，替换与 baseUrl 改写都触发首次快照） | 是 |
| `oauth` / `streamSimple` / `refreshModels` | `ProviderConfig` 支持 OAuth 登录回调、自定义 streamSimple、模型刷新 | 未支持（无 OAuth 登录流，见 DEVIATIONS.md #21；JS 扩展运行时不在 main 分支） | 是（有意偏差，见 DEVIATIONS.md #21） |
| Clone 共享注册状态 | N/A（`ModelRuntime` 单实例） | `ModelRegistry::clone()` 通过 `Arc` 共享 `models` / `registered_providers` / `models_json_providers` / `replaced_models`，扩展钩子注册的模型对持有另一 clone 的 session 可见 | 是 |
| Post-bind live registration | `runtime.registerProvider = (name, config) => modelRegistry.registerProvider(...)` | `install_register_provider_hook` 安装闭包，payload 为 `{ "providerId", "config" }`（TS 的 provider id 与 config.name 是两个参数） | 是 |
| 注册失败处理 | 抛异常，由 loader 记入扩展诊断 | payload 解析/校验失败返回 `Err(String)`，不再静默丢弃 | 是 |

### createAgentSession — Model / Thinking Level 恢复

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
|---------|-----------|--------------|--------|
| 恢复 session 模型 | `existingSession.model` → `getModel` + `hasConfiguredAuth`，成功则用其作为初始模型 | `session_manager.build_context()` → `model_registry.find` + `has_configured_auth` | 是 |
| 恢复失败 | `Could not restore model {provider}/{id}`，随后初始模型解析成功时追加 `. Using {p}/{id}` | 同文案、同追加顺序 | 是 |
| `isContinuing` | `findInitialModel({ isContinuing: hasExistingSession })`，续聊时跳过 scoped models | `find_initial_model(..., has_existing_session, ...)` | 是 |
| 恢复 thinking level | `options.thinkingLevel` → session 记录（仅当存在 `thinking_level_change`，否则 settings 默认）→ settings 默认 → `DEFAULT`，最后按模型 `clampThinkingLevel`，无模型为 `off` | 同顺序；`--models provider/id:level` 的显式 level 等价于 TS main.ts 写入 `options.thinkingLevel`（仅新会话、未指定 `--thinking` 时生效）；`clamp_thinking_level`；`has_thinking_entry = get_branch().any(ThinkingLevelChange)` | 是 |
| CLI `--thinking` | `options.thinkingLevel = parsed.thinking`（优先于 scoped/默认） | `thinking_level: args.thinking.clone()`（此前被忽略，见 PORTING_MISTAKES.md） | 是 |
| 无模型 session | 仍创建 session（空 model），`modelFallbackMessage = formatNoModelsAvailableMessage()`，thinking = `off` | 同 | 是 |

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

## 内置工具注册与激活（`core/tools`、`core/agent_session.rs`、`core/settings_manager.rs`、`core/sdk.rs`）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
| -------- | ----------- | ------------- | -------- |
| 内置工具注册 | `createAllToolDefinitions()` 注册 read/bash/powershell/edit/write/grep/find/ls；默认只激活 `DEFAULT_TOOL_NAMES`（read/bash/edit/write） | `tools::create_all_tools()` 注册 read/bash/edit/write/grep/find/ls（powershell 未移植）；默认激活同左 | 是 |
| `--tools` 白名单 | `allowedToolNames`：只保留名单内工具，且“点名即激活”（即使默认未激活） | `allowed_tool_names`：先过滤 registry，激活逻辑 `allowed_names.contains` 命中即激活 | 是 |
| `--exclude-tools` | `_isAllowedTool` 同时过滤 registry，被排除的工具无法再激活 | 同左（先过滤再快照 registry；`getAllTools()` 也看不到） | 是 |
| `--no-tools` | `allowedToolNames = []`，registry 为空 | `NoToolsMode::All` → `allowed_tool_names = Some([])` | 是 |
| `--no-builtin-tools` | 初始激活为空，registry/扩展工具保留 | `NoToolsMode::Builtin` → `initial_active = []`，registry 保留 | 是 |
| `defaultTools` 设置 | `getDefaultTools() ?? DEFAULT_TOOL_NAMES`；`+name`/`-name` 增量（`resolveDefaultTools`），项目层 modifier-only 列表叠加用户层（`mergeDefaultTools`） | `SettingsManager::get_default_tools()` 同款解析；`deep_merge_settings` 对 `defaultTools` 调用 `merge_default_tools` | 是 |
| `/reload` 新增 `defaultTools` | `usesDefaultTools` 时激活新增项；移除项保持激活，会话中手动关掉的保持关闭 | `uses_default_tools` + `reload()` 同左 | 是 |
| 同名工具覆盖 | registry 按名索引，custom/extension 覆盖同名内置（保留首次插入位置） | `tool_list` 按名去重（保留首次位置，后者覆盖） | 是 |
| `setActiveToolsByName()` | 从完整 registry 解析，未知名忽略，重建 system prompt | `set_active_tools_by_name()` 同左 | 是 |
| `getAllTools()` 来源 | 内置 + 扩展 + SDK 定义，来源标注 `<builtin:name>` / `<sdk:name>` | `tool_definitions` 先插入内置定义（`create_builtin_source_info`），custom 覆盖、extension 不覆盖 custom | 是 |
| system prompt 工具段 | 只声明当前激活工具（`_rebuildSystemPrompt(activeNames)`），snippet 表覆盖整个 registry | 先激活、再以激活名单构建 prompt；snippet/guidelines 来自过滤后的 registry | 是 |
| 隐藏暴露（`hidden`/`model-only`）、`prepareLoadout`、pending tool | TS 有 | 未实现 | 否（未移植） |

## ACP Mode（新增功能，无 TS 对应物）

> ACP mode 是 pi-rs 新增功能（TS 原版无原生 ACP，Zed 里靠第三方 `pi-acp` 适配器）。
> 行为基准是 ACP 规范（agentclientprotocol.com）与 `pi-acp` 适配器的映射方式。

| 行为场景 | ACP 规范 / pi-acp 行为 | pi-rs ACP 行为 | 是否一致 |
|---------|----------------------|---------------|--------|
| initialize | 返回协议版本 + capabilities + agent info | 返回 V1 + loadSession/image/list capabilities + `pi-coding-agent` info | 是 |
| session/new | 创建会话，返回 sessionId | 创建 AgentSession（无默认模型时回退内置模型），返回 UUID | 是（见 DEVIATIONS.md #12） |
| session/prompt | 流式 agent_message_chunk / tool_call / tool_call_update，结束返回 stopReason | session task 驱动 prompt，事件经 translate.rs 映射为 ACP 通知，结束返回 EndTurn | 是 |
| prompt 图片 | `ContentBlock::Image` 传 `session.prompt(images)` | 提取 `ImageContent`（data/mime_type）经 `PromptOptions.images` 传给 `prompt()`，同 RPC 模式路径 | 是 |
| session/cancel | 取消进行中的 turn | 向 session task 发 Cancel：actor 置 cancelled 标志 + 非阻塞 spawn `session.abort()`（不在命令循环内等待），turn task 观察到 abort 后以 `cancelled` stopReason 回复，并清空已排队 prompt（同样回复 `cancelled`） | 是 |
| session/list | 列出会话 | 列出内存 registry 中的会话 | 是 |
| session/load | 恢复会话 | 按 ID 从 on-disk map（`{sessions_dir}/acp/session-map.json`）查找，重建 AgentSession 并恢复消息；进程内已加载则 no-op | 是 |
| session/config | 模型/思考级别选择器 | model + thinking_level 两个 select 选项 | 是 |
| 权限流 | ACP request_permission | 未实现（pi 内部 trust 模型自动决策） | 否（有意偏差，见 DEVIATIONS.md #11） |
| MCP | ACP mcpServers（stdio/http） | `session/new`/`session/load` 携带的 MCP 服务器被连接，工具枚举并注入 custom_tools，调用转发回服务器；capabilities 声明 http=true、sse=false；SSE 未实现 | 是（stdio+http；SSE 有意不支持，见 DEVIATIONS.md #14） |


## API-key 认证优先级（A1）

对齐基准：`packages/coding-agent/src/core/{model-runtime,auth-storage,provider-composer}.ts` +
`packages/ai/src/auth/resolve.ts`。

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 密钥解析优先级 | `resolveProviderAuth`：显式 override > stored credential > configured（models.json/扩展）key > ambient env | `get_api_key_and_headers`/`get_api_key_for_provider`：stored → registered → models.json → env | 是 | 修复见 PORTING_MISTAKES.md |
| `hasConfiguredAuth` | `Models.checkAuth` → provider `ApiKeyAuth.check`/`resolve`；`ANTHROPIC_AUTH_TOKEN` 等发现层变量算已配置 | `is_provider_configured` 用 `find_env_keys`（含 AUTH_TOKEN）+ stored + configured | 是 | 修复见 PORTING_MISTAKES.md |
| `getProviderAuthStatus` | runtime > stored > configured > env，四者 `configured:true` | `AuthStorage::get_auth_status` 同序、同布尔语义 | 是 | 修复见 PORTING_MISTAKES.md |
| `getEnvApiKey` 作为请求 key | anthropic 跳过 `ANTHROPIC_AUTH_TOKEN` | `get_env_api_key` 同左 | 是 | |

## 认证命令 `/login` / `/logout`

对齐基准：`modes/interactive/interactive-mode.ts`（`handleLoginCommand` /
`showApiKeyLoginDialog` / `completeProviderAuthentication` / `/logout`）。

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| `/login` 被识别为命令 | 打开 provider 选择器 | 列为内建命令并显示用法/可用 provider，不再当普通消息发出 | 是（选择器改为参数式，见 DEVIATIONS.md #21） | |
| `/login <provider>` | 选择器 → 认证方式 → 密钥对话框 | 按 id/显示名匹配 provider → `Login to X` / `Enter Y` / `> <input>` / 键位提示（editor dock，同原版） | 是（有界子集） | 见 DEVIATIONS.md #21 |
| 密钥持久化 | 写入 auth.json（`AuthStorage.modify`） | 同左（`AuthCredential::ApiKey`，`env: None`） | 是 | |
| 登录后模型选择 | 无模型时选 provider 默认模型（`completeProviderAuthentication`） | `AgentCmd::LoginApiKey`：无模型时用 `DEFAULT_MODEL_PER_PROVIDER` 选默认模型 | 是 | |
| API-key 输入 UI | `LoginDialogComponent.showPrompt`：标题 `Login to {provider}`、消息 `Enter {auth name}`、明文输入、`(escape/ctrl+c to cancel, enter to submit)`，占用编辑器区 | `AppMode::Secret` 同标题/消息/明文输入/键位提示，占用 editor dock | 是 | 见 DEVIATIONS.md #21 |
| `/logout [provider]` | 已存凭据选择器 → 删除 | 参数式删除；无参列出已存凭据并给出 TS 同款提示文案 | 是（有界子集） | 见 DEVIATIONS.md #21 |
| OAuth 登录 | 浏览器回调 / device code | 未实现（`AuthStorage::login` 未实现） | 否 | 见 DEVIATIONS.md #21（范围外） |
| `/login` 可用 provider 列表 | `getLoginProviderOptions` 遍历已注册 provider（含模型目录暂空的动态 provider） | `login_providers_for`：内建 provider 显示名全集 ∪ 注册表 provider（模型目录为空的 `opencode-go` 等也在列） | 是 | 修复见 PORTING_MISTAKES.md |
| `/model` 参数补全数据源 | `getArgumentCompletions` 每次调用读 `scopedModels` 或 `getAvailableSnapshot()`（实时） | `model_argument_completions` 每次请求读共享实时快照；scoped 非空时优先 | 是 | 修复见 PORTING_MISTAKES.md |

## `/reload`（资源重发现）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 |
| -------- | ----------- | ------------- | -------- |
| `/reload` 重读 settings | `settingsManager.reload()` | `settings_manager.reload()` | 是 |
| `/reload` 重发现 skills | `resourceLoader.reload()` → `updateSkillsFromPaths()` 重新扫描 `{agentDir}/skills`、`{cwd}/{CONFIG_DIR_NAME}/skills`、CLI/扩展路径 | `AgentSession` 持有 `Box<dyn ResourceLoader>`（对齐 TS `resourceLoader`），`reload()` 调 `loader.reload()` 重新扫描同一批路径 | 是 |
| `/reload` 重发现 prompt templates / context 文件 | 同 `resourceLoader.reload()` | 同上（`LoadedResources` 整体替换） | 是 |
| `/reload` 刷新 system prompt | 重建含 skills 段的 system prompt | 用新 skills/context 重建 base system prompt | 是 |
| `/reload` 刷新 slash 补全（`/skill:<name>`） | 补全提供者每次从 `resourceLoader` 读取 | 重建补全命令并推送 `Msg::SetCompletionCommands` | 是 |
| `/reload` 重载扩展 | `reload()` 重建 ExtensionRunner（重新从磁盘加载扩展文件） | 扩展为 Rust 原生注册，注册表不重建（见 DEVIATIONS.md；`/reload` 仅刷新 provider/模型补全与资源） | 是（有意偏差，见 DEVIATIONS.md 扩展系统条目） |
| `resourceLoader` 访问器 | TS 属性返回 loader 实例，调用方读 `getSkills()` | Rust `AgentSession::resource_loader()` 返回 owned `LoadedResources` 快照（`get_resources().clone()`）；loader 实例本身保存在 `AgentSession` 内 | 是（内部访问器，非 wire 契约） |

## 配置目录与项目信任

对齐基准：`packages/coding-agent/src/{config,main}.ts`、
`core/{trust-manager,project-trust,settings-manager,resource-loader}.ts`。

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 配置目录名 | `package.json` 的 `piConfig.configDir`（默认 `.pi`）：用户级 `~/.pi/agent/`，项目级 `{cwd}/.pi/` | 硬编码 `.pi-rs`：用户级 `~/.pi-rs/agent/`，项目级 `{cwd}/.pi-rs/` | 是（有意偏差） | 见 DEVIATIONS.md「配置目录与项目信任」 |
| 项目资源识别 | `.pi/{settings.json,extensions,skills,prompts,themes,SYSTEM.md,APPEND_SYSTEM.md}` | `.pi-rs/` 下同名资源（含 `/reload` 重发现） | 是 | 目录名不同，见上 |
| 项目信任门控 | interactive 弹 "Trust project folder?"；未受信任时项目资源全部忽略（`ResourceLoader`/`SettingsManager`/`SYSTEM.md`/扩展） | 不实现门控，全部视为可信任，项目资源无条件加载 | 是（有意偏差） | 见 DEVIATIONS.md「配置目录与项目信任」 |
| print/json 未受信任项目 | `resolveProjectTrusted`（无 UI + ask）返回 false，继续运行但不加载项目资源 | 不再阻断运行；项目资源照常加载（原先的 "Project not trusted" 退出已移除） | 是（有意偏差） | 见 DEVIATIONS.md「配置目录与项目信任」 |

## MCP（原生客户端，新增功能）

> pi-rs 原生 MCP 客户端，非 TS 移植（TS 基线版本无原生 MCP；后续版本的内置 MCP
> 扩展未移植）。行为基准是本 port 自身的设计，见 DEVIATIONS.md #14。

| 行为场景 | 期望行为 | pi-rs 行为 | 是否一致 |
| -------- | -------- | ---------- | -------- |
| 配置文件 | `{agent_dir}/mcp.json`（全局）+ `{cwd}/.pi-rs/mcp.json`（项目），`mcpServers` 通用形状 | 同左；项目条目按名覆盖全局；`enabled:false` 跳过 | 是 |
| 传输 | stdio / streamable-HTTP / legacy SSE | 三种均支持；`type:"sse"` 或 ACP `McpServer::Sse` → SSE | 是 |
| 工具注入 | 各模式（interactive/print/rpc/acp）都可用 MCP 工具 | `create_agent_session` 统一加载并注入 `custom_tools` | 是 |
| 连接失败 | 告警，不阻断会话启动 | 10s 超时，stderr 告警，跳过该服务器 | 是 |
| `${VAR}` / `!cmd` 解析 | env 值、header 值支持 | 复用 `resolve_config_value_or_throw` | 是 |
| `pi-rs mcp list/add/remove/enable/disable` | 管理配置 | 已实现 | 是 |
| exposure / codemode / deferred | （TS 后续版本） | 未实现，全部按 direct 注入 | 否（有意，DEVIATIONS.md #14） |
| MCP resources 工具 | （TS 后续版本）`list_mcp_resources` 等 | 未实现 | 否（有意，DEVIATIONS.md #14） |
| OAuth / `/mcp` TUI | （TS 后续版本） | 未实现 | 否（有意，DEVIATIONS.md #14） |
| 工具刷新（`tools/list_changed`） | TS 支持 | 未实现（session 创建时枚举一次） | 否（有意，DEVIATIONS.md #14） |

## Context token 估算（`core/compaction.rs`，footer 上下文百分比 / 自动压缩阈值）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| `estimateContextTokens` 基准 | 取最后一条有效 assistant usage（跳过 aborted/error/全零）作为已用 token，只对之后的 `trailing` 消息做估算 | 同左（`get_last_assistant_usage_info` + `estimate_agent_messages_tokens(trailing)`） | 是 | |
| 单条消息估算 `estimateTokens(AgentMessage)` | 按 role 取原始 UTF-16 字符数：user/custom/toolResult 文本+图片(4800 字符)、assistant text+thinking+toolCall(name+JSON args)、bash `command.length+output.length`、summary `summary.length`；`Math.ceil(chars/4)` | `estimate_agent_message_tokens` 逐 role 同款（UTF-16 长度、图片 4800、bash 命令+输出、summary 仅 summary），`chars.div_ceil(4)` | 是 | 原 CJK 加权启发式（`cjk*1.5 + non_cjk/4`、图片 100）已移除，对齐 TS |
| `calculateContextTokens(usage)` | `totalTokens \|\| input+output+cacheRead+cacheWrite` | `calculate_context_tokens_from_usage` 同款 | 是 | |
| 自动压缩阈值触发 token 来源 | `_checkCompaction` Case 3：优先最后一条 assistant 的 usage（`calculateContextTokens`），error 或零 usage 才回落 `estimateContextTokens(messages)`（含 stale-pre-compaction 校验） | `compaction::compaction_trigger_tokens` 同款：最后一条 assistant 非 error 且 usage>0 → usage，否则 `estimate_context_tokens`；`check_auto_compact` 用它 | 是 | 见 PORTING_MISTAKES.md #134。context-edit 投影估算分支（TS `estimateProjectedContextTokens`）未单独实现，但 pi-rs `refresh_context` 已把投影写回 agent state，故触发时读到的即投影后消息 |
| 上下文百分比 | `estimate.tokens / contextWindow * 100`（不做 clamp） | `usage_percentage()` 同款（footer 也不 clamp） | 是 | |

## Session 持久化格式与分支 API（`core/session_manager.rs`）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 会话文件字段命名 | 全部 camelCase：`parentId`、`firstKeptEntryId`、`tokensBefore`、`thinkingLevel`、`modelId`、`fromId`、`fromHook`、`customType`、`targetId`、`systemMessage` | `#[serde(rename_all_fields = "camelCase")]`，并保留 snake_case `alias` 读取旧 Rust 文件 | 是 | 修复见 PORTING_MISTAKES.md #153 |
| `CompactionEntry.systemMessage` | `appendCompaction` 用 `getCurrentSystemMessage(buildSessionProjection().messages)` 存压缩点的 prompt/tool 状态（附毫秒 `timestamp`）；投影时排在 summary 前，保留区间剔除旧 system 消息 | 同左（`append_compaction` 抓取、两条投影路径重放、`build_context_entries` 跳过保留区间内的 system 消息） | 是 | 修复见 PORTING_MISTAKES.md #154 |
| 打开会话后的 leaf | `_buildIndex`：`leafId = null` 后按文件顺序遍历，最后一条 entry 即 leaf | `rebuild_index` 同左 | 是 | 修复见 PORTING_MISTAKES.md #155 |
| `resetLeaf()` | `leafId = null`（下一次 append 成为新 root） | `reset_leaf()` 同左（无参） | 是 | 修复见 PORTING_MISTAKES.md #155 |
| `branchWithSummary(branchFromId \| null, summary, details, fromHook, usage)` | `fromId = leafId ?? "root"` → `leafId = branchFromId` → 追加 `parentId = branchFromId` 的 summary（append 后 leaf 指向 summary）；非法 id 抛错 | 同左；非法 id 返回 `Err`（Rust 无异常） | 是 | 修复见 PORTING_MISTAKES.md #155 |
| 每轮 turn 前重读会话文件 | 无此行为（内存即权威） | 已删除 `refresh_config()` 与 turn 前调用 | 是 | 修复见 PORTING_MISTAKES.md #155 |

## Session context 投影与 context edit（`core/session_manager.rs`）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 追加式 context edit | `appendContextEdit(targetId, replacement \| null)`：`null` 从模型上下文省略目标，`{content}` 只替换内容（assistant/toolResult 的字符串 content 归一为 text block） | `SessionManager::append_context_edit` 同款（校验 target 存在 / 在活动分支 / 可编辑） | 是 | |
| 会话投影 | `buildSessionProjection` 保留 provenance（`ProjectedSessionEntry`）；只有最新 compaction（index 0）贡献 checkpoint/summary | `build_session_projection` 同款 | 是 | |
| `SessionEntry` union 增 `usage` | `UsageEntry`：`{type:"usage", kind, provider, model, usage, note?}`，由 `appendUsage()` 追加，不进模型上下文 | `SessionEntry::Usage` 同款（`append_usage` 返回该条目，note 为 `Option<String>` 且 skip none） | 是 | cache warming（#9668）前置；此前无 usage 条目 |
| self-retaining compaction | `appendCompaction(summary, null, ...)` → `firstKeptEntryId = id`（不保留任何前置条目） | `append_compaction(..., None, ...)` 同款 | 是 | |
| 逐请求权威投影 + 扩展边界 | `AgentSession._installAgentRequestProjection` 每次 provider 请求用 projection.messages 覆盖 context；`_installAgentBoundaryHooks` 在 `finishTurn` 分发 `turn_end` 边界，settle 前分发 `agent_before_settle`（草稿先预览后提交、支持 `continue`）| `prepare_request` 每次请求用 `build_projected_context_messages()` 覆盖 context.messages；`finish_turn` 分发 `turn_end` 边界（`run_boundary` + `build_boundary_preview` + `apply_boundary_drafts`，失败丢弃）；`run_settled_continuations` 调 `dispatch_before_settle_boundary` 并支持 `continue_run` | 是（有意偏差：void `on_turn_end` 通知 hook 保留） | 见 DEVIATIONS.md 扩展系统条目 |
| bash 结果持久化 | `recordBashResult` 将 bashExecution 消息 append 到 session，随后 `_refreshFinalizedContext`；运行的排队在 flush 时同样处理 | `record_bash_result` / `_flush_pending_bash_messages` append 到 session 后 `refresh_context()` | 是 | 保证投影不丢 bash 消息（此前只在 agent state，未持久化） |
| 边界延续的可运行性判定 | `_buildBoundaryContext`：`canContinue = (hasNonSystemContext && finalRole !== "assistant") \|\| pendingCustomContext \|\| (turn_end ? hasQueuedMessages() : finalRole === "assistant" && hasQueuedMessages())`；`turn_end` / `agent_before_settle` 在 `continue` 且 `!canContinue` 时经 `_reportInvalidBoundaryContinuation` 上报 `<boundary>` 错误并拒绝该延续 | `build_boundary_preview(boundary, has_queued_messages, pending_custom_context)` 复刻同一公式；两处边界都在提交草稿后用 `build(&[])` 取基线校验 `can_continue`，拒绝时调 `report_invalid_boundary_continuation`（新增 `ErrorListenerHandle` 供 agent 闭包内上报）；`agent_before_settle` 同时补上 `continue \|\| hasQueuedMessages` 项 | 是 | 修复见 PORTING_MISTAKES.md #137/#139 |

## `agent_settled` 派发与延迟续跑（`core/agent_session.rs`）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 事件顺序 | `_emitAgentSettled`：`_isAgentRunActive = false` → 扩展 `emit({type:"agent_settled"})` → 会话 `_emit(...)` | 同左（扩展先于会话监听器，`is_emitting_agent_settled` 覆盖整个派发窗口） | 是 | 修复见 PORTING_MISTAKES.md #138（此前顺序相反） |
| 派发期间的续跑请求 | `_isEmittingAgentSettled` 期间 `prompt()` / `sendCustomMessage({triggerTurn})` 压入 `_deferredSettledActions`，派发结束后按序回放，再 `_resolveIdleWaitIfIdle()` | 同左（`DeferredSettledAction::{Prompt, CustomTurn}` 队列 + `run_deferred_settled_actions`）；`send_message` 的 `triggerTurn` 分支在追加 session entry 前入队 | 是 | handler 因此不会看到重入的 `agent_start` |
| `agent_settled` 时缓存预热 | `_cacheWarmer?.onAgentSettled()` | 无（cache warming #9668 实现中） | 见 DEVIATIONS.md #27 | |

## Prompt cache warming（`core/cache_warmer.rs`，TS #9668）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 刷新时机 | `getCacheWarmingDelayMs`：`min(ttl*0.9, ttl-10s)`，`ttl <= 10s` 不刷新 | 同左（`get_cache_warming_delay_ms`） | 是 | |
| 缓存寿命 | `getPromptCacheTtlMs`：按 `options.cacheRetention`（缺省时看 `PI_CACHE_RETENTION`）取 `model.promptCache[tier]`；`none` → 不刷新 | 同左；`promptCache` 由生成器只为 `anthropic` + `anthropic-messages` 写入（同 TS `applyPromptCacheMetadata`） | 是 | |
| 可重放性 | `isReplayable`：非 reasoning 或非 anthropic-messages → true；否则要求 `forceAdaptiveThinking === true` | 同左 | 是 | |
| 经济性判定 | `evaluate`：`warmCost` = cache read + 1 output token；`missCost` = max(0, cacheWrite-or-input − cacheRead)；`continuationProbability` = idle ? 0.15 : 1；`expectedSavings >= $0.05` 才 warm | 同左（`evaluate`，阈值 `CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS`） | 是 | |
| 安全窗口 | `MAX_WARMING_AGE_MS` 1h（streaming）/ `MAX_IDLE_WARMING_AGE_MS` 30m（idle）+ `refreshDeadlineAt`（过期即停） | 同左（`schedule`/`refresh_deadline_missed`） | 是 | |
| 模式与生命周期 | `_cacheWarmer`：`onAgentSettled`（streaming 模式在 settle 时停）、`onModeChanged`、`cancel`、`status`；`agent_settled` 派发时最先调用 | 同左（`AgentSession::_emit_agent_settled` 先调 `on_agent_settled`；`dispose_inner` 里 `on_warmed = None` + `cancel`；`cache_warming_status`/`cache_warming_mode_changed` 公开） | 是 | |
| 刷新请求来源 | `streamFn` 包装：仅 `options.sessionId === sessionManager.getSessionId()` 时 `start()`（compaction/summary 的路由 id 不重置缓存条目）；`isCurrent` 比较 model + 消息前缀 | 同左（`AgentSession::new` 里的 stream_fn 包装 + `CacheWarmerSlot`；前缀比较用值相等而非对象同一性，等价于 TS 的不可变消息对象语义） | 是 | |
| 决策事件 | `cache_warming_decision`：handler 可返回 `{action}` 覆盖；handler 抛错回落到 pi 的判断 | 同左（`pi-extension-api::HookHandler::on_cache_warming_decision` + `fire_cache_warming_decision`；`DecideFn` 回落） | 是 | |
| 刷新记账 | 成功刷新经 `appendUsage("cache_warm", provider, model, usage, note?)` 落一条 `usage` 条目 | 同左（`append_usage` + `SessionEntry::Usage`），`on_warmed` 回调可用于发出 `entry_appended` | 是 | |
| 展示面 | `/session` 状态行、转录 notice、设置选择器 | 格式化函数已移植但无调用方 | 见 DEVIATIONS.md #27 | 展示面属 TUI 组件层 |

## P1 批次：工具采样、重试上限、压缩预算、context 相位

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 内置工具 constrained sampling | `read`/`bash`/`edit`/`write` 默认声明 `{type:"json_schema",strict:"prefer"}`（不需要 `PI_EXPERIMENTAL`）；`grep`/`find`/`ls` 不声明；扩展工具用自己声明的值 | 同左（`tools::preferred_json_schema_sampling()` + 扩展 `def.constrained_sampling`） | 是 | 修复见 PORTING_MISTAKES.md #141 |
| agent 重试退避上限 | 每档 delay 上限 `maxAgentDelayMs ?? 60_000`，可由 `retry.maxAgentDelayMs` 配置 | 同左（`retry_delay_ms` + `RetrySettings.max_agent_delay_ms()`） | 是 | 修复见 PORTING_MISTAKES.md #143 |
| per-model 压缩预算 | `compaction.modelOverrides["provider/id"]` → 普通设置 → 内置默认 | 同左（`CompactionSettings::for_model`，自动压缩与压缩触发点按当前模型解析） | 是 | 修复见 PORTING_MISTAKES.md #144 |
| `context` / `context_with_system` 相位 | `context` handler 只见对话，Pi 跑完补回 prompt/工具声明；`context_with_system` 见全量 transcript、结果原样生效、丢首条 system 报错 | 同左（`run_on_context` 过滤+恢复、`run_context_with_system` + dispatcher + transform_context 串联） | 是 | 修复见 PORTING_MISTAKES.md #145 |
| `toolcall_start` 线事件 | 剥 `partial` 并补 `id`/`toolName`（#7953） | 同左 | 是 | 修复见 PORTING_MISTAKES.md #140 |
| 模型/思考级别变更的作用域 | `setModel(model, {persist})` / `setThinkingLevel(level, {persist})`：默认**会话内**，仅显式 persist 才写全局默认（#8356） | 同左（两个 setter 都有 `persist: bool`；interactive/RPC/ACP 调用点传 `false`） | 是 | 修复见 PORTING_MISTAKES.md #149 |
| per-model 思考级别 | `modelThinkingLevels["provider/modelId"]`：切换模型时 显式 → per-model → 全局默认 → 会话当前 | 同左（`Settings.model_thinking_levels` + `_get_thinking_level_for_model_switch`） | 是 | 修复见 PORTING_MISTAKES.md #151 |
| 会话树导航前置检查 | 运行中 / 压缩中拒绝导航（#9179） | 同左（返回 `false`，不抛错） | 是 | 修复见 PORTING_MISTAKES.md #150 |

## Interactive TUI assistant errors

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| assistant `message_start` | 立即创建 `AssistantMessageComponent` | bridge 转发 `AssistantMessageStart`，TUI 立即创建空 assistant 行 | 是 | 修复见 PORTING_MISTAKES.md #152 |
| 首个 token 前的 provider 错误 | `message_end` 即使内容为空也渲染 `Error: <errorMessage>` | 空内容 `MessageEnd` 照常转发并渲染同一错误行 | 是 | 修复见 PORTING_MISTAKES.md #152 |
| 部分输出后失败 | 保留已收到的内容并追加错误行 | agent loop 以 partial 为基准按 block 合并终态、保留顺序并回填 usage；TUI 也不会用空白 termination payload 清空已显示文本 | 是 | 修复见 PORTING_MISTAKES.md #152 |
| 普通完成（stop/toolUse） | 不显示错误 notice | `Stop`/`ToolUse`/`Pending` 映射为无 notice，仅 Length/Aborted/Error 显示 | 是 | |
| abort 文案 | 按 `session.retryAttempt` 显示 `Operation aborted` 或 `Aborted after N retry attempt(s)` | session listener 在 retry counter 清零前快照，bridge 携带该值，TUI 使用同一文案 | 是 | E2E：`tui_abort_during_retry_uses_retry_attempt_wording` |
| 空 `errorMessage` | truthiness 回落到 `Error` / `Unknown error`；精确 `"Request was aborted"` 映射为 `Operation aborted` | bridge 归一化空串为 `None`，渲染器使用 TS 同款 fallback/精确串映射 | 是 | |
| 错误消息含 tool call | 标记 pending tool 组件失败并抑制 assistant 级错误行 | bridge 转发未执行 tool calls；TUI 生成 failed tool 行并抑制 assistant notice | 是 | 修复见 PORTING_MISTAKES.md #152 |
| 恢复空的错误消息 | 每个 assistant 消息都创建组件，空内容错误仍显示 | hydration 对 Error/Aborted/Length/error_message 保留空 assistant 行 | 是 | 修复见 PORTING_MISTAKES.md #152 |

## 内置扩展配置（`core/extensions/mod.rs`、`core/sdk.rs`、`core/settings_manager.rs`）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 内置扩展开关 | 按 `extensions` 资源列表 + 路径模式启停 | `extensionsEnabled: {"<id>": bool}`，缺失=关闭（opt-in） | 否（有界子集） | DEVIATIONS.md #24 |
| `webSearch` 配置 | 无（TS 只调本地 Ollama） | `Settings.web_search`（camelCase `webSearch`：`provider` / `deepseek.{baseUrl,apiKey}` / `ollama.host`），全局/项目按外层键浅合并；`SettingsManager::get_web_search_config()` | 否（新增） | pi-extensions/DEVIATIONS.md「web_search 扩展」 |
| `webSearch.deepseek.apiKey` 解析 | — | 显式值（支持 `${ENV}` / `!cmd`，经 `resolve_config_value`）优先，其次 `ModelRegistry::get_api_key_for_provider("deepseek")`（auth.json → models.json/注册 provider → 环境变量），再交给扩展合并环境变量 | 否（新增） | 同上；`/login deepseek` 存的凭据可直接用于搜索 |
| `--no-extensions` | 只关自动发现 | 总开关，`None` registry | 是 | DEVIATIONS.md #24 |
