# pi-rs 全量复刻进度报告

> 对照 TypeScript 源码逐文件比对  
> 更新日期：2026-06-05 (v3)

---

## 总览

| Crate | TS 源仓库 | 文件数 | 代码行数 | 测试数 | 编译 | 完成度 |
|-------|-----------|:---:|:---:|:---:|:---:|:---:|
| pi-agent-core | `packages/agent` | 32 | 12,603 | 197/197 ✅ | ✅ | ~96% |
| pi-coding-agent | `packages/coding-agent` | 53 | 11,620 | 208/208 ✅ | ✅ | ~55% |
| pi-ai | `packages/ai` | 25 | 6,220 | 167+2/169 ✅ | ✅ | ~65% |
| pi-tui | `packages/tui` | 24 | 8,384 | 191/191 ✅ | ✅ | ~95% |
| **合计** | | **134** | **38,827** | **765** | | |

> 注：pi-tui 文件数从 27 减至 24 是因为去除了 `keys.rs`/`native_modifiers.rs`/`stdin_buffer.rs` 三个 Rust 独有胶水模块（被 crossterm 原生替代），测试减少源于移除模块的对应测试。功能完整性不变。

---

## 一、pi-agent-core（32 文件 / 12,603 行 / 完成度 ~96%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/agent`

### 类型指标

struct: 76 | enum: 27 | trait: 4 | pub fn: 138 | impl block: 32

### 完整复刻（15 个文件）

| TypeScript | Rust | 说明 |
|------------|------|------|
| `index.ts` | `lib.rs` | barrel 导出 |
| `types.ts` | `types.rs` | AgentMessage / AgentEvent / AgentTool / AgentState 等全部类型 |
| — | `extraction.rs` | Extractor 结构化提取 trait（Rust 独有增强） |
| `harness/messages.ts` | `harness/messages.rs` | 消息转换、摘要常量 |
| `harness/system-prompt.ts` | `harness/system_prompt.rs` | skill XML 格式化 |
| `harness/compaction/utils.ts` | `harness/compaction/utils.rs` | 文件操作提取 |
| `harness/env/nodejs.ts` | `harness/env/nodejs.rs` | NodeExecutionEnv |
| `harness/session/session.ts` | `harness/types.rs`（合并） | Session 结构体 |
| `harness/session/jsonl-repo.ts` | `harness/session/jsonl_repo.rs` | JSONL 仓库 |
| `harness/session/jsonl-storage.ts` | `harness/session/jsonl_storage.rs` | JSONL 存储 |
| `harness/session/memory-repo.ts` | `harness/session/memory_repo.rs` | 内存仓库 |
| `harness/session/memory-storage.ts` | `harness/session/memory_storage.rs` | 内存存储 |
| `harness/session/repo-utils.ts` | `harness/session/repo_utils.rs` | 会话工具函数 |
| `harness/utils/truncate.ts` | `harness/utils/truncate.rs` | 文本截断 |
| — | `pi_ai_types.rs` | 外部 AI 类型映射（Rust 独有） |

### 部分复刻（9 个文件）

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `agent-loop.ts` | `agent_loop.rs` | ~95% | — |
| `agent.ts` | `agent.rs` | ~98% | — |
| `proxy.ts` | `proxy.rs` | ~90% | — |
| `harness/agent-harness.ts` | `harness/agent_harness.rs` | ~70% | prompt() 不直接运行 agent loop（由 Agent 负责） |
| `harness/prompt-templates.ts` | `harness/prompt_templates.rs` | ~90% | `loadPromptTemplates` / `loadSourcedPromptTemplates` 已实现 |
| `harness/skills.ts` | `harness/skill_loader.rs` + `skills.rs` | ~85% | `formatSkillInvocation` / `loadSourcedSkills` 已实现 |
| `harness/types.ts` | `harness/types.rs` | ~90% | ExecutionEnv 合并了 FileSystem + Shell |
| `harness/compaction/compaction.ts` | `harness/compaction/compaction.rs` | ~90% | `generate_summary` 已接入 pi_ai LLM |
| `harness/compaction/branch-summarization.ts` | `harness/compaction/branch_summarization.rs` | ~85% | `generate_branch_summary` 已接入 pi_ai LLM |
| `harness/utils/shell-output.ts` | `harness/utils/shell_output.rs` | ~90% | 完整输出写入临时文件 |

