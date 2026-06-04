# pi-rs 全量复刻进度报告

> 对照 TypeScript 源码逐文件比对  
> 更新日期：2026-06-05

---

## 总览

| Crate | TS 源仓库 | 文件数 | 代码行数 | 测试数 | 编译 | 完成度 |
|-------|-----------|:---:|:---:|:---:|:---:|:---:|
| pi-agent-core | `packages/agent` | 32 | 12,549 | 197/197 ✅ | ✅ | ~95% |
| pi-coding-agent | `packages/coding-agent` | 53 | 11,620 | 208/208 ✅ | ✅ | ~55% |
| pi-ai | `packages/ai` | 25 | 6,220 | 167/167 ✅ | ✅ | ~60% |
| pi-tui | `packages/tui` | 12 | 3,202 | 96/96 ✅ | ✅ | ~30% |
| **合计** | | **122** | **33,591** | **668** | | |

---

## 一、pi-agent-core（29 文件 / 10,300 行 / 完成度 ~95%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/agent`

### 类型指标

struct: 74 | enum: 26 | trait: 3 | pub fn: 133 | impl block: 30

### 完整复刻（14 个文件）

| TypeScript | Rust | 说明 |
|------------|------|------|
| `index.ts` | `lib.rs` | barrel 导出 |
| `types.ts` | `types.rs` | AgentMessage / AgentEvent / AgentTool / AgentState 等全部类型 |
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
| `core/extensions/` | `core/extensions/` | ~35% | 无扩展运行时、无事件钩子、无 worker |
| `core/compaction/` | `core/compaction.rs` | ~50% | `compact()` 主函数、token 计数、分支摘要 |
| `core/agent-session.ts` | `core/agent_session.rs` | ~60% | 事件系统（10+ 事件类型）、自动重试、压缩集成 |
| `core/agent-session-runtime.ts` | `core/agent_session_runtime.rs` | **NEW** (~85%) | 会话运行时（流式/重试/压缩编排） |
| `core/agent-session-services.ts` | `core/agent_session_services.rs` | **NEW** (~90%) | DI 容器、服务注册 |
| `core/auth-guidance.ts` | `core/auth_guidance.rs` | **NEW** (~90%) | 认证引导消息 |
| `core/auth-storage.ts` | `core/auth_storage.rs` | **NEW** (~90%) | 加密认证存储 |
| `core/defaults.ts` | `core/defaults.rs` | **NEW** (~90%) | 默认 thinking level |
| `core/exec.ts` | `core/exec.rs` | **NEW** (~95%) | 进程执行抽象（含超时/取消） |
| `core/http-dispatcher.ts` | `core/http_dispatcher.rs` | **NEW** (~90%) | HTTP 请求分发 |
| `core/source-info.ts` | `core/source_info.rs` | **NEW** (~90%) | 资源源元数据 |
| `core/bash-executor.ts` | `core/bash_executor.rs` | ~40% | 无流式输出、无 output buffer 管理、无 sanitizeBinaryOutput |
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
| `core/tools/edit-diff.ts` | `core/tools/edit_diff.rs` | **NEW** (~90%) | Diff 计算引擎（替换 `string.replace()`） |
| `core/tools/file-mutation-queue.ts` | `core/tools/file_mutation_queue.rs` | **NEW** (~95%) | 文件变异序列化队列 |
| `core/tools/output-accumulator.ts` | `core/tools/output_accumulator.rs` | **NEW** (~95%) | 流式输出累积器 |
| `core/tools/tool-definition-wrapper.ts` | `core/tools/tool_definition_wrapper.rs` | **NEW** (~90%) | AgentTool → ToolDefinition 包装 |
| `core/tools/read.ts` | `core/tools/read.rs` | ~40% | 无图片处理、无语法高亮、无 macOS 路径变体 |
| `core/tools/write.ts` | `core/tools/write.rs` | ~50% | 无语法高亮、无增量缓存（file-mutation-queue 已就位） |
| `core/tools/grep.ts` | `core/tools/grep.rs` | ~30% | 纯 Rust regex vs ripgrep 二进制（架构不同，无 gitignore 感知） |
| `core/tools/find.ts` | `core/tools/find.rs` | ~30% | 纯 Rust glob vs fd 二进制（架构不同，无 gitignore 感知） |
| `core/tools/ls.ts` | `core/tools/ls.rs` | ~45% | 无大小写不敏感排序、无 stat 逐项检查 |
| `core/tools/truncate.ts` | `core/tools/truncate.rs` | ~80% | DEFAULT_MAX_BYTES 256KB（TS 50KB，5 倍差异） |
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

