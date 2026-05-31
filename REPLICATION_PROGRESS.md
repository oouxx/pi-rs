# pi-rs 全量复刻进度报告

> 对照 TypeScript 源码逐文件比对  
> 更新日期：2026-05-31

---

## 总览

| Crate | TS 源仓库 | 文件数 | 代码行数 | 测试数 | 编译 | 完成度 |
|-------|-----------|:---:|:---:|:---:|:---:|:---:|
| pi-agent-core | `packages/agent` | 29 | 8,933 | 141/141 ✅ | ✅ | ~80% |
| pi-coding-agent | `packages/coding-agent` | 33 | 7,582 | 88 ❌ | ❌ | ~35% |
| pi-ai | `packages/ai` | 18 | 4,820 | 120/120 ✅ | ✅ | ~45% |
| pi-tui | `packages/tui` | 12 | 3,202 | 96/96 ✅ | ✅ | ~30% |
| **合计** | | **92** | **24,537** | **357** | | |

pi-coding-agent 编译错误：`core::skills::Skill` 缺少 `instructions` 字段（2 处）。

---

## 一、pi-agent-core（29 文件 / 8,933 行 / 完成度 ~80%）

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
| `agent-loop.ts` | `agent_loop.rs` | ~90% | — |
| `agent.ts` | `agent.rs` | ~85% | — |
| `proxy.ts` | `proxy.rs` | ~85% | FileSystemStart/FileSystemDelta/FileSystemEnd 事件变体 |
| `harness/agent-harness.ts` | `harness/agent_harness.rs` | ~55% | 编排循环不完整 |
| `harness/prompt-templates.ts` | `harness/prompt_templates.rs` | ~75% | `loadPromptTemplates()`、frontmatter 解析缺失 |
| `harness/skills.ts` | `harness/skill_loader.rs` + `skills.rs` | ~60% | `formatSkillInvocation()`、`loadSourcedSkills()`、gitignore 支持 |
| `harness/types.ts` | `harness/types.rs` | ~85% | `StreamOptionsPatch`、`ChainSummary`、`FileSystem` 接口 |
| `harness/compaction/compaction.ts` | `harness/compaction/compaction.rs` | ~75% | **`generate_summary()` 是桩代码**（返回占位文本） |
| `harness/compaction/branch-summarization.ts` | `harness/compaction/branch_summarization.rs` | ~60% | **`generate_branch_summary()` 是桩代码** |
| `harness/utils/shell-output.ts` | `harness/utils/shell_output.rs` | ~70% | 临时文件管理不完整 |

### 未复刻（2 个文件）

| TypeScript | 说明 |
|------------|------|
| `node.ts` | Node.js 入口（Rust crate 自身即入口） |
| `harness/session/uuid.ts` | UUIDv7（Rust 用 uuid crate 的 v4） |

### agent_loop.rs 实现状况（~90%）

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

### agent.rs 实现状况（~85%）

- `process()` / `continue_run()` — 连线到 agent_loop
- `steer()` / `follow_up()` — PendingMessageQueue（All / OneAtATime 模式）
- `abort()` — CancellationToken 取消
- `create_event_sink()` — 事件分发 + AgentState 同步
- 全部 hooks 已连线

### P0 阻塞项

1. **`generate_summary()`**（compaction.rs:304）— 返回占位文本，不调用 LLM
2. **`generate_branch_summary()`**（branch_summarization.rs:117）— 返回占位文本，不调用 LLM
3. **`loadPromptTemplates()`** — 不存在
4. **AgentHarness 编排循环** — 不完整

---

## 二、pi-coding-agent（33 文件 / 7,582 行 / 完成度 ~35%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/coding-agent`

### 类型指标

