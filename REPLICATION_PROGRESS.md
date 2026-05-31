# pi-rs 全量复刻进度报告

> 对照 CLAUDE.md 规范，以 TypeScript 源码为基准逐文件比对  
> 更新日期：2026-05-31

---

## 总览

| Crate | TS 源仓库 | 整体完成度 | 测试数 |
|-------|-----------|-----------|--------|
| **pi-agent-core** | `packages/agent` | **~72%** | 0 |
| **pi-coding-agent** | `packages/coding-agent` | **~25-30%** | 40 |
| **pi-ai** | `packages/ai` | **~15-20%** | 20 |
| **pi-tui** | `packages/tui` | **~22%** | 35 |

---

## 一、pi-agent-core 复刻进度（整体 ~72%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/agent`

### 完整复刻（14 个文件）

| TypeScript | Rust | 说明 |
|------------|------|------|
| `index.ts` | `lib.rs` | barrel 导出 |
| `types.ts` | `types.rs` | 全部类型（AgentMessage/AgentEvent/AgentTool/AgentState 等） |
| `harness/messages.ts` | `harness/messages.rs` | 消息转换、摘要常量 |
| `harness/system-prompt.ts` | `harness/system_prompt.rs` | skill XML 格式化 |
| `harness/compaction/utils.ts` | `harness/compaction/utils.rs` | 文件操作提取 |
| `harness/env/nodejs.ts` | `harness/env/nodejs.rs` | NodeExecutionEnv 全部方法 |
| `harness/session/session.ts` | `harness/types.rs`（合并） | Session 结构体及方法 |
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
| `agent.ts` | `agent.rs` | ~65% | 未集成多轮循环、无流式事件处理、工具执行是桩代码 |
| `agent-loop.ts` | `agent_loop.rs` | **~30%** | **最大缺口**：只调一次 API 就退出；无多轮迭代、无顺序/并行工具执行、`run_agent_loop_continue` 是空函数、before/after tool hooks 未调用 |
| `proxy.ts` | `proxy.rs` | ~85% | 缺失 FileSystemStart/FileSystemDelta/FileSystemEnd 事件变体 |
| `harness/agent-harness.ts` | `harness/agent_harness.rs` | ~50% | 所有零件都在但无编排循环；prompt() 不实际运行 |
| `harness/prompt-templates.ts` | `harness/prompt_templates.rs` | ~30% | 缺失 `loadPromptTemplates()`、`loadSourcedPromptTemplates()`、frontmatter 解析；`substitute_args` 的 regex 是桩代码 |
| `harness/skills.ts` | `harness/skill_loader.rs` + `skills.rs` | ~60% | 缺失 `formatSkillInvocation()`、`loadSourcedSkills()`、gitignore 支持、递归目录遍历、仅加载 SKILL.md |
| `harness/types.ts` | `harness/types.rs` | ~85% | 缺失 `StreamOptionsPatch`、`ChainSummary` 变体、`FileSystem` 接口 |
| `harness/compaction/compaction.ts` | `harness/compaction/compaction.rs` | ~80% | **`generate_summary()` 是桩代码**（返回占位文本而非调用 LLM） |
| `harness/compaction/branch-summarization.ts` | `harness/compaction/branch_summarization.rs` | ~65% | **`generate_branch_summary()` 是桩代码**；缺失 `CollectEntriesResult`、`BranchSummaryDetails` |
| `harness/utils/shell-output.ts` | `harness/utils/shell_output.rs` | ~70% | 缺失完整输出到临时文件的管理 |

### 未复刻（2 个文件）

| TypeScript | 说明 |
|------------|------|
| `node.ts` | Node.js 入口 re-export（Rust crate 自身就是入口） |
| `harness/session/uuid.ts` | UUIDv7；Rust 使用 `uuid` crate 的 UUIDv4 |

### P0 关键缺口（阻塞基础功能）

1. **Agent 循环是骨架** — `run_agent_loop` 只调用一次 stream_fn 就退出，需要完整的多轮迭代
2. **工具执行是桩代码** — `process_tool_calls_in_loop` 生成占位结果，`AgentTool.execute` 从未被调用
3. **generate_summary / generate_branch_summary 是桩代码** — 压缩和分支摘要功能不工作
4. **Prompt 模板加载缺失** — `loadPromptTemplates` 不存在，模板系统不可用
5. **AgentHarness 无运行循环** — 所有零件就位但没有编排

---

## 二、pi-coding-agent 复刻进度（整体 ~25-30%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/coding-agent`

### 已有 Rust 对应文件的模块

