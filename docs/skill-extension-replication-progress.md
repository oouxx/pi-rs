# 技能/扩展系统复刻进度对比

**最后更新**: 2026-07-13 (Phase 8 对齐完成)
**对比范围**: TypeScript 原版 (`packages/coding-agent`) → Rust 版 (`crates/pi-coding-agent` + `crates/pi-agent-core`)

---

## 一、技能系统（Skills）

### 1.1 核心类型定义

| 组件 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `Skill` struct | `skills.ts:74-81` | `pi-coding-agent/src/core/skills.rs:32-39` | ✅ 已复刻 |
| `SkillFrontmatter` | `skills.ts:67-72` | `pi-coding-agent/src/core/skills.rs:22-28` | ✅ 已复刻 |
| `LoadSkillsOptions` | `skills.ts:372-381` | `pi-coding-agent/src/core/skills.rs:42-47` | ✅ 已复刻 |
| `LoadSkillsResult` | `skills.ts:83-86` | `pi-coding-agent/src/core/skills.rs:49-53` | ✅ 已复刻 |
| `ResourceDiagnostic` | `diagnostics.ts` | `diagnostics.rs` | ✅ 已复刻 |

### 1.2 核心函数

| 函数 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `validateName()` | `skills.ts:92-112` | `skills.rs:60-86` | ✅ 已复刻 |
| `validateDescription()` | `skills.ts:117-127` | `skills.rs:89-110` | ✅ 已复刻 |
| `loadSkillFromFile()` | `skills.ts:277-325` | `skills.rs:206-278` | ✅ 已复刻 |
| `loadSkillsFromDir()` | `skills.ts:168-275` | `skills.rs:286-356` | ✅ 已复刻 |
| `loadSkills()` | `skills.ts:387-487` | `skills.rs:363-466` | ✅ 已复刻 |
| `formatSkillsForPrompt()` | `skills.ts:335-370` | `skills.rs:477-511` | ✅ 已复刻 |
| `escapeXml()` | `skills.ts:363-370` | `skills.rs:513-519` | ✅ 已复刻 |

### 1.3 差异点

| 差异 | TS 原版 | Rust 版 | 优先级 | 说明 |
|------|---------|---------|--------|------|
| 前端解析 | 使用 `parseFrontmatter()` 工具函数 | 内联 `parse_frontmatter()` 手写解析 | 🟡 中 | 不支持多行 YAML 值，但单行场景够用 |
| 忽略规则 | 支持 `.gitignore`/`.ignore`/`.fdignore` | **缺失** | 🟢 低 | 技能目录通常没有 ignore 文件 |
| 符号链接处理 | 完整处理（`statSync` 区分文件/目录） | 部分处理 | 🟢 低 | 边界情况 |
| 路径解析 | 使用 `resolvePath()` 支持 `~` 展开 | 使用 `PathBuf::from()` | 🟡 中 | 影响用户体验 |
| `node_modules` 跳过 | 在遍历中跳过 | ✅ 已实现 | — | 已复刻 |
| 多行描述 | 支持 YAML 多行值 | ❌ 不支持 | 🟢 低 | 技能描述通常为单行 |

### 1.4 测试覆盖