### 本轮更新（2026-06-05）

本次提交新增 **19 个 Rust 源文件**（3,964 行），复刻了 12+ 个关键 TS 模块：

**基础设施层（10 个新模块）：**
- `auth_guidance` / `auth_storage` — 认证引导和加密存储，支撑 model-registry OAuth
- `defaults` — 默认 thinking level
- `exec` — 进程执行抽象（超时/取消支持，25 tests ✅）
- `http_dispatcher` — HTTP 请求分发
- `output_guard` — 输出保护器
- `provider_attribution` / `provider_display_names` — 提供者归属和显示名
- `resolve_config_value` — 配置值解析
- `session_cwd` — 会话工作目录管理
- `source_info` — 资源源元数据
- `telemetry` / `timings` — 遥测事件收集和性能计时器（32 tests ✅）
- `agent_session_runtime` / `agent_session_services` — 会话运行时和 DI 容器

**工具层（4 个新模块）：**
- `edit_diff` — **Diff 计算引擎**（最大功能缺口已填补）
- `file_mutation_queue` — 文件变异序列化队列（8 tests ✅）
- `output_accumulator` — 流式输出累积器（8 tests ✅）
- `tool_definition_wrapper` — AgentTool → ToolDefinition 包装（8 tests ✅）

**依赖更新：** 新增 `pi-ai`、`similar`（diff 引擎）、`unicode-normalization`、`url`

**状态变化：**
- 编译错误 **已消除**（`Skill` 缺少 `instructions` 字段）→ 编译通过，仅 9 个 warnings
- 测试从 90 → **208**（+118）全部通过
- 文件从 33 → **53**（+20），行数从 7,582 → **11,620**（+4,038）
- 完成度从 ~35% → **~55%**

### P0 阻塞项

1. ~~**Edit diff 引擎完全缺失** — `string.replace()` 替代~~ ✅ 已实现 `edit_diff.rs`
2. ~~**File mutation queue 缺失** — 无并发文件操作保护~~ ✅ 已实现 `file_mutation_queue.rs`
3. **Prompt 模板 `$1`/`$@`/`${@:N}` 系统缺失** — 与 pi-agent-core 的 prompt_templates.rs 不一致
4. **Bash 超时参数被忽略** — 接受但不处理
5. **Grep/Find 纯 Rust 实现** — 原版用 rg/fd 二进制，无 gitignore 感知

---

## 三、pi-ai（23 文件 / 5,783 行 / 完成度 ~58%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/ai`

### 类型指标

struct: 26 | enum: 10 | trait: 0 | pub fn: 42 | impl block: 9

### 模块状态