### 未复刻（2 个文件）

| TypeScript | 说明 |
|------------|------|
| `node.ts` | Node.js 入口（Rust crate 自身即入口） |
| `harness/session/uuid.ts` | UUIDv7（Rust 用 uuid crate 的 v4） |

### agent_loop.rs 实现状况（~95%）

已实现功能：
- 多轮循环（内层 `has_more_tool_calls` + 外层 `follow_up`）
- 顺序工具执行（`execute_tool_calls_sequential`）
- 并行工具执行（`execute_tool_calls_parallel`）
- `run_agent_loop` 和 `run_agent_loop_continue` 完整实现
- `before_tool_call` / `after_tool_call` hooks
- `prepare_next_turn` / `should_stop_after_turn` hooks
- `get_steering_messages` / `get_follow_up_messages` 支持
- 取消信号集成
- 流式响应处理
- 32 个单元测试 + 12 个集成测试（已对齐 `agent-loop.test.ts`）

### agent.rs 实现状况（~98%）

- `process()` / `continue_run()` — 连线到 agent_loop，含 active run 检查
- `steer()` / `follow_up()` — PendingMessageQueue（All / OneAtATime 模式）
- `abort()` — CancellationToken 取消
- `cancellation_token()` — 暴露当前运行的取消信号
- `create_event_sink()` — 事件分发 + AgentState 同步（messages/pendingToolCalls/errorMessage/isStreaming）
- 错误事件发射（handleRunFailure 模式，失败时发射 MessageStart/End/TurnEnd/AgentEnd）
- `finish_run()` — 统一清理状态
- 全部 hooks 已连线

### extraction.rs（新增，583 行）

Extractor trait + 实现：从 agent 输出中提取结构化 JSON，支持 schema 验证。

### P0 阻塞项

1. ~~`generate_summary()` — 返回占位文本~~ ✅ 已接入 pi_ai::stream，真实 LLM 调用
2. ~~`generate_branch_summary()` — 返回占位文本~~ ✅ 已接入 pi_ai::stream，真实 LLM 调用
3. ~~`loadPromptTemplates()` — 不存在~~ ✅ 已实现（frontmatter 解析 + 目录扫描）
4. ~~AgentHarness compact() API key 空字符串~~ ✅ 已通过 env 解析 API key
5. ~~AgentHarness 编排循环~~ ✅ `process()` / `continue_run()` 完整连线，含 active run 检查
6. **AgentState 消息同步** — `create_event_sink` 现在在 `MessageEnd` 时自动推送到 `state.messages`

---

## 二、pi-coding-agent（53 文件 / 11,620 行 / 完成度 ~55%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/coding-agent`

### 类型指标

struct: 163 | enum: 30 | trait: 8 | pub fn: 271 | impl block: 65