| TypeScript | Rust | 状态 | 覆盖率 | 关键缺失 |
|------------|------|------|--------|----------|
| `config.ts` | `config.rs` | 部分 | ~25% | 缺失包检测、安装方式、自更新、全局包路径 |
| `core/event-bus.ts` | `core/event_bus.rs` | 基本 | ~80% | **unsubscribe 是空操作**（内存泄漏） |
| `core/diagnostics.ts` | `core/diagnostics.rs` | 基本 | ~90% | 缺失 `error` 诊断类型变体 |
| `core/session-manager.ts` | `core/session_manager.rs` | 部分 | ~65% | 缺失 ReadonlySessionManager、getLatestCompactionEntry |
| `core/settings-manager.ts` | `core/settings_manager.rs` | 部分 | ~40% | **无文件锁**、缺失 10+ 个设置字段、无运行时覆盖层 |
| `core/slash-commands.ts` | `core/slash_commands.rs` | **完整** | ~95% | — |
| `core/messages.ts` | `core/messages.rs` | 基本 | ~85% | 缺失工厂函数 |
| `core/context-usage.ts` | `core/context_usage.rs` | **完整** | ~95% | Rust 独有整合模块 |
| `core/model-registry.ts` | `core/model_registry.rs` | 部分 | ~50% | 缺失 OAuth 支持、resolve-config-value 集成、AuthStorage |
| `core/model-resolver.ts` | `core/model_resolver.rs` | 部分 | ~50% | 缺失 defaultModelPerProvider（25+ provider）、别名检测 |
| `core/system-prompt.ts` | `core/system_prompt.rs` | 基本 | ~85% | — |
| `core/skills.ts` | `core/skills.rs` | 部分 | ~40% | 缺失 frontmatter 解析、名称/描述验证、gitignore 感知扫描 |
| `core/prompt-templates.ts` | `core/prompt_templates.rs` | **极少** | ~30% | **无参数替换**（`$1`/`$@`/`${@:N}` 系统完全缺失） |
| `core/resource-loader.ts` | `core/resource_loader.rs` | 部分 | ~40% | 缺失主题加载、祖先目录扫描、PackageManager 集成 |
| `core/extensions/` | `core/extensions/` | **极少** | ~35% | **无扩展运行时**、无事件钩子系统、无 worker 支持 |
| `core/compaction/` | `core/compaction/` | **极少** | ~30% | 缺失 `compact()` 主函数、token 计数、分支摘要生成 |
| `core/agent-session.ts` | `core/agent_session.rs` | 部分 | ~40% | 缺失事件系统（10+ 事件类型）、自动重试、压缩集成 |
| `core/bash-executor.ts` | `core/bash_executor.rs` | **极少** | ~35% | 无流式输出、无输出缓冲管理、无 sanitizeBinaryOutput |

### 工具模块

| TypeScript | Rust | 状态 | 覆盖率 | 关键缺失 |
|------------|------|------|--------|----------|
| `core/tools/index.ts` | `core/tools/mod.rs` | 部分 | ~50% | 缺失 ToolDefinition 层、file-mutation-queue、output-accumulator |
| `core/tools/bash.ts` | `core/tools/bash.rs` | **极少** | ~35% | **超时参数被忽略**、无 spawn hook、无进程树管理、无流式输出 |
| `core/tools/edit.ts` | `core/tools/edit.rs` | **极少** | **~30%** | **无 diff 计算**（edit-diff.ts 完全缺失）、无模糊匹配、无 Unicode 标准化 |
| `core/tools/read.ts` | `core/tools/read.rs` | 部分 | ~40% | 无图片处理、无语法高亮、无 macOS 路径变体 |
| `core/tools/write.ts` | `core/tools/write.rs` | **极少** | ~35% | 无文件变异队列、无语法高亮、无增量缓存 |
| `core/tools/grep.ts` | `core/tools/grep.rs` | **极少** | ~30% | **纯 Rust regex vs ripgrep 二进制**（架构不同，无 gitignore 感知） |
| `core/tools/find.ts` | `core/tools/find.rs` | **极少** | ~30% | **纯 Rust glob vs fd 二进制**（架构不同，无 gitignore 感知） |
| `core/tools/ls.ts` | `core/tools/ls.rs` | 部分 | ~45% | 无大小写不敏感排序、无 stat 逐项检查 |
| `core/tools/truncate.ts` | `core/tools/truncate.rs` | 基本 | ~80% | DEFAULT_MAX_BYTES 为 256KB（TS 50KB，5 倍差异） |
| `core/tools/path-utils.ts` | `core/tools/path_utils.rs` | 部分 | ~55% | 缺失 macOS 专用变体（NFD/screenshot/curly quotes） |
| `core/tools/render-utils.ts` | `core/tools/render_utils.rs` | **极少** | ~40% | 缺失 shortenPath、linkPath、图片块处理 |