| TypeScript | Rust | 覆盖率 | 说明 |
|------------|------|--------|------|
| `types.ts` | `types.rs` | ~90% | 1,041 行，35+ public types |
| `models.ts` | `models.rs` | ~100% | get_model / calculate_cost / thinking levels；RwLock 运行时注册表 |
| `models.generated.ts` | **`build.rs`** | **~90%** | **已删除手写 models_generated.rs**。改用 build.rs 在编译期从 OpenRouter API + models.dev 自动拉取并生成模型数据（255 个 OpenRouter 模型 + 203 个 models.dev 模型，14 个 provider） |
| `api-registry.ts` | `api_registry.rs` | ~80% | 注册/查找/注销机制完整；get_api_provider 不能真正 clone |
| `stream.ts` | `stream.rs` | ~100% | stream / complete / streamSimple / completeSimple |
| `env-api-keys.ts` | `env_api_keys.rs` | ~55% | 25 provider → env var 映射 |
| `utils/event-stream.ts` | `utils/event_stream.rs` | ~60% | pull-based vs push-based 架构差异 |
| `utils/diagnostics.ts` | `utils/diagnostics.rs` | ~30% | 数据模型与 TS 不一致 |
| `utils/json-parse.ts` | `utils/json_parse.rs` | ~70% | JSON repair / clean_partial / parse_streaming_json（15 tests） |
| `utils/validation.ts` | `utils/validation.rs` | ~60% | 工具调用参数验证 + JSON Schema validate（10 tests） |
| `utils/overflow.ts` | `utils/overflow.rs` | **~100%** | 上下文溢出检测：3 种检测策略 + 20+ provider 模式 + 25 tests |
| `utils/typebox-helpers.ts` | `utils/typebox_helpers.rs` | **~100%** | `string_enum()` JSON Schema 辅助函数 + 5 tests |
| `session-resources.ts` | `utils/session_resources.rs` | **~100%** | 会话资源清理注册/反注册/批量清理 + 9 tests |
| **—** | **`utils/headers.rs`** | **~100%** | HeaderMap → HashMap 转换（2 tests，Rust 独有） |
| **`providers/anthropic.ts`** | **`providers/anthropic.rs`** | **~60%** | SSE streaming + 消息转换 + 工具转换 + stop reason + 测试（20 tests） |
| **`providers/openai-completions.ts`** | **`providers/openai.rs`** | **~40%** | SSE streaming + 消息转换 + 工具转换 + 测试（15 tests） |
| **`providers/register-builtins.ts`** | **`providers/register_builtins.rs`** | **~90%** | 注册 API provider + 编译期加载生成模型数据；**已移除 ~800 行硬编码模型** |
| **—** | **`build.rs`** | **~80%** | 编译期模型生成。port 原版 generate-models.ts + generate-image-models.ts 核心逻辑：fetch OpenRouter / models.dev API → 处理 pricing 转换 → 生成 JSON 到 OUT_DIR |
| **—** | **`utils/sse.rs`** | **~100%** | SSE 解析器（共享），23 tests，支持 Anthropic 和 OpenAI 两种 SSE 格式 |

### 本轮更新（2026-06-02）

- **build.rs 替代 models_generated.rs** — 不再手写维护模型数据。编译期自动从 OpenRouter API + models.dev 拉取，255 个工具模型 + 203 个 models.dev 模型
- **register_builtins 瘦身** — 移除 ~800 行硬编码模型数据，改用 `include_str!(concat!(env!("OUT_DIR"), "/models_generated.json"))` 编译期加载
- **模型注册表改为运行时** — `models.rs` 用 `RwLock<HashMap>` 替代静态 `LazyLock`，支持程序化注册（`register_model()`）
- **types.rs 增加 Default** — `OpenAICompletionsCompat` 现在可 `#[derive(Default)]`
- **删除的文件** — `models_generated.rs`（586 行）、`models.json`（内嵌 JSON）、`fetcher.rs`（运行时拉取）
- **补全全部剩余 Utils** — `overflow.rs`（25 tests, 25 个正则溢出检测）、`typebox_helpers.rs`（5 tests, JSON Schema string_enum）、`session_resources.rs`（9 tests, 会话资源注册/清理）

### 完全缺失（19+ 个 TS 文件）

| 类别 | 数量 | 说明 |
|------|------|------|
| Provider 实现 | ~13 | mistral / google-native / bedrock / azure / vertex / codex / copilot 等 |
| ~~Utils~~ | ~~3~~ | ~~overflow / typebox-helpers / session-resources~~ ✅ 已完成 |
| Images 功能 | 5 | images/models / api-registry / image-models.generated + providers/images |
| 其他 | 3 | index / cli / oauth |

### P0 阻塞项

1. ~~Provider 实现全是空壳~~ ✅ Anthropic 和 OpenAI 已实现
2. **13+ provider 未复刻** — mistral / google-native / bedrock / vertex / codex / copilot 等
3. ~~register-builtins 缺失~~ ✅ 已实现（3 API 注册）
4. ~~models_generated 手写维护~~ ✅ 已用 build.rs 替代，编译期自动拉取
5. ~~Utils 模块缺失 3+~~ ✅ overflow / typebox-helpers / session-resources 已完成

---

## 四、pi-tui（12 文件 / 3,202 行 / 完成度 ~30%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/tui`

### 类型指标

struct: 21 | enum: 3 | trait: 2 | pub fn: 81 | impl block: 23