| 测试用例 | TS 原版 | Rust 版 | 优先级 | 状态 |
|---------|---------|---------|--------|------|
| 验证有效名称 | ✅ | ✅ | — | 已复刻 |
| 名称超长 | ✅ | ✅ | — | 已复刻 |
| 名称含无效字符 | ✅ | ✅ | — | 已复刻 |
| 名称以连字符开头 | ✅ | ✅ | — | 已复刻 |
| 名称含连续连字符 | ✅ | ✅ | — | 已复刻 |
| 描述为空 | ✅ | ✅ | — | 已复刻 |
| 描述超长 | ✅ | ✅ | — | 已复刻 |
| 无 frontmatter | ✅ | ✅ | — | 已复刻 |
| 含字段的 frontmatter | ✅ | ✅ | — | 已复刻 |
| 部分 frontmatter | ✅ | ✅ | — | 已复刻 |
| 格式化 skills 为 XML | ✅ | ✅ | — | 已复刻 |
| 空 skills 列表 | ✅ | ✅ | — | 已复刻 |
| disable-model-invocation | ✅ | ✅ | — | 已复刻 |
| XML 转义 | ✅ | ✅ | — | 已复刻 |
| 从目录加载（含 fixture） | ✅ | ✅ | ✅ 已复刻（25 个 fixture 测试） |
| 嵌套技能 | ✅ | ✅ | ✅ 已复刻 |
| 名称冲突 | ✅ | ✅ | ✅ 已复刻 |
| 不存在的路径 | ✅ | ✅ | ✅ 已复刻 |
| 多行描述 | ✅ | ⚠️ 不支持 | 🟢 低 — YAML 多行值解析器限制 |
| 未知字段 | ✅ | ✅ | ✅ 已复刻 |
| 从 explicit skillPaths 加载 | ✅ | ✅ | ✅ 已复刻 |
| 根 SKILL.md 优先 | ✅ | ✅ | ✅ 已复刻 |
| 无 frontmatter 文件 | ✅ | ✅ | ✅ 已复刻 |
| 无效 YAML | ✅ | ⚠️ 部分 | 🟢 低 — 简单解析器限制 |

### 1.5 技能系统进度：~80%

**已完成**：核心加载、验证、格式化、冲突检测逻辑全部复刻。
**待完善**：`~` 路径展开、基于 fixture 的集成测试。

---

## 二、扩展系统（Extensions）

### 2.1 架构对比

| 维度 | TS 原版 | Rust 版 |
|------|---------|---------|
| **运行时** | Bun 原生 JS 运行时（同进程） | 嵌入式 `deno_core` V8 运行时（独立线程） |
| **模块加载** | `jiti` 动态导入 | `deno_core::ModuleLoader` + `deno_ast` 转译 |
| **通信方式** | 同进程直接调用 | V8 线程 ↔ 主线程通过 `mpsc` 通道 + `oneshot` 回复 |
| **扩展语言** | TypeScript/JavaScript | TypeScript/JavaScript（通过 deno_core） |
| **热重载** | 通过 `reload()` 方法 | ✅ 已实现 `RuntimeCommand::Reload` |

### 2.2 文件级对比

| 文件 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `types.ts` / `types.rs` | 1568 行完整类型定义 | 55 行仅 `ToolDefinition` | ⚠️ **大幅简化** — 大部分类型在 JS shim 中 |
| `loader.ts` / `loader.rs` | 601 行 | 286 行 | ⚠️ **架构不同** — jiti → deno_core |
| `runner.ts` / `runtime.rs` | 1069 行 | 728 行 | ⚠️ **架构不同** — 类 → 线程+通道 |
| `dispatcher.rs` | — | 467 行 | ✅ **新增** — Rust 特有的事件分发层 |
| `ops.rs` | — | 616 行 | ✅ **新增** — deno_core ops 定义 |
| `wrapper.ts` | — | — | ❌ **不需要** — Rust 版通过 `create_extension_agent_tools()` 实现 |
| `index.ts` / `mod.rs` | 173 行 | 13 行 | ⚠️ 仅 re-export |

### 2.3 扩展发现

| 发现规则 | TS 原版 | Rust 版 | 状态 |
|---------|---------|---------|------|
| 项目本地 `{cwd}/.pi/extensions/` | ✅ | ✅ `{cwd}/.pi-rs/extensions/` | ✅ 已复刻（目录名不同，避免冲突） |
| 全局 `{agentDir}/extensions/` | ✅ | ✅ | 已复刻 |
| 显式路径 | ✅ | ✅ | 已复刻 |
| `package.json` `pi.extensions` 清单 | ✅ | ✅ | 已复刻 |
| `index.ts`/`index.js` 自动发现 | ✅ | ✅ | 已复刻 |
| 热重载支持 | ✅ | ✅ | 已复刻 |
| 符号链接去重 | ✅ | ✅ | 已复刻 |

---

## 三、当前优先级（重要）— 不依赖 pi-tui

以下部分**不依赖 pi-tui**，可以独立完成，是当前阶段的重点。

### 3.1 核心架构（已就绪）

