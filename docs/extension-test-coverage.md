# Extension 系统测试覆盖率报告

> 分析日期：2026-07-14（更新：2026-07-14，补齐优先级 1 V8 运行时测试）
> 分析范围：原版 TypeScript `@earendil-works/pi-coding-agent` 测试 vs Rust 复刻 `pi-coding-agent`

---

## 总览

| 原版测试文件 | 测试数 | Rust 已覆盖 | 覆盖率 |
|---|---|---|---|
| `extensions-discovery.test.ts` | 30 | 25 | 83% |
| `extensions-runner.test.ts` | ~30 | 24 | 80% |
| `extensions-input-event.test.ts` | 10 | 6 | 60% |
| `package-manager.test.ts` | ~40 | 36 | ~90% |
| `package-manager-ssh.test.ts` | 8 | 0 | 0% |
| `compaction-extensions.test.ts` | 8 | 0 | 0% |
| `compaction-extensions-example.test.ts` | 2 | 0 | 0% |
| `plan-mode-extension.test.ts` | 4 | 0 | 0% |
| `git-merge-and-resolve-extension.test.ts` | 9 | 0 | 0% |
| `resource-loader.test.ts` | 22 | 0 | 0% |
| **合计** | **~163** | **91** | **~56%** |

### Rust 测试模块分布（200 个测试）

| Rust 模块 | 测试数 | 说明 |
|-----------|--------|------|
| `loader.rs` | 66 | 发现逻辑 + node_modules 解析（含 33 个 Rust 新增） |
| `dispatcher.rs` | 22 | 事件 payload 构建 + AgentEvent 映射 |
| `ops.rs` | 51 | 全部 deno ops（注册/查询/host command/stub） |
| `runtime.rs` | 24 | V8 线程生命周期 + 加载/调用/分发/重载 |
| `types.rs` | 1 | ToolDefinition 序列化 |
| `package_manager.rs` | 36 | 包管理器（解析/缓存/进度/序列化/去重） |

---

## 逐模块详细对比

### 1. `loader.rs` — 模块加载器 + 扩展发现

**Rust 文件：** `crates/pi-coding-agent/src/core/extensions/loader.rs`
**原版 TS 源：** `packages/coding-agent/src/core/extensions/loader.ts`
**原版测试：** `test/extensions-discovery.test.ts`

#### 已迁移（25 个）

| 测试 | 原版对应 | Rust 函数 |
|------|---------|----------|
| 直接 `.ts` 文件发现 | `discovers direct .ts files in extensions/` | `discover_in_dir` |
| 直接 `.js` 文件发现 | `discovers direct .js files in extensions/` | `discover_in_dir` |
| 子目录 `index.ts` | `discovers subdirectory with index.ts` | `discover_in_dir` + `find_index` |
| 子目录 `index.js` | `discovers subdirectory with index.js` | `discover_in_dir` + `find_index` |
| 优先 `index.ts` > `index.js` | `prefers index.ts over index.js` | `find_index`（顺序已修复） |
| `package.json` `pi.extensions` | `discovers subdirectory with package.json pi field` | `discover_in_dir` |
| 多个 extension 声明 | `package.json can declare multiple extensions` | `discover_in_dir` |
| `pi` 字段优先于 `index.ts` | `package.json with pi field takes precedence over index.ts` | `discover_in_dir` |
| 无 `pi` 字段回退 `index.ts` | `ignores package.json without pi field, falls back to index.ts` | `discover_in_dir` |
| 无 index/package.json 忽略 | `ignores subdirectory without index or package.json` | `discover_in_dir` |
| 不递归超过一层 | `does not recurse beyond one level` | `discover_in_dir` |
| 混合文件和子目录 | `handles mixed direct files and subdirectories` | `discover_extensions` |
| 跳过不存在的路径 | `skips non-existent paths declared in package.json` | `discover_in_dir` |
| 显式路径 | `handles explicitly configured paths` | `discover_extensions` |
| 去重（规范路径） | `dedup by resolved path` | `discover_extensions` |
| 全局 extension | `global extensions` | `discover_extensions` |
| 空目录 | `empty dir` | `discover_extensions` |
| 符号链接文件 | `symlink file` | `discover_in_dir` |
| 符号链接目录 | `symlink dir` | `discover_in_dir` |
| `.mjs`/`.cjs`/`.tsx` 文件 | 扩展支持 | `discover_in_dir` |
| 非 extension 文件忽略 | `non-extension files ignored` | `discover_in_dir` |
| 不存在的目录 | `nonexistent dir` | `discover_extensions` |
| 显式目录含 index | `explicit dir with index` | `discover_extensions` |
| 显式目录扫描内容 | `explicit dir without index` | `discover_extensions` |
| 不存在的显式路径跳过 | `explicit nonexistent path skipped` | `discover_extensions` |