struct: 97 | enum: 9 | trait: 6 | pub fn: 170 | impl block: 36

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
| `core/model-registry.ts` | `core/model_registry.rs` | ~50% | OAuth 支持、AuthStorage、resolve-config-value |
| `core/model-resolver.ts` | `core/model_resolver.rs` | ~50% | defaultModelPerProvider（25+ provider）、别名检测 |
| `core/system-prompt.ts` | `core/system_prompt.rs` | ~85% | — |
| `core/skills.ts` | `core/skills.rs` | ~40% | frontmatter 解析、名称/描述验证、gitignore 感知 |
| `core/prompt-templates.ts` | `core/prompt_templates.rs` | ~30% | `$1`/`$@`/`${@:N}` 系统缺失 |
| `core/resource-loader.ts` | `core/resource_loader.rs` | ~40% | 主题加载、祖先目录扫描、PackageManager 集成 |
| `core/extensions/` | `core/extensions/` | ~35% | 无扩展运行时、无事件钩子、无 worker |
| `core/compaction/` | `core/compaction.rs` | ~50% | `compact()` 主函数、token 计数、分支摘要 |
| `core/agent-session.ts` | `core/agent_session.rs` | ~60% | 事件系统（10+ 事件类型）、自动重试、压缩集成 |
| `core/bash-executor.ts` | `core/bash_executor.rs` | ~40% | 无流式输出、无 output buffer 管理、无 sanitizeBinaryOutput |
| — | `core/sdk.rs` | — | Rust 独有：SDK 集成层（DI 容器），183 行 |

### 工具模块

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `core/tools/index.ts` | `core/tools/mod.rs` | ~50% | ToolDefinition 层、file-mutation-queue、output-accumulator |
| `core/tools/bash.ts` | `core/tools/bash.rs` | ~35% | 超时参数被忽略、无 spawn hook、无进程树管理、无流式输出 |
| `core/tools/edit.ts` | `core/tools/edit.rs` | ~30% | **无 diff 计算**（edit-diff.ts 完全缺失）、无模糊匹配、无 Unicode 标准化 |
| `core/tools/read.ts` | `core/tools/read.rs` | ~40% | 无图片处理、无语法高亮、无 macOS 路径变体 |
| `core/tools/write.ts` | `core/tools/write.rs` | ~35% | 无文件变异队列、无语法高亮、无增量缓存 |
| `core/tools/grep.ts` | `core/tools/grep.rs` | ~30% | 纯 Rust regex vs ripgrep 二进制（架构不同，无 gitignore 感知） |
| `core/tools/find.ts` | `core/tools/find.rs` | ~30% | 纯 Rust glob vs fd 二进制（架构不同，无 gitignore 感知） |
| `core/tools/ls.ts` | `core/tools/ls.rs` | ~45% | 无大小写不敏感排序、无 stat 逐项检查 |
| `core/tools/truncate.ts` | `core/tools/truncate.rs` | ~80% | DEFAULT_MAX_BYTES 256KB（TS 50KB，5 倍差异） |
| `core/tools/path-utils.ts` | `core/tools/path_utils.rs` | ~55% | macOS 专用变体（NFD/screenshot/curly quotes） |
| `core/tools/render-utils.ts` | `core/tools/render_utils.rs` | ~40% | shortenPath、linkPath、图片块处理 |

### 完全未复刻（24+ 个 TS 文件）

| 模块 | 用途 |
|------|------|
| `core/agent-session-runtime.ts` | 会话运行时（流式/重试/压缩编排） |
| `core/agent-session-services.ts` | DI 容器 |
| `core/auth-guidance.ts` | 认证引导消息 |
| `core/auth-storage.ts` | 加密认证存储 |
| `core/defaults.ts` | 默认 thinking level |
| `core/exec.ts` | 进程执行抽象 |
| `core/export-html/` (6 文件) | HTML 会话导出 |
| `core/http-dispatcher.ts` | HTTP 请求分发 |
| `core/keybindings.ts` | 键盘快捷键 |
| `core/package-manager.ts` | 扩展/skill 包管理 |
| `core/source-info.ts` | 资源源元数据 |
| `core/tools/edit-diff.ts` | **Diff 计算引擎**（最大功能缺口） |
| `core/tools/file-mutation-queue.ts` | 文件变异序列化队列 |
| `core/tools/output-accumulator.ts` | 流式输出累积器 |
| `core/tools/tool-definition-wrapper.ts` | AgentTool → ToolDefinition 包装 |
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

1. **Edit diff 引擎完全缺失** — `string.replace()` 替代了模糊匹配+diff+归一化引擎
2. **Prompt 模板 `$1`/`$@`/`${@:N}` 系统缺失** — 与 pi-agent-core 的 prompt_templates.rs 不一致
3. **File mutation queue 缺失** — 无并发文件操作保护
4. **Bash 超时参数被忽略** — 接受但不处理
5. **Grep/Find 纯 Rust 实现** — 原版用 rg/fd 二进制，无 gitignore 感知