| 组件 | 状态 | 说明 |
|------|------|------|
| 扩展发现（项目/全局/显式路径） | ✅ | 已复刻 |
| 扩展加载（deno_core V8 运行时） | ✅ | 已复刻 |
| 工具注册和调用 | ✅ | 已复刻 |
| 事件分发框架（fire-and-forget + result-returning） | ✅ | 已复刻 |
| 命令/快捷键/标志注册 | ✅ | 已复刻 |
| exec 命令执行 | ✅ | 已复刻 |
| 热重载 | ✅ | 已复刻 |
| EventBus（JS shim 内） | ✅ | 已复刻 |

### 3.2 ExtensionAPI 方法（22 个）

| API 方法 | TS 原版 | Rust 版 | 状态 |
|----------|---------|---------|------|
| `on()` | ✅ | ✅ (通过 JS runtime.js) | ✅ |
| `registerTool()` | ✅ | ✅ `op_pi_register_tool` | ✅ |
| `registerCommand()` | ✅ | ✅ `op_pi_register_command` | ✅ |
| `registerShortcut()` | ✅ | ✅ `op_pi_register_shortcut` | ✅ |
| `registerFlag()` | ✅ | ✅ `op_pi_register_flag` | ✅ |
| `getFlag()` | ✅ | ✅ `op_pi_get_flags` | ✅ |
| `sendMessage()` | ✅ | ✅ `op_pi_send_message` | ✅ |
| `sendUserMessage()` | ✅ | ✅ `op_pi_send_user_message` | ✅ |
| `appendEntry()` | ✅ | ✅ `op_pi_append_entry` | ✅ |
| `setSessionName()` | ✅ | ✅ `op_pi_set_session_name` | ✅ |
| `getSessionName()` | ✅ | ✅ `op_pi_get_session_name` | ✅ |
| `setLabel()` | ✅ | ✅ `op_pi_set_label` | ✅ |
| `exec()` | ✅ | ✅ `op_pi_exec` | ✅ |
| `getCommands()` | ✅ | ✅ `op_pi_get_commands` | ✅ |
| `setModel()` | ✅ | ✅ `op_pi_set_model` | ✅ |
| `setThinkingLevel()` | ✅ | ✅ `op_pi_set_thinking_level` | ✅ |
| `registerProvider()` | ✅ | ✅ `op_pi_register_provider` | ✅ |
| `unregisterProvider()` | ✅ | ✅ `op_pi_unregister_provider` | ✅ |
| `events` (EventBus) | ✅ | ✅ (JS shim 内) | ✅ |
| `getActiveTools()` | ✅ | ✅ `op_pi_get_active_tools` | ✅ |
| `getAllTools()` | ✅ | ✅ `op_pi_get_all_tools` | ✅ |
| `setActiveTools()` | ✅ | ✅ `op_pi_set_active_tools` | ✅ |
| `getThinkingLevel()` | ✅ | ✅ `op_pi_get_thinking_level` | ✅ |

**进度：22/22 = 100%**

### 3.3 ExtensionContext 方法（12 个，排除 UI 相关）

| ctx 方法 | TS 原版 | Rust 版 | 状态 |
|----------|---------|---------|------|
| `cwd` | ✅ | ✅ (通过 `__piSetCwd`) | ✅ |
| `isIdle()` | ✅ | ✅ `op_pi_ctx_is_idle` | ✅ — 通过 HostCommand 查询 agent 状态 |
| `abort()` | ✅ | ✅ `op_pi_ctx_abort` | ✅ — 通过 HostCommand 请求中止 |
| `hasPendingMessages()` | ✅ | ✅ `op_pi_ctx_has_pending_messages` | ✅ — 通过 HostCommand 查询 pending tool calls |
| `shutdown()` | ✅ | ✅ `op_pi_ctx_shutdown` | ✅ — 通过 HostCommand 请求关闭 |
| `getSystemPrompt()` | ✅ | ✅ `op_pi_ctx_get_system_prompt` | ✅ — 通过 HostCommand 返回 system prompt |
| `sessionManager` | ✅ | ✅ (JS shim 内) | ✅ — 暴露 newSession/fork/switchSession/reload |
| `modelRegistry` | ✅ | ✅ (JS shim 内) | ✅ — 暴露 getModel/setModel |
| `model` | ✅ | ✅ (JS shim 内) | ✅ — 通过 `op_pi_ctx_get_model` |
| `signal` | ✅ | ✅ (JS shim 内) | ✅ — 暴露为 `null`（等待 pi-tui 集成） |
| `getContextUsage()` | ✅ | ✅ `op_pi_ctx_get_context_usage` | ✅ — 通过 HostCommand 返回使用统计 |
| `compact()` | ✅ | ✅ `op_pi_ctx_compact` | ✅ — 通过 HostCommand 触发压缩 |