#### 未迁移（5 个，需 V8 运行时）

| 测试 | 原因 |
|------|------|
| 加载 extension 并注册命令 | 需要启动 V8 实例执行 JS |
| 加载 extension 并注册工具 | 同上 |
| 无效代码报错 | 同上 |
| 初始化抛异常 | 同上 |
| 无 default export 报错 | 同上 |
| with-deps 依赖解析 | 同上 |

#### Rust 新增（33 个，原版无对应）

| 测试类别 | 数量 | 说明 |
|---------|------|------|
| `parse_bare_specifier` | 6 | 裸 specifier 解析（简单/subpath/scoped/earendil） |
| `resolve_file_with_extensions` | 3 | 文件扩展名补全 |
| `resolve_package_entry` | 7 | 包入口解析（index.js/main/exports/subpath） |
| `resolve_node_modules` | 5 | node_modules 遍历（向上/就近/未找到/subpath/scoped） |
| 完整 `resolve()` | 3 | 相对/绝对/bare specifier 集成 |
| `@earendil-works/*` 兜底 | 4 | 通过编译时 fallback 解析 |
| 边界条件 | 3 | data URL/空 specifier/无效 referrer |
| 发现逻辑增强 | 2 | reloadable 标记、全局 extension reloadable |

---

### 2. `runtime.rs` — ExtensionRuntime（V8 线程）

**Rust 文件：** `crates/pi-coding-agent/src/core/extensions/runtime.rs`
**原版 TS 源：** `packages/coding-agent/src/core/extensions/loader.ts`（`createExtensionRuntime`）
**原版测试：** `test/extensions-runner.test.ts`（部分）

#### 已迁移（24 个）

| 测试 | 说明 |
|------|------|
| 创建运行时 | `ExtensionRuntime::new()` 启动 V8 线程 |
| 创建并停止 | `stop()` 优雅关闭 |
| 加载空目录 | 无扩展时不报错 |
| 加载含工具的扩展 | 注册工具并回读 metadata |
| 加载含命令的扩展 | 注册命令并回读 metadata |
| 加载多个扩展 | 多文件发现 + 工具聚合 |
| 调用工具 | `call_tool()` → JS execute → 结果回传 |
| 调用工具含通知 | `ctx.ui.notify` 缓冲到 response |
| 无效扩展报错 | 语法错误 → `LoadError` |
| 初始化抛异常 | factory throw → `LoadError` |
| 无 default export 报错 | 非 function → `LoadError` |
| fire-and-forget 分发 | `dispatch_fire_and_forget()` |
| result-returning 分发（block） | `dispatch_result()` 返回 block 决策 |
| result-returning 分发（no block） | 安全工具不被拦截 |
| 错误事件订阅 | `on_error()` 接收 handler 异常 |
| HostCommand 轮询 | `poll_host_command()` 取队列首 |
| HostCommand 清空 | `drain_host_commands()` 取全部 |
| HostCommand 处理 | `process_host_commands()` 闭包处理 |
| 重载扩展 | `reload()` 清除 + 重新发现 + 加载 |
| 调用不存在工具 | 返回 Err |
| 重复停止 | 幂等 no-op |
| Clone + Drop | 共享线程，clone 停止后原句柄失效 |
| agent_dir 加载 | 全局扩展目录 |
| 显式路径加载 | `-e` 指定的路径 |

#### 未迁移

| 测试 | 说明 |
|------|------|
| 超时处理 | `COMMAND_TIMEOUT` 120s — 难以在测试中模拟 |

---

### 3. `dispatcher.rs` — 事件分发

**Rust 文件：** `crates/pi-coding-agent/src/core/extensions/dispatcher.rs`
**原版 TS 源：** `packages/coding-agent/src/core/extensions/runner.ts`（`ExtensionRunner`）
**原版测试：** `test/extensions-runner.test.ts` + `test/extensions-input-event.test.ts`

#### 已迁移（22 个）