### 已有 Rust 对应文件的模块

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `config.ts` | `config.rs` | ~25% | 包检测、安装方式、自更新、全局包路径 |
| `core/event-bus.ts` | `core/event_bus.rs` | ~80% | `unsubscribe` 是空操作（内存泄漏） |
| `core/diagnostics.ts` | `core/diagnostics.rs` | ~90% | `error` 诊断类型变体 |
| `core/session-manager.ts` | `core/session_manager.rs` | ~75% | ReadonlySessionManager、getLatestCompactionEntry |
| `core/settings-manager.ts` | `core/settings_manager.rs` | ~50% | 无文件锁、缺失 10+ 设置字段、无运行时覆盖 |
| `core/slash-commands.ts` | `core/slash_commands.rs` | ~95% | — |
| `core/messages.ts` | `core/messages.rs` | ~85% | 工厂函数 |
| `core/context-usage.ts` | `core/context_usage.rs` | ~95% | — |
| `core/model-registry.ts` | `core/model_registry.rs` | ~65% | OAuth 支持（auth-storage 已就位） |
| `core/model-resolver.ts` | `core/model_resolver.rs` | ~50% | defaultModelPerProvider（25+ provider）、别名检测 |
| `core/system-prompt.ts` | `core/system_prompt.rs` | ~85% | — |
| `core/skills.ts` | `core/skills.rs` | ~40% | frontmatter 解析、名称/描述验证、gitignore 感知 |
| `core/prompt-templates.ts` | `core/prompt_templates.rs` | ~30% | `$1`/`$@`/`${@:N}` 系统缺失 |
| `core/resource-loader.ts` | `core/resource_loader.rs` | ~40% | 主题加载、祖先目录扫描、PackageManager 集成 |
| `core/extensions/` | `core/extensions.rs` | ~35% | 无扩展运行时、无事件钩子、无 worker |
| `core/compaction/` | `core/compaction.rs` | ~50% | `compact()` 主函数、token 计数、分支摘要 |
| `core/agent-session.ts` | `core/agent_session.rs` | ~60% | 事件系统（10+ 事件类型）、自动重试、压缩集成 |
| `core/agent-session-runtime.ts` | `core/agent_session_runtime.rs` | ~85% | 会话运行时（流式/重试/压缩编排） |
| `core/agent-session-services.ts` | `core/agent_session_services.rs` | ~90% | DI 容器、服务注册 |
| `core/auth-guidance.ts` | `core/auth_guidance.rs` | ~90% | 认证引导消息 |
| `core/auth-storage.ts` | `core/auth_storage.rs` | ~90% | 加密认证存储 |
| `core/defaults.ts` | `core/defaults.rs` | ~90% | 默认 thinking level |
| `core/exec.ts` | `core/exec.rs` | ~95% | 进程执行抽象（含超时/取消） |
| `core/http-dispatcher.ts` | `core/http_dispatcher.rs` | ~90% | HTTP 请求分发 |
| `core/source-info.ts` | `core/source_info.rs` | ~90% | 资源源元数据 |
| `core/bash-executor.ts` | `core/bash_executor.rs` | ~40% | 无流式输出、无 output buffer 管理、无 sanitizeBinaryOutput |
| `core/footer-data.ts` | `core/footer_data_provider.rs` | ~70% | Git 分支检测、扩展状态、提供者计数 |
| — | `core/sdk.rs` | — | Rust 独有：SDK 集成层（DI 容器），183 行 |
| — | `core/output_guard.rs` | — | Rust 独有：输出保护器 |
| — | `core/provider_attribution.rs` | — | Rust 独有：提供者归属标记 |
| — | `core/provider_display_names.rs` | — | Rust 独有：提供者显示名映射 |
| — | `core/resolve_config_value.rs` | — | Rust 独有：配置值解析 |
| — | `core/session_cwd.rs` | — | Rust 独有：会话工作目录管理 |
| — | `core/telemetry.rs` | — | Rust 独有：遥测事件收集 |
| — | `core/timings.rs` | — | Rust 独有：性能计时器 |

### 工具模块

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `core/tools/index.ts` | `core/tools/mod.rs` | ~80% | — |
| `core/tools/bash.ts` | `core/tools/bash.rs` | ~35% | 超时参数被忽略、无 spawn hook、无进程树管理、无流式输出 |
| `core/tools/edit.ts` | `core/tools/edit.rs` | ~60% | 模糊匹配、Unicode 标准化（edit-diff 引擎已到位） |
| `core/tools/edit-diff.ts` | `core/tools/edit_diff.rs` | ~90% | Diff 计算引擎（替换 `string.replace()`） |
| `core/tools/file-mutation-queue.ts` | `core/tools/file_mutation_queue.rs` | ~95% | 文件变异序列化队列 |
| `core/tools/output-accumulator.ts` | `core/tools/output_accumulator.rs` | ~95% | 流式输出累积器 |
| `core/tools/tool-definition-wrapper.ts` | `core/tools/tool_definition_wrapper.rs` | ~90% | AgentTool → ToolDefinition 包装 |
| `core/tools/read.ts` | `core/tools/read.rs` | ~40% | 无图片处理、无语法高亮、无 macOS 路径变体 |
| `core/tools/write.ts` | `core/tools/write.rs` | ~50% | 无语法高亮、无增量缓存（file-mutation-queue 已就位） |
| `core/tools/grep.ts` | `core/tools/grep.rs` | ~30% | 纯 Rust regex vs ripgrep 二进制（架构不同，无 gitignore 感知） |
| `core/tools/find.ts` | `core/tools/find.rs` | ~30% | 纯 Rust glob vs fd 二进制（架构不同，无 gitignore 感知） |
| `core/tools/ls.ts` | `core/tools/ls.rs` | ~45% | 无大小写不敏感排序、无 stat 逐项检查 |
| `core/tools/truncate.ts` | `core/tools/truncate.rs` | ~80% | DEFAULT_MAX_BYTES 256KB（TS 50KB） |
| `core/tools/path-utils.ts` | `core/tools/path_utils.rs` | ~55% | macOS 专用变体（NFD/screenshot/curly quotes） |
| `core/tools/render-utils.ts` | `core/tools/render_utils.rs` | ~40% | shortenPath、linkPath、图片块处理 |