**进度：12/12 完整实现**

### 3.4 事件系统（28 个，排除 UI 相关）

| 事件 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `resources_discover` | ✅ | ✅ `dispatch_resources_discover()` | ✅ |
| `session_start` | ✅ | ✅ `dispatch_session_start()` | ✅ |
| `session_shutdown` | ✅ | ✅ `dispatch_session_shutdown()` | ✅ |
| `context` | ✅ | ✅ `dispatch_context()` | ✅ |
| `before_provider_request` | ✅ | ✅ `dispatch_before_provider_request()` | ✅ |
| `after_provider_response` | ✅ | ✅ `dispatch_after_provider_response()` | ✅ |
| `agent_start` | ✅ | ✅ (fire-and-forget) | ✅ |
| `agent_end` | ✅ | ✅ (fire-and-forget) | ✅ |
| `turn_start` | ✅ | ✅ (fire-and-forget) | ✅ |
| `turn_end` | ✅ | ✅ (fire-and-forget) | ✅ |
| `message_start` | ✅ | ✅ (fire-and-forget) | ✅ |
| `message_update` | ✅ | ✅ (fire-and-forget) | ✅ |
| `tool_execution_start` | ✅ | ✅ (fire-and-forget) | ✅ |
| `tool_execution_end` | ✅ | ✅ (fire-and-forget) | ✅ |
| `model_select` | ✅ | ✅ `dispatch_model_select()` | ✅ |
| `thinking_level_select` | ✅ | ✅ `dispatch_thinking_level_select()` | ✅ |
| `user_bash` | ✅ | ✅ `dispatch_user_bash()` | ✅ |
| `input` | ✅ | ✅ `dispatch_input()` | ✅ |
| `tool_call` | ✅ | ✅ `dispatch_tool_call()` | ✅ |
| `tool_result` | ✅ | ✅ `dispatch_tool_result()` | ✅ |
| `message_end` | ✅ | ✅ result-returning | ✅ — JS 侧已支持结果返回，Rust 侧改为 `dispatch_result` |
| `session_before_switch` | ✅ | ✅ `dispatch_session_before_switch()` | ✅ |
| `session_before_fork` | ✅ | ✅ `dispatch_session_before_fork()` | ✅ |
| `session_before_compact` | ✅ | ✅ `dispatch_session_before_compact()` | ✅ |
| `session_compact` | ✅ | ✅ `dispatch_session_compact()` | ✅ |
| `session_before_tree` | ✅ | ✅ `dispatch_session_before_tree()` | ✅ |
| `session_tree` | ✅ | ✅ `dispatch_session_tree()` | ✅ |
| `before_agent_start` | ✅ | ✅ `dispatch_before_agent_start()` | ✅ |
| `tool_execution_update` | ✅ | ❌ **跳过** | 高频事件，有意跳过 |

**进度：27/28 = 96%**

### 3.5 ExtensionCommandContext 方法（6 个）

| 方法 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `waitForIdle()` | ✅ | ✅ `op_pi_wait_for_idle` | ✅ — 通过 HostCommand 轮询 agent 空闲状态 |
| `newSession()` | ✅ | ✅ `op_pi_new_session` | ✅ — 通过 HostCommand + 生命周期事件 |
| `fork()` | ✅ | ✅ `op_pi_fork` | ✅ — 通过 HostCommand + 生命周期事件 |
| `switchSession()` | ✅ | ✅ `op_pi_switch_session` | ✅ — 通过 HostCommand + 生命周期事件 |
| `reload()` | ✅ | ✅ `op_pi_reload` | ✅ — 通过 HostCommand + 生命周期事件 |
| `navigateTree()` | ✅ | ✅ `op_pi_navigate_tree` | ✅ — 通过 HostCommand + 生命周期事件 |