| 测试 | 说明 |
|------|------|
| `tool_call_payload` 结构 | JSON 字段验证（type/toolCallId/toolName/input） |
| `tool_call_payload` 空 args | `Value::Null` 处理 |
| `tool_result_payload` 结构 | content/details/isError 字段 |
| `tool_result_payload` 错误 | isError=true 路径 |
| `event_from_agent_start` | 事件名 + 非结果返回 |
| `event_from_agent_end` | messages 字段 |
| `event_from_turn_start` | 事件名 |
| `event_from_turn_end` | message + toolResults |
| `event_from_message_start` | User 消息映射 |
| `event_from_message_update` | Assistant 消息 + AssistantMessageEvent |
| `event_from_message_end` | 结果返回标记 |
| `event_from_tool_execution_start` | toolCallId/toolName/args |
| `event_from_tool_execution_update` 跳过 | 高频事件返回 None |
| `event_from_tool_execution_end` | result/isError |
| `fire_and_forget` 丢弃 result 标记 | legacy wrapper |
| `fire_and_forget` 跳过高频 | ToolExecutionUpdate |
| `ResourcesDiscoverResult` 默认 | 空路径列表 |
| `InputEventResult::Continue` | text/images 变体 |
| `InputEventResult::Handled` | 短路变体 |
| `ProjectTrustResult` yes/no/undecided | 三种信任决策 |

#### 未迁移（需 V8 集成）

| 事件类型 | 说明 |
|---------|------|
| `tool_result` 链式修改 | 多 handler 串行 content 累加 |
| `tool_result` 部分 patch 保留 | 后续 handler 只改 isError，保留前 content |
| `before_provider_headers` 修改 | header 原地修改 |
| `before_provider_headers` 错误隔离 | 一个 handler 抛异常不影响其他 |
| `input` transform/handled/continue | 完整 input 事件链 |
| `before_agent_start` systemPrompt 链式 | 多 handler 拼接 systemPrompt |

---

### 4. `ops.rs` — Deno ops

**Rust 文件：** `crates/pi-coding-agent/src/core/extensions/ops.rs`
**原版 TS 源：** `packages/coding-agent/src/core/extensions/runner.ts`
**原版测试：** `test/extensions-runner.test.ts`

#### 已迁移（51 个）

| 功能 | 测试数 | 说明 |
|------|--------|------|
| `op_pi_register_tool` | 2 | 基本注册 + prompt_guidelines/execution_mode |
| `op_pi_register_command` | 2 | 带/不带 description |
| `op_pi_register_shortcut` / `get_shortcuts` | 2 | 注册 + 查询 |
| `op_pi_register_flag` / `get_flags` | 2 | 注册 + 查询 |
| `op_pi_get_commands` | 1 | 空列表 |
| `op_pi_send_message` | 1 | HostCommand 入队 |
| `op_pi_send_user_message` | 1 | HostCommand 入队 |
| `op_pi_append_entry` | 1 | HostCommand 入队 |
| `op_pi_set_session_name` | 1 | HostCommand 入队 |
| `op_pi_notify` | 2 | 单条 + 多条通知缓冲 |
| `op_pi_log` | 1 | 不崩溃验证 |
| `op_pi_emit_error` | 1 | broadcast 错误事件 |
| `op_pi_set_model` | 1 | HostCommand 入队 |
| `op_pi_set_thinking_level` | 1 | HostCommand 入队 |
| `op_pi_ctx_is_idle` | 1 | 默认 true |
| `op_pi_ctx_is_project_trusted` | 1 | 默认 true |
| `op_pi_ctx_has_pending_messages` | 1 | 默认 false |
| `op_pi_ctx_get_system_prompt` | 1 | 默认空串 |
| `op_pi_ctx_get_model` | 1 | 默认空串 |
| `op_pi_ui_set_status/working_message/title` | 3 | stub 不崩溃 |
| `op_pi_new_session/fork/switch_session` | 3 | HostCommand 入队 |
| `op_pi_reload/wait_for_idle/navigate_tree` | 3 | HostCommand 入队 |
| `op_pi_set_label` | 1 | HostCommand 入队 |
| `op_pi_get_active/all_tools` | 2 | 默认空数组 |
| `op_pi_set_active_tools` | 1 | HostCommand 入队 |
| `op_pi_register/unregister_provider` | 2 | HostCommand 入队 |
| `op_pi_ctx_abort/shutdown/compact` | 3 | HostCommand 入队 |
| `op_pi_ctx_get_context_usage` | 1 | 默认零值 |
| `PiOpState::new()` | 1 | 初始空状态 |
| serde roundtrip（4 个类型） | 4 | ToolInfo/CommandInfo/FlagOptions/ShortcutInfo |
| ExecOptionsSerde | 2 | 默认 + 转换 |
| ExecResultSerde | 1 | From 转换 |

#### 未迁移（需复杂状态）