### 完全未复刻（11+ 个 TS 文件）

| 模块 | 用途 |
|------|------|
| `core/export-html/` (6 文件) | HTML 会话导出 |
| `core/keybindings.ts` | 键盘快捷键 |
| `core/package-manager.ts` | 扩展/skill 包管理 |
| `cli/` (6 文件) | CLI 参数解析、配置选择、会话选择器 |
| `modes/` (7+ 文件) | 运行模式（interactive TUI/print/RPC） |
| `utils/` (28 文件) | 全部工具模块 |
| `bun/` (3 文件) | Bun 运行时 |

### sdk.rs 实现内容（183 行，Rust 独有）

```
create_agent_session()
  → SettingsManager::create()
  → ModelRegistry::new(builtin_models_list())
  → model_resolver::find_initial_model()
  → resource_loader::load_all_resources()
  → SessionManager::new()
  → EventBusController::new()
  → AgentSession::new()
```
- `NoToolsMode` 枚举（All / Builtin）
- scoped models / tools 选择
- model fallback 消息

### P0 阻塞项

1. ~~**Edit diff 引擎完全缺失** — `string.replace()` 替代~~ ✅ 已实现 `edit_diff.rs`
2. ~~**File mutation queue 缺失** — 无并发文件操作保护~~ ✅ 已实现 `file_mutation_queue.rs`
3. **Prompt 模板 `$1`/`$@`/`${@:N}` 系统缺失** — 与 pi-agent-core 的 prompt_templates.rs 不一致
4. **Bash 超时参数被忽略** — 接受但不处理
5. **Grep/Find 纯 Rust 实现** — 原版用 rg/fd 二进制，无 gitignore 感知

---

## 三、pi-ai（25 文件 / 6,220 行 / 完成度 ~65%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/ai`

### 类型指标

struct: 28 | enum: 11 | trait: 0 | pub fn: 46 | impl block: 10

### 模块状态