### 完全未复刻的模块（24+ 个 TS 文件）

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
| `core/tools/edit-diff.ts` | **Diff 计算引擎**（最大的单个功能缺口） |
| `core/tools/file-mutation-queue.ts` | 文件变异序列化队列 |
| `core/tools/output-accumulator.ts` | 流式输出累积器 |
| `core/tools/tool-definition-wrapper.ts` | AgentTool → ToolDefinition 包装 |
| `cli/` (6 文件) | CLI 参数解析、配置选择、会话选择器 |
| `modes/` (7+ 文件) | 所有运行模式（interactive TUI/print/RPC） |
| `utils/` (28 文件) | 全部工具模块 |
| `bun/` (3 文件) | Bun 运行时 |

### P0 关键缺口（阻塞基础功能）

1. **Edit diff 引擎完全缺失** — 简单的 `string.replace()` 替代了完整的模糊匹配+diff+归一化引擎
2. **Prompt 模板参数替换缺失** — `$1`/`$@`/`${@:N}` 系统完全不存在
3. **File mutation queue 缺失** — 无并发文件操作保护
4. **Bash 超时参数被忽略** — 接受但完全不处理

---

## 三、pi-ai 复刻进度（整体 ~15-20%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/ai`

### 完整复刻（3 个模块）

| TypeScript | Rust | 说明 |
|------------|------|------|
| `models.ts` | `models.rs` | 7 个函数 100% 复刻 |
| `api-registry.ts` | `api_registry.rs` | 注册/查找/注销机制完整 |
| `stream.ts` | `stream.rs` | stream/complete/streamSimple/completeSimple 4 个入口完整 |

### 部分复刻（6 个模块）

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `types.ts` | `types.rs` | ~90% | 缺失 `KnownApi`/`KnownProvider` 联合类型枚举、`ImagesContext`/`AssistantImages`、`onPayload`/`onResponse` 回调字段 |
| `models.generated.ts` | `models_generated.rs` | ~30% | 仅覆盖 7 个 provider，缺失 15+ 个（bedrock/mistral/cerebras/together/fireworks/minimax/moonshotai 等） |
| `env-api-keys.ts` | `env_api_keys.rs` | ~40% | 缺失 `findEnvKeys()`、OAuth 支持、Vertex ADC 检测、Bedrock 多源凭证 |
| `utils/diagnostics.ts` | `utils/diagnostics.rs` | ~30% | 数据模型与 TS 不一致 |
| `utils/event-stream.ts` | `utils/event_stream.rs` | ~60% | pull-based vs push-based 架构差异 |

### 空壳/桩代码（2 个模块）

| TypeScript | Rust | 覆盖率 | 说明 |
|------------|------|--------|------|
| `providers/anthropic.ts` | `providers/anthropic.rs` | **~1%** | `Box::new(futures::stream::empty())` + TODO |
| `providers/openai-completions.ts` | `providers/openai.rs` | **~1%** | 同上 |

### 完全缺失（29+ 个 TS 文件）

| 类别 | 数量 | 说明 |
|------|------|------|
| Provider 实现 | ~15 | mistral/google/bedrock/azure/vertex/codex/copilot 等全部缺失 |
| Provider 注册 | 1 | `providers/register-builtins.ts`（9 API 自动注册） |
| Utils | 6 | json-parse/overflow/validation/typebox-helpers/headers/session-resources |
| Images 功能 | 5 | images/models/api-registry/image-models.generated + providers/images |
| 其他 | 3 | index/cli/oauth |

### P0 关键缺口

1. **Provider 实现全是空壳** — 整个 crate 的核心价值缺失
2. **15+ provider 未复刻** — 所有非 Anthropic/OpenAI provider
3. **register-builtins 缺失** — 9 API 自动注册未实现

---

## 四、pi-tui 复刻进度（整体 ~22%）

对照 `https://github.com/earendil-works/pi/tree/main/packages/tui`

### 完整复刻（1 个模块）

| TypeScript | Rust | 说明 |
|------------|------|------|
| `components/spacer.ts` | `components/spacer.rs` | **唯一 100% 复刻的组件** |

### 基本复刻（1 个模块）

| TypeScript | Rust | 覆盖率 | 说明 |
|------------|------|--------|------|
| `components/text.ts` | `components/text.rs` | ~85% | 缓存有 bug（`&self` 不可变导致无法写入缓存） |

### 部分复刻（10 个模块）