| 功能 | 说明 |
|------|------|
| 快捷键冲突检测 | 需内置 keybindings 配置 |
| 工具同名去重 | 需多扩展加载到同一运行时 |
| 命令同名后缀编号 | 需 ExtensionRunner 层逻辑 |
| Provider bindCore 验证 | 需 ModelRegistry 集成 |

---

### 5. `types.rs` — 类型定义

**Rust 文件：** `crates/pi-coding-agent/src/core/extensions/types.rs`
**原版 TS 源：** `packages/coding-agent/src/core/extensions/types.ts`

| 测试 | Rust 状态 |
|------|-----------|
| ToolDefinition 序列化/反序列化 | ✅ 已有 1 个测试 |

---

### 6. `package_manager.rs` — 包管理器

**Rust 文件：** `crates/pi-coding-agent/src/core/package_manager.rs`
**原版 TS 源：** `packages/coding-agent/src/core/package-manager.ts`
**原版测试：** `test/package-manager.test.ts` + `test/package-manager-ssh.test.ts`

#### 已迁移（36 个）

| 测试类别 | 数量 | 说明 |
|---------|------|------|
| 基础操作 | 3 | npm 可用性、创建管理器、空目录解析 |
| 包发现 | 2 | node_modules 中检测普通包和 scoped 包 |
| Source 解析 | 4 | 基本解析、缓存命中、未找到、project-only |
| 跨作用域解析 | 3 | user scope、project scope、both scopes |
| 进度事件 | 3 | 回调触发、回调清除、ProgressEvent serde |
| 缺失源处理 | 2 | MissingSourceAction 变体、on_missing 回调 |
| 序列化 | 8 | PathMetadata（含/不含 base_dir）、ConfiguredPackage（含/不含 installed_path）、ResolvedResource（启用/禁用）、ResolvedPaths 默认、SourceScope serde、ProgressEvent serde |
| 列表配置包 | 3 | 有包、双作用域、去重（project 优先） |
| 移除操作 | 1 | 清除缓存 |
| NpmHelper 边界 | 3 | 不存在的目录、空目录、未安装的包 |
| 边界条件 | 3 | 不存在的目录、空路径、空缓存 |

#### 未迁移（需新增功能）

| 功能 | 说明 |
|------|------|
| Source 解析（npm/git/local） | Rust 未实现 `parseSource()` |
| 模式过滤（`!`/`+`/`-` 前缀） | Rust 未实现 pattern filtering |
| SSH URL 解析 | 在 `package-manager-ssh.test.ts` 中 |
| 离线模式 | Rust 未实现 offline mode |
| Git 安装路径 | Rust 未实现 git operations |

---

### 7. 其他扩展相关测试

#### `resource-loader.test.ts`（22 个测试，全部未迁移）

| 功能 | 测试数 | 说明 |
|------|--------|------|
| 初始化空结果 | 1 | reload 前状态 |
| 技能发现 | 2 | agentDir 发现、忽略多余 markdown |
| 提示词发现 | 1 | agentDir 发现 |
| 项目优先于用户 | 1 | 同名冲突时项目优先 |
| 符号链接去重 | 1 | 用户/项目同文件只加载一次 |
| 信任延迟加载 | 1 | 信任前加载用户扩展，信任后复用 |
| 命令名冲突 | 1 | 同名命令保留两个 |
| 覆盖自动发现 | 1 | override 机制 |
| AGENTS.md/CLAUDE.md 发现 | 2 | context 文件发现 |
| SYSTEM.md 发现 | 1 | `.pi` 目录发现 |
| 信任保护 | 1 | 未信任项目跳过 |
| APPEND_SYSTEM.md | 1 | 附加系统提示词 |
| 扩展资源加载 | 2 | skills/prompts 带扩展元数据、file URL |
| noSkills 选项 | 2 | 跳过技能发现 |
| override 函数 | 2 | skillsOverride、systemPromptOverride |
| 扩展冲突检测 | 2 | 工具冲突、CLI 扩展优先 |

#### `compaction-extensions.test.ts`（8 个测试，全部未迁移）

| 测试 | 说明 |
|------|------|
| before_compact + compact 事件 | 事件触发顺序 |
| 取消 compaction | extension 返回 `{ cancelled: true }` |
| 自定义 compaction | extension 提供自定义实现 |
| compact 后事件包含 entries | 事件数据完整性 |
| 错误时回退默认 | extension 抛异常时使用默认 compaction |
| 多 extension 顺序调用 | 按注册顺序调用 |
| before_compact 数据正确 | 事件 payload 字段 |
| 不同值使用 extension compaction | 配置覆盖 |