| TypeScript | Rust | 覆盖率 | 说明 |
|------------|------|--------|------|
| `types.ts` | `types.rs` | ~90% | 1,074 行，35+ public types，含新增 ToolChoice 枚举 |
| `models.ts` | `models.rs` | ~100% | get_model / calculate_cost / thinking levels；RwLock 运行时注册表 |
| `models.generated.ts` | **`build.rs`** | **~90%** | 编译期从 OpenRouter API + models.dev 自动拉取并生成模型数据 |
| `api-registry.ts` | `api_registry.rs` | ~80% | 注册/查找/注销机制完整 |
| `stream.ts` | `stream.rs` | ~100% | stream / complete / streamSimple / completeSimple（含 ToolChoice 参数） |
| `env-api-keys.ts` | `env_api_keys.rs` | ~55% | 25 provider → env var 映射 |
| `utils/event-stream.ts` | `utils/event_stream.rs` | ~60% | pull-based vs push-based 架构差异 |
| `utils/diagnostics.ts` | `utils/diagnostics.rs` | ~30% | 数据模型与 TS 不一致 |
| `utils/json-parse.ts` | `utils/json_parse.rs` | ~70% | JSON repair / clean_partial / parse_streaming_json（15 tests） |
| `utils/validation.ts` | `utils/validation.rs` | ~60% | 工具调用参数验证 + JSON Schema validate（10 tests） |
| `utils/overflow.ts` | `utils/overflow.rs` | **~100%** | 上下文溢出检测：3 种检测策略 + 20+ provider 模式 + 25 tests |
| `utils/typebox-helpers.ts` | `utils/typebox_helpers.rs` | **~100%** | `string_enum()` JSON Schema 辅助函数 + 5 tests |
| `session-resources.ts` | `utils/session_resources.rs` | **~100%** | 会话资源清理注册/反注册/批量清理 + 9 tests |
| **—** | **`utils/headers.rs`** | **~100%** | HeaderMap → HashMap 转换（2 tests，Rust 独有） |
| **—** | **`utils/sse.rs`** | **~100%** | SSE 解析器（共享），23 tests |
| **`providers/anthropic.ts`** | **`providers/anthropic.rs`** | **~60%** | SSE streaming + 消息转换 + 工具转换 + stop reason（20 tests） |
| **`providers/openai-completions.ts`** | **`providers/openai.rs`** | **~60%** | SSE streaming + 消息转换 + 工具转换 + 测试（15 tests） |
| **`providers/register-builtins.ts`** | **`providers/register_builtins.rs`** | **~90%** | 注册 API provider + 编译期加载生成模型数据 |
| **—** | **`providers/simple_options.rs`** | **~90%** | 共享 SimpleStreamOptions → StreamOptions 转换（Rust 独有） |
| **—** | **`providers/deepseek.rs`** | **~90%** | DeepSeek API 包装（OpenAI 兼容），12 行核心逻辑 |
| **—** | **`providers/xai.rs`** | **~90%** | xAI Grok API 包装（OpenAI 兼容），12 行核心逻辑 |
| **—** | **`build.rs`** | **~80%** | 编译期模型生成 |

### 本轮更新（v3 — 新 Provider + ToolChoice）

**新增 Provider（3 个）：**
- `simple_options.rs` — 共享 SimpleStreamOptions → StreamOptions 构建逻辑，供 OpenAI 兼容 provider 复用
- `deepseek.rs` — DeepSeek API 适配（OpenAI 兼容协议，委托 openai::stream_openai）
- `xai.rs` — xAI Grok API 适配（OpenAI 兼容协议，委托 openai::stream_openai）

**新增特性：**
- `ToolChoice` 枚举 + `tool_choice` 字段 — 支持强制工具调用模式（`Mode::Any`/`Mode::Tool`）
- `StreamOptions.tool_choice` — 在 stream/complete/streamSimple/completeSimple 中透传

**状态变化：**
- 测试：167 单元 + 2 doc = **169**（全部通过）
- 完成度：~60% → **~65%**

### 完全缺失（13+ 个 TS 文件）

| 类别 | 数量 | 说明 |
|------|------|------|
| Provider 实现 | ~11 | mistral / google-native / bedrock / azure / vertex / codex / copilot 等 |
| ~~Utils~~ | ~~3~~ | ~~overflow / typebox-helpers / session-resources~~ ✅ 已完成 |
| Images 功能 | 5 | images/models / api-registry / image-models.generated + providers/images |
| 其他 | 3 | index / cli / oauth |

### P0 阻塞项

1. ~~Provider 实现全是空壳~~ ✅ Anthropic / OpenAI / DeepSeek / xAI 已实现
2. **11+ provider 未复刻** — mistral / google-native / bedrock / vertex / codex / copilot 等
3. ~~register-builtins 缺失~~ ✅ 已实现（3+ API 注册）
4. ~~models_generated 手写维护~~ ✅ 已用 build.rs 替代，编译期自动拉取
5. ~~Utils 模块缺失 3+~~ ✅ overflow / typebox-helpers / session-resources 已完成

---

## 四、pi-tui（24 文件 / 8,384 行 / 完成度 ~95%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/tui`

### 类型指标

struct: 52+ | enum: 14+ | trait: 8+ | pub fn: 190+ | impl block: 65+