**进度：6/6 完整实现**

### 3.6 当前优先级进度：~95%

**核心架构已就绪**（扩展加载、工具调用、事件分发框架）。所有非 UI 的 ExtensionAPI 方法、ExtensionContext 方法、ExtensionCommandContext 方法、事件均已实现。HostCommand 通道已建立，大部分方法通过 HostCommand 委托到主线程处理。

#### 待完成清单（按优先级排序）

| 优先级 | 任务 | 涉及文件 | 工作量估计 | 状态 |
|--------|------|---------|-----------|------|
| 🔴 P0 | `before_agent_start` 事件 | `dispatcher.rs`, `runtime.js` | 小 | ✅ 已完成 |
| 🔴 P0 | `message_end` 结果返回 | `dispatcher.rs`, `runtime.js` | 小 | ✅ 已完成 |
| 🔴 P0 | 工具管理 API（getActiveTools/getAllTools/setActiveTools） | `ops.rs`, `runtime.js` | 中 | ✅ 已完成 |
| 🔴 P0 | `getThinkingLevel()` | `ops.rs`, `runtime.js` | 小 | ✅ 已完成 |
| 🟡 P1 | ExtensionContext 方法从 stub 改为真实实现 | `ops.rs`, `runtime.rs` | 中 | ✅ 已完成 |
| 🟡 P1 | 会话生命周期事件（before_switch/before_fork 等 6 个） | `dispatcher.rs`, `runtime.js` | 中 | ✅ 已完成 |
| 🟡 P1 | ExtensionCommandContext 方法从 stub 改为真实实现 | `ops.rs`, `runtime.rs` | 大 | ✅ 已完成 |
| 🟢 P2 | 技能系统集成测试（基于 fixture） | `tests/` | 中 | ⏳ 待完成 |
| 🟢 P2 | `~` 路径展开 | `skills.rs` | 小 | ⏳ 待完成 |

---

## 四、后续规划 — 依赖 pi-tui 的部分

以下部分**强依赖 pi-tui**（TUI 渲染引擎），需要等 pi-tui 升级 ratatui 版本、interactive 模式重新启用后再实现。

### 4.1 依赖关系说明

```
ExtensionUIContext (28 个方法)
  ├── 直接依赖 pi-tui 的类型：
  │     TUI, Component, EditorComponent, EditorTheme,
  │     KeyId, OverlayHandle, OverlayOptions,
  │     AutocompleteItem, AutocompleteProvider
  ├── 依赖 interactive mode 的 Theme
  └── 方法签名中大量出现 (tui: TUI, theme: Theme) => Component
```

Rust 版 `pi-tui` 当前已移出 workspace members（ratatui 版本冲突），interactive 模式 gated 在 `#[cfg(feature = "interactive")]` 后。

### 4.2 ExtensionUIContext（28 个方法）