| TypeScript | Rust | 覆盖率 | 关键缺失 |
|------------|------|--------|----------|
| `tui.ts` | `tui.rs` | ~50% | **无 diff 渲染管线**（行级增量 → ratatui 全屏重绘）；覆盖层简化（无百分比/z-ordering/focus 回调） |
| `terminal.ts` | `terminal.rs` | ~35% | 缺失 Kitty 协商序列、StdinBuffer、paste 处理、Apple Terminal 检测、drainInput |
| `keys.ts` | `keys.rs` | ~30% | 缺失 modifyOtherKeys 解析、Kitty flag 4 替代键、数字键盘、4 张完整 legacy 表 |
| `keybindings.ts` | `keybindings.rs` | ~75% | 23/30 绑定已实现；缺失 word 导航/yank/delete-to/copy |
| `utils.ts` | `utils.rs` | ~35% | 缺失 extractSegments、sliceByColumn、AnsiCodeTracker、grapheme 分词 |
| `components/box.ts` | `components/box_component.rs` | ~60% | 缺失 removeChild、functional background、有效缓存 |
| `components/input.ts` | `components/input.rs` | ~30% | 缺失 kill-ring、undo stack、bracketed paste、grapheme 光标、word 导航 |
| `components/select-list.ts` | `components/select_list.rs` | ~50% | 缺失 callbacks、label 字段、wrapping selection、layout options |

### 完全缺失（14 个模块）

| 模块 | 行数 | 重要性 | 说明 |
|------|------|--------|------|
| `components/editor.ts` | ~1500 | **核心** | 多行编辑器、选区、kill-ring、undo、autocomplete 集成、历史导航 |
| `components/markdown.ts` | ~800 | **核心** | Markdown 渲染器（标题/代码块/列表/表格/行内格式） |
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

### P0 关键缺口

1. **Editor 组件**（~1500 行）— 整个 TUI 的核心交互组件完全缺失
2. **Diff 渲染管线** — 当前用 ratatui 全屏重绘，TS 是行级增量 diff
3. **Markdown 组件**（~800 行）— AI 回复渲染缺失
4. **基础设施链** — kill-ring → undo-stack → stdin-buffer → word-navigation → fuzzy，5 个模块都是 Editor 前置依赖

---

## 五、四 crate 依赖链与实施顺序

### pi-ai（最底层，其他 crate 的基础）

```
Step 1: 补全 models_generated.rs（15+ provider 模型数据）
Step 2: 补全 env_api_keys.rs（OAuth/Vertex/Bedrock 凭证检测）
Step 3: 实现 register-builtins（自动注册 9 API provider）
Step 4: 实现 providers/anthropic.rs（完整 SSE streaming）
Step 5: 实现 providers/openai.rs（completions + responses）
Step 6: 逐个补全其他 provider ← 这是最耗时的部分
Step 7: 复刻 utils 模块（json-parse/overflow/validation）
Step 8: Images 功能
```

### pi-agent-core（依赖 pi-ai 的类型）

```
Step 1: 补全 agent_loop.rs 多轮循环 + 工具执行
Step 2: 实现 compaction/branch_summarization 的 LLM 调用（通过 pi-ai）
Step 3: 补全 prompt_templates 文件加载 + 参数替换
Step 4: 实现 AgentHarness 编排循环
Step 5: 补全 skills（递归加载/gitignore/frontmatter）
```

### pi-tui（独立 crate，构建 UI 框架）

```
Step 1: 基础设施 — kill-ring + undo-stack + stdin-buffer + word-navigation + fuzzy
Step 2: 补全 Input（kill-ring/undo/paste/grapheme）
Step 3: 补全 SelectList（callbacks/wrapping/layout）
Step 4: Editor 组件（核心交互 — 最复杂）
Step 5: Markdown 组件（AI 回复渲染）
Step 6: Autocomplete
Step 7: Diff 渲染管线优化
Step 8: 高级组件 + Terminal 增强
```

### pi-coding-agent（最高层，依赖上面三个 crate）

```
Step 1: Edit diff 引擎（最大功能缺口）
Step 2: Prompt 模板参数替换
Step 3: File mutation queue
Step 4: Bash 超时 + 流式输出 + 进程树管理
Step 5: Grep/Find 改为使用 rg/fd 二进制
Step 6: 补全 Session/Settings/Model registry 缺失字段
Step 7: Compaction pipeline
Step 8: Agent session 事件系统 + 自动重试
Step 9: Extensions 运行时
Step 10: CLI + Modes + Utils
```

---

## 六、测试覆盖总览

| Crate | 测试数 | 通过 | 失败 |
|-------|--------|------|------|
| pi-agent-core | 0 | — | — |
| pi-coding-agent | 40 | 40 | 0 |
| pi-ai | 20 | 20 | 0 |
| pi-tui | 35 | 35 | 0 |
| **总计** | **95** | **95** | **0** |

> 注：pi-agent-core 无测试，按 CLAUDE.md 规范"先写测试，再写实现"的要求，这是最大的流程缺口。