### 模块状态

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `components/spacer.ts` | `components/spacer.rs` | ~100% | |
| `components/text.ts` | `components/text.rs` | ~90% | |
| `components/truncated-text.ts` | `components/truncated_text.rs` | **~95%** | ✅ 单行截断，5 tests |
| `components/input.ts` | `components/input.rs` | ~70% | 已集成 grapheme 分词 |
| `components/editor.ts` | `components/editor.rs` | ~85% | 多行编辑器，kill-ring/undo/yank/paste/autocomplete/word-nav，1,879 行，16 tests |
| `components/markdown.ts` | `components/markdown.rs` | **~90%** | ✅ pulldown-cmark 渲染（标题/代码块/列表/表格/行内样式/引用/链接），1,261 行 |
| `components/select-list.ts` | `components/select_list.rs` | ~75% | 过滤/选择/主题/滚动/wrapping |
| `components/settings-list.ts` | `components/settings_list.rs` | **~90%** | ✅ 可搜索设置列表，值循环，7 tests |
| `components/loader.ts` | `components/loader.rs` | **~95%** | ✅ 动画 spinner（10 frames/80ms/颜色回调），7 tests |
| `components/cancellable-loader.ts` | `components/cancellable_loader.rs` | **~95%** | ✅ 可取消 spinner（Arc\<AtomicBool\> 信号），5 tests |
| `components/box.ts` | `components/box_component.rs` | ~70% | 缺 removeChild、functional background |
| `components/image.ts` | `components/image.rs` | **~90%** | ✅ 终端图片（ratatui-image：Kitty/iTerm2/Sixel/Halfblocks），7 tests |
| `keybindings.ts` | `keybindings.rs` | ~85% | KeybindingsManager / 冲突检测 / 覆盖 |
| `keys.ts` | **通过 crossterm::event KeyEvent** | **~100%** | ✅ 不再需要独立 keys.rs — 用 crossterm 原生 KeyEvent |
| `tui.ts` | `tui.rs` | ~70% | Container/Component trait/渲染管线；无 diff 渲染 |
| `terminal.ts` | `terminal.rs` | ~55% | 缺 Kitty 协商/paste/Apple Terminal 检测 |
| `terminal-image.ts` | *ratatui-image* | **~100%** | 底层图片协议实现（通过 ratatui-image 覆盖） |
| `utils.ts` | `utils.rs` | ~75% | strip_ansi 已修复（APC/OSC/DCS），hyperlink 支持 |
| `editor-component.ts` | `editor_component.rs` | **~95%** | ✅ Editor 插件接口 trait |
| `kill-ring.ts` | `kill_ring.rs` | **~95%** | ✅ Emacs kill-ring（push/rotate/yank/accumulate），10 tests |
| `undo-stack.ts` | `undo_stack.rs` | **~95%** | ✅ 泛型撤销栈，7 tests |
| `word-navigation.ts` | `word_navigation.rs` | **~95%** | ✅ 单词级导航（forward/backward + atomic segments），18 tests |
| `fuzzy.ts` | `fuzzy.rs` | **~95%** | ✅ 基于 fuzzy-matcher crate 的模糊匹配（多 token + 排序），4 tests |
| `stdin-buffer.ts` | **通过 crossterm event-stream** | **~100%** | ✅ 不再需要独立 stdin_buffer.rs |
| `autocomplete.ts` | `autocomplete.rs` | **~90%** | ✅ 文件路径补全 + 斜杠命令 + @前缀，10 tests（669 行） |
| `index.ts` | `lib.rs` | **~95%** | ✅ barrel 导出 |
| `native-modifiers.ts` | **通过 crossterm** | **~100%** | ✅ 不再需要独立 native_modifiers.rs |

### 完全缺失模块

| 模块 | 说明 |
|------|------|
| 无 | **全部 26 个 TS 源文件均已复刻**（`keys.ts`/`native-modifiers.ts`/`stdin-buffer.ts` 通过 crossterm 覆盖） |

### 本轮重构（v3 — 消除 1,600 行胶水代码）

**删除的 Rust 独有文件（3 个）：**
- ~~`keys.rs`（737 行）~~ → 用 `crossterm::event::KeyEvent` 直接替代，消除按键解析胶水
- ~~`native_modifiers.rs`（102 行）~~ → 用 crossterm 原生修饰键支持
- ~~`stdin_buffer.rs`（520 行）~~ → 用 crossterm `event-stream` feature 替代，消除输入缓冲胶水

**简化的文件（1 个）：**
- `fuzzy.rs` — 从 ~700 行自实现模糊匹配 → 依赖 `fuzzy-matcher` crate（~160 行）