#### `plan-mode-extension.test.ts`（4 个测试，全部未迁移）

| 测试 | 说明 |
|------|------|
| 保留自定义 active tools | 切换 plan mode 时工具列表不变 |
| 无 plan 时不提示 | assistant 响应不含 plan 时跳过 |
| plan 优化排队 | 作为 follow-up user message |
| plan 执行排队 | 作为 follow-up custom message |

#### `git-merge-and-resolve-extension.test.ts`（9 个测试，全部未迁移）

| 测试 | 说明 |
|------|------|
| 非 git 仓库跳过 | 无 `.git` 目录 |
| 无 upstream 跳过 | 未配置远程 |
| 未完成 merge 重发冲突 | 检测 merge 冲突标记 |
| dirty working tree 跳过 | 非 merge 状态有未提交修改 |
| fetch 失败跳过 | 网络错误 |
| clean merge 跳过 | 无冲突 |
| 冲突报告 | 作为 follow-up 消息发送 |
| 空 ours/theirs 处理 | 无冲突内容 |
| merge 失败无冲突标记 | 不发送消息 |

---

## 关键缺口总结

### ✅ 优先级 1：V8 运行时测试（已完成，97 个测试）

`runtime.rs` + `dispatcher.rs` + `ops.rs` 三个模块已补齐测试：

- **`ops.rs`（51 个）**：全部 deno ops 通过 V8 实例验证——注册工具/命令/快捷键/flag、HostCommand 入队、通知缓冲、错误广播、stub 默认值、serde roundtrip
- **`runtime.rs`（24 个）**：V8 线程生命周期——创建/停止/重载、加载扩展（含工具/命令/多文件/错误/无 default）、调用工具（含通知）、事件分发（fire-and-forget + result-returning block）、错误事件订阅、HostCommand 轮询/清空/处理、显式路径/全局目录加载
- **`dispatcher.rs`（22 个）**：payload 构建（tool_call/tool_result）、AgentEvent → 事件名映射（全部变体）、fire-and-forget 过滤、结果类型枚举

**测试方式：**
- ops：直接创建 `JsRuntime` + `pi_extension` 扩展，执行 JS 调用 op，验证 `OpState` 状态
- runtime：通过 `ExtensionRuntime` 句柄发送命令到 V8 线程，async 等待结果
- dispatcher：纯单元测试，构造 `AgentEvent`/`BeforeToolCallContext` 验证 payload JSON

### ✅ 优先级 2：Package manager 测试（已完成，36 个测试）

`package_manager.rs` 从 7 个基础测试扩展到 36 个：

- **进度事件**：回调触发/清除、ProgressEvent serde
- **跨作用域解析**：user scope、project scope、both scopes、project 优先去重
- **Source 解析缓存**：缓存命中/清除/未找到/project-only
- **序列化**：PathMetadata（含/不含 base_dir）、ConfiguredPackage、ResolvedResource、SourceScope、ProgressEvent
- **列表配置包**：有包/双作用域/去重
- **移除操作**：缓存清除
- **NpmHelper 边界**：不存在目录/空目录/未安装
- **缺失源处理**：MissingSourceAction 变体/on_missing 回调

**未覆盖（需新增 Rust 功能）：**
- Source 解析（npm/git/local URL 解析）— Rust 未实现 `parseSource()`
- 模式过滤（`!`/`+`/`-` 前缀）— Rust 未实现 pattern filtering
- SSH URL 解析 — 在 `package-manager-ssh.test.ts` 中
- 离线模式 — Rust 未实现

### 优先级 3：Resource loader 测试（22 个测试，未完成）

`resource-loader.ts` 的 Rust 复刻尚未开始，测试也无从谈起。

### 优先级 4：Compaction/Plan mode/Git merge 扩展测试（21 个测试，未完成）

这些是特定扩展的行为测试，依赖完整的 Agent 运行时。

---

## 已修复的问题

在迁移过程中发现并修复了以下行为差异：

| 问题 | 修复 |
|------|------|
| `find_index()` 优先 `index.js` 而非 `index.ts` | 改为 `index.ts` → `index.tsx` → `index.mts` → `index.js` → `index.mjs` → `index.cjs`，与原版 TS 一致 |
| `discover_in_dir()` 不处理符号链接 | Rust 的 `is_file()`/`is_dir()` 自动 follow symlink，无需额外处理 |
| 发现路径使用 `{cwd}/extensions/` 而非 `{cwd}/.pi-rs/extensions/` | 测试 fixture 已修正为 `.pi-rs/extensions/` |