---

## 三、pi-ai（16 文件 / 3,782 行 / 完成度 ~40%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/ai`

### 类型指标

struct: 22 | enum: 9 | trait: 0 | pub fn: 35 | impl block: 8

### 模块状态

| TypeScript | Rust | 覆盖率 | 说明 |
|------------|------|--------|------|
| `types.ts` | `types.rs` | ~90% | 1,041 行，35+ public types |
| `models.ts` | `models.rs` | ~100% | 314 行，get_model / calculate_cost / thinking levels |
| `models.generated.ts` | `models_generated.rs` | ~40% | 332 行，7 provider / 33 模型条目 |
| `api-registry.ts` | `api_registry.rs` | ~80% | 注册/查找/注销机制完整；get_api_provider 不能真正 clone |
| `stream.ts` | `stream.rs` | ~100% | stream / complete / streamSimple / completeSimple |
| `env-api-keys.ts` | `env_api_keys.rs` | ~55% | 25 provider → env var 映射 |
| `utils/event-stream.ts` | `utils/event_stream.rs` | ~60% | pull-based vs push-based 架构差异 |
| `utils/diagnostics.ts` | `utils/diagnostics.rs` | ~30% | 数据模型与 TS 不一致 |
| **`providers/anthropic.ts`** | **`providers/anthropic.rs`** | **~60%** | SSE streaming + 消息转换 + 工具转换 + stop reason + 测试（20 tests） |
| **`providers/openai-completions.ts`** | **`providers/openai.rs`** | **~40%** | SSE streaming + 消息转换 + 工具转换 + 测试（15 tests） |
| **`providers/register-builtins.ts`** | **`providers/register_builtins.rs`** | **~60%** | 2 API 注册 + reset_api_providers + 测试（4 tests） |
| **—** | **`utils/sse.rs`** | **~100%** | SSE 解析器（共享），23 tests，支持 Anthropic 和 OpenAI 两种 SSE 格式 |

### 本轮新增（2026-05-31 第二次更新）

- **SSE 解析器**（`utils/sse.rs`, 200+ 行, 23 tests）— 完整的 SSE 协议解析，独立于具体 provider
- **Anthropic provider**（`providers/anthropic.rs`, 670+ 行, 20 tests）— 消息/工具转换、SSE 事件处理、streamAnthropic / streamSimpleAnthropic
- **OpenAI provider**（`providers/openai.rs`, 540+ 行, 15 tests）— 消息/工具转换、OpenAI SSE 格式解析、streamOpenAI / streamSimpleOpenAI
- **register_builtins**（`providers/register_builtins.rs`, 80+ 行, 4 tests）— 注册 anthropic-messages 和 openai-completions

### 完全缺失（25+ 个 TS 文件）

| 类别 | 数量 | 说明 |
|------|------|------|
| Provider 实现 | ~13 | mistral / google / bedrock / azure / vertex / codex / copilot 等 |
| Utils | 6 | json-parse / overflow / validation / typebox-helpers / headers / session-resources |
| Images 功能 | 5 | images/models / api-registry / image-models.generated + providers/images |
| 其他 | 3 | index / cli / oauth |

### P0 阻塞项

1. ~~Provider 实现全是空壳~~ ✅ Anthropic 和 OpenAI 已实现
2. **13+ provider 未复刻** — mistral / google / bedrock / vertex / codex / copilot 等
3. ~~register-builtins 缺失~~ ✅ 已实现（2 API）
4. **models_generated 不完整** — 仅 7 provider，缺 15+ provider

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
   4. 补全 models_generated.rs — 15+ provider 模型数据
   5. 逐个补全其他 provider（mistral/google/bedrock/vertex 等）
   6. 补全 utils 模块（json-parse/overflow/validation/headers）
   7. Images 功能
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
1. Edit diff 引擎（最大功能缺口）
2. Prompt 模板 $1/$@/${@:N} 系统
3. File mutation queue
4. Bash 超时 + 流式输出 + 进程树管理
5. Grep/Find 改为 rg/fd 二进制
6. 补全 Session/Settings/Model registry
7. Compaction pipeline
8. Agent session 事件系统 + 自动重试
9. Extensions 运行时
10. CLI + Modes + Utils
```