### 模块状态

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `components/spacer.ts` | `components/spacer.rs` | ~100% | 唯一 100% 复刻的组件 |
| `components/text.ts` | `components/text.rs` | ~85% | 缓存有 bug（`&self` 不可变导致无法写缓存） |
| `keybindings.ts` | `keybindings.rs` | ~80% | 434 行，KeybindingsManager / 冲突检测 / 覆盖 |
| `tui.ts` | `tui.rs` | ~60% | 671 行，Container / Component trait / 渲染管线；无 diff 渲染 |
| `keys.ts` | `keys.rs` | ~55% | 530 行，Key / KeyEvent / KeyModifiers；缺 modifyOtherKeys / Kitty flag 4 |
| `components/select-list.ts` | `components/select_list.rs` | ~65% | 398 行，过滤/选择/主题/滚动；缺 callbacks/wrapping |
| `components/box.ts` | `components/box_component.rs` | ~60% | 缺 removeChild、functional background、有效缓存 |
| `components/input.ts` | `components/input.rs` | ~50% | 410 行，光标/插入/删除/单词导航；缺 kill-ring/undo/paste/grapheme |
| `utils.ts` | `utils.rs` | ~50% | 338 行，visible_width/strip_ansi/wrap_text；缺 grapheme 分词 |
| `terminal.ts` | `terminal.rs` | ~40% | 197 行，缺 Kitty 协商/StdinBuffer/paste/Apple Terminal 检测 |

### 完全缺失（14 个模块）

| 模块 | 行数 | 重要性 | 说明 |
|------|------|--------|------|
| `components/editor.ts` | ~1500 | **核心** | 多行编辑器、选区、kill-ring、undo、autocomplete、历史导航 |
| `components/markdown.ts` | ~800 | **核心** | Markdown 渲染（标题/代码块/列表/表格/行内格式） |
| `autocomplete.ts` | ~439 | 高 | 斜杠命令 + 文件路径自动补全 |
| `fuzzy.ts` | ~120 | 中 | 模糊匹配 |
| `kill-ring.ts` | ~45 | 中 | Emacs kill-ring（Editor 前置依赖） |
| `undo-stack.ts` | ~30 | 中 | 撤销栈（Editor 前置依赖） |
| `stdin-buffer.ts` | ~300 | 中 | 输入缓冲与粘性拆包 |
| `terminal-image.ts` | ~400 | 中 | Kitty/iTerm2 图片协议 |
| `word-navigation.ts` | ~135 | 中 | 单词级导航 |
| `components/settings-list.ts` | ~240 | 低 | 可搜索设置 UI |
| `components/truncated-text.ts` | ~70 | 低 | 单行截断 |
| `components/image.ts` | ~140 | 低 | 终端图片渲染 |
| `components/loader.ts` | ~115 | 低 | 动画 spinner |
| `components/cancellable-loader.ts` | ~45 | 低 | 可取消 spinner |
| `native-modifiers.ts` | ~80 | 低 | macOS 修饰键检测 |
| `editor-component.ts` | ~50 | 低 | Editor 插件接口 |

### P0 阻塞项

1. **Editor 组件**（~1500 行）— 核心交互组件完全缺失
2. **Diff 渲染管线** — ratatui 全屏重绘 vs TS 行级增量 diff
3. **Markdown 组件**（~800 行）— AI 回复渲染缺失
4. **基础设施链** — kill-ring → undo-stack → stdin-buffer → word-navigation → fuzzy，5 个模块需要先实现才能做 Editor

---

## 五、阻塞依赖链

```
pi-ai providers（Anthropic ✅ / OpenAI ✅ / 其他 ❌）
  ↓ 部分解除
pi-agent-core generate_summary / generate_branch_summary（桩代码）
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
   6. 逐个补全其他 provider（mistral/google-native/bedrock/vertex 等）
   7. 剩余 utils（overflow/typebox-helpers/session-resources）
   8. Images 功能
```

### pi-agent-core（依赖 pi-ai）

```
1. 实现 compaction 的 LLM 调用（替换 generate_summary/generate_branch_summary 桩）
2. 补全 prompt_templates — loadPromptTemplates / frontmatter 解析
3. 实现 AgentHarness 编排循环
4. 补全 skills — 递归加载 / gitignore / frontmatter
```

### pi-tui（独立 crate）

```
1. 基础设施 — kill-ring + undo-stack + stdin-buffer + word-navigation + fuzzy
2. 补全 Input — kill-ring / undo / paste / grapheme
3. 补全 SelectList — callbacks / wrapping / layout
4. Editor 组件（核心交互，最复杂）
5. Markdown 组件（AI 回复渲染）
6. Autocomplete
7. Diff 渲染管线优化
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