| UI 方法 | TS 原版 | Rust 版 | 说明 |
|---------|---------|---------|------|
| `notify()` | ✅ | ✅ `op_pi_notify` | 唯一有实际效果的方法 |
| `setStatus()` | ✅ | ⚠️ stub | 空操作 |
| `setWorkingMessage()` | ✅ | ⚠️ stub | 空操作 |
| `setTitle()` | ✅ | ⚠️ stub | 空操作 |
| `select()` | ✅ | ❌ | 需要 TUI 选择器组件 |
| `confirm()` | ✅ | ❌ | 需要 TUI 对话框组件 |
| `input()` | ✅ | ❌ | 需要 TUI 输入框组件 |
| `editor()` | ✅ | ❌ | 需要 TUI 编辑器组件 |
| `custom()` | ✅ | ❌ | 需要 TUI 叠加层 |
| `setWidget()` | ✅ | ❌ | 需要 TUI 组件渲染 |
| `setFooter()` | ✅ | ❌ | 需要 TUI 组件渲染 |
| `setHeader()` | ✅ | ❌ | 需要 TUI 组件渲染 |
| `setEditorComponent()` | ✅ | ❌ | 需要 TUI 编辑器 |
| `getEditorComponent()` | ✅ | ❌ | 需要 TUI 编辑器 |
| `addAutocompleteProvider()` | ✅ | ❌ | 需要 TUI 自动补全 |
| `pasteToEditor()` | ✅ | ❌ | 需要 TUI 编辑器 |
| `setEditorText()` | ✅ | ❌ | 需要 TUI 编辑器 |
| `getEditorText()` | ✅ | ❌ | 需要 TUI 编辑器 |
| `onTerminalInput()` | ✅ | ❌ | 需要 TUI 终端输入 |
| `setWorkingVisible()` | ✅ | ❌ | 需要 TUI 加载动画 |
| `setWorkingIndicator()` | ✅ | ❌ | 需要 TUI 加载动画 |
| `setHiddenThinkingLabel()` | ✅ | ❌ | 需要 TUI 渲染 |
| `theme` | ✅ | ❌ | 需要 pi-tui Theme 类型 |
| `getAllThemes()` | ✅ | ❌ | 需要 pi-tui Theme 类型 |
| `getTheme()` | ✅ | ❌ | 需要 pi-tui Theme 类型 |
| `setTheme()` | ✅ | ❌ | 需要 pi-tui Theme 类型 |
| `getToolsExpanded()` | ✅ | ❌ | 需要 TUI 状态 |
| `setToolsExpanded()` | ✅ | ❌ | 需要 TUI 状态 |

### 4.3 其他依赖 pi-tui 的 API

| API | 依赖原因 |
|-----|---------|
| `registerMessageRenderer()` | 返回 `Component`（TUI 类型） |
| `renderCall` / `renderResult`（ToolDefinition 中） | 返回 `Component`（TUI 类型） |
| `hasUI`（ExtensionContext 中） | 需要 interactive 模式存在 |

### 4.4 后续实现计划

当 pi-tui 就绪后，需要：

1. **runtime.js**：在 `makeContext()` 中暴露所有 UI 方法
2. **ops.rs**：为每个 UI 方法添加 `op_pi_ui_*` op（或通过 `HostCommand` 通道委托到主线程）
3. **interactive 模式**：实现 `ExtensionUIContext` 接口（参考 `interactive-mode.ts:1964-2018`）

---

## 五、pi-agent-core 中的技能系统（Harness）

`pi-agent-core/src/harness/` 下存在另一套技能系统，与 `pi-coding-agent` 中的技能系统**功能重叠**：

| 组件 | `pi-coding-agent` skills.rs | `pi-agent-core` harness skills |
|------|---------------------------|-------------------------------|
| 定位 | 生产级技能加载 | 测试/夹具用技能加载 |
| 前端解析 | 手写行解析 | `serde_yaml` 完整解析 |
| 忽略规则 | ❌ 缺失 | ❌ 缺失 |
| 符号链接 | 部分处理 | ❌ 缺失 |
| 格式化 | `format_skills_for_prompt()` | `format_skills_for_system_prompt()` + `format_skill_invocation()` |
| 测试 | 单元测试 | 单元测试 |

**建议**：统一为 `pi-coding-agent` 中的实现，`pi-agent-core` 中的技能系统作为测试辅助保留或移除。

---

## 六、总体进度总结

| 子系统 | 进度 | 说明 |
|--------|------|------|
| 技能系统（Skills） | **~80%** | 核心逻辑已复刻，缺集成测试和少量边界功能 |
| 扩展系统 — 核心架构 | **~90%** | 发现、加载、工具调用、事件分发框架已就绪 |
| 扩展系统 — 非 UI 接线 | **~95%** | API 方法 100%，事件 96%，ctx 方法 100%，cmd 方法 100% |
| 扩展系统 — UI 部分 | **~0%** | 依赖 pi-tui，后续规划 |
| 测试覆盖 | **~80%** | 技能 25 个 fixture 测试 + 扩展 10 个端到端测试 |