**修复对齐（4 轮）：**
- 输入双字符和光标偏移 bug
- Markdown 渲染修复：间距/加粗/链接/代码块/表格/引用样式
- `wrap_text_with_ansi` 丢失 ESC 字符修复
- 内容截断修复
- 补齐 OverlayHandle / InputListener / matches_key_str / hyperlink 等功能

**测试变化：**
- 测试数从 238 → **191**（移除模块及其测试被删除；保留模块的测试增量改善）
- 所有 191 个测试 ✅ 全部通过

**统计变化：**
- 文件数：27 → **24**（-3）
- 行数：9,010 → **8,384**（-626）
- 完成度：~95%（功能完整性不变 — Rust 化精简而非功能缺失）

### P0 阻塞项

1. ~~**Editor 组件**（~1500 行）— 核心交互组件完全缺失~~ ✅ 已实现（1,879 行）
2. **Diff 渲染管线** — ratatui 全屏重绘 vs TS 行级增量 diff（优化项，非阻塞）
3. ~~**Markdown 组件**（~800 行）— AI 回复渲染缺失~~ ✅ 已实现（pulldown-cmark，1,261 行）
4. ~~**基础设施链** — kill-ring / undo-stack / stdin-buffer / word-navigation / fuzzy~~ ✅ 全部完成
5. ~~**剩余组件** — SettingsList / TruncatedText / Loader / CancellableLoader / Image~~ ✅ 全部完成
6. ~~**native-modifiers** — macOS 修饰键检测~~ ✅ 不再需要（crossterm 原生支持）

---

## 五、阻塞依赖链

```
pi-ai providers（Anthropic ✅ / OpenAI ✅ / DeepSeek ✅ / xAI ✅ / 其他 ❌）
  ↓ 部分解除
pi-agent-core generate_summary / generate_branch_summary（真实 LLM 调用 ✅）
  ↓
pi-agent-core compaction pipeline
  ↓
pi-coding-agent compaction / 会话压缩
```

---

## 六、实施顺序

### pi-ai（底层，被其他 crate 依赖）

```
✅ 1. 实现 providers/anthropic.rs — 完整 SSE streaming
✅ 2. 实现 providers/openai.rs — completions
✅ 3. 实现 register-builtins — 自动注册
✅ 4. 补全 models_generated.rs — 12 provider / 35+ 模型
✅ 5. 补全 utils 模块（json-parse/validation/headers）
✅ 6. 新增 provider: deepseek / xai / simple_options
   7. 逐个补全其他 provider（mistral/google-native/bedrock/vertex 等）
   8. Images 功能
```

### pi-agent-core（依赖 pi-ai）

```
✅ 1. 实现 compaction 的 LLM 调用（替换 generate_summary/generate_branch_summary 桩）
✅ 2. 补全 prompt_templates — loadPromptTemplates / frontmatter 解析
✅ 3. 实现 AgentHarness 编排循环
   4. 补全 skills — 递归加载 / gitignore / frontmatter
```

### pi-tui（独立 crate）

```
✅ 1. 基础设施 — kill-ring + undo-stack + stdin-buffer + word-navigation + fuzzy
✅ 2. 补全 Input — kill-ring / undo / paste / grapheme
✅ 3. 补全 SelectList — callbacks / wrapping / layout
✅ 4. Editor 组件（核心交互，最复杂）
✅ 5. Markdown 组件（AI 回复渲染）
✅ 6. Autocomplete
✅ 7. Rust 化精简 — 用 crossterm 替代 keys/native-modifiers/stdin-buffer
   8. Diff 渲染管线优化
```

### pi-coding-agent（最高层，依赖上面三个）

```
✅ 1. Edit diff 引擎（最大功能缺口）
✅ 2. File mutation queue
   3. Prompt 模板 $1/$@/${@:N} 系统
   4. Bash 超时 + 流式输出 + 进程树管理
   5. Grep/Find 改为 rg/fd 二进制
   6. 补全 Session/Settings/Model registry
   7. Compaction pipeline
   8. Agent session 事件系统 + 自动重试
   9. Extensions 运行时
  10. CLI + Modes + Utils
```
