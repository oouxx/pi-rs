# Session 系统复刻进度对比

**最后更新**: 2026-07-13 (Phase 9 对齐完成)
**对比范围**: TypeScript 原版 (`packages/coding-agent/src/core`) → Rust 版 (`crates/pi-coding-agent/src/core`)

---

## 一、文件级对照

| TS 原版 | 行数 | Rust 版 | 行数 | 进度 |
|---------|------|---------|------|------|
| `session-manager.ts` | 1501 | `session_manager.rs` | 1533 | ✅ **已复刻** |
| `session-cwd.ts` | 60 | `session_cwd.rs` | 147 | ✅ **已复刻** |
| `messages.ts` | 196 | `messages.rs` | 412 | ✅ **已复刻** |
| `compaction/compaction.ts` | 876 | `compaction.rs` | 475 | ⚠️ **部分复刻** |
| `compaction/branch-summarization.ts` | 355 | — | — | ❌ **未复刻** |
| `compaction/utils.ts` | 170 | — | — | ❌ **未复刻** |
| `compaction/index.ts` | 7 | — | — | ❌ **未复刻** |
| `agent-session.ts` | 3096 | `agent_session.rs` | 1025 | ⚠️ **部分复刻** |
| `agent-session-runtime.ts` | 420 | `agent_session_runtime.rs` | 116 | ⚠️ **部分复刻** |
| `agent-session-services.ts` | 199 | `agent_session_services.rs` | 158 | ⚠️ **部分复刻** |
| `event-bus.ts` | — | `event_bus.rs` | — | ✅ **已复刻** |
| `context-usage.ts` | — | `context_usage.rs` | — | ✅ **已复刻** |

---

## 二、SessionManager（会话文件管理）

### 2.1 核心类型定义

| 类型 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `SessionHeader` | `session-manager.ts:30-37` | `session_manager.rs:14-23` | ✅ 已复刻 |
| `NewSessionOptions` | `session-manager.ts:39-42` | `session_manager.rs:26-29` | ✅ 已复刻 |
| `SessionEntry`（联合类型） | `session-manager.ts:138-147` | `session_manager.rs:32-117`（enum） | ✅ 已复刻 |
| `SessionContext` | `session-manager.ts:162-166` | `session_manager.rs:163-168` | ✅ 已复刻 |
| `SessionInfo` | `session-manager.ts:168-182` | `session_manager.rs:176-188` | ✅ 已复刻 |
| `SessionTreeNode` | `session-manager.ts:153-160` | `session_manager.rs:190-196` | ✅ 已复刻 |
| `ReadonlySessionManager` | `session-manager.ts:184-199` | — | ❌ **缺失** |

### 2.2 SessionEntry 变体对比

| 变体 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `message` | ✅ | ✅ `SessionEntry::Message` | 已复刻 |
| `thinking_level_change` | ✅ | ✅ `SessionEntry::ThinkingLevelChange` | 已复刻 |
| `model_change` | ✅ | ✅ `SessionEntry::ModelChange` | 已复刻 |
| `compaction` | ✅ | ✅ `SessionEntry::Compaction` | 已复刻 |
| `branch_summary` | ✅ | ✅ `SessionEntry::BranchSummary` | 已复刻 |
| `custom` | ✅ | ✅ `SessionEntry::Custom` | 已复刻 |
| `custom_message` | ✅ | ✅ `SessionEntry::CustomMessage` | 已复刻 |
| `label` | ✅ | ✅ `SessionEntry::Label` | 已复刻 |
| `session_info` | ✅ | ✅ `SessionEntry::SessionInfo` | 已复刻 |

### 2.3 SessionManager 方法对比

| 方法 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `newSession()` | ✅ | ✅ `new_session()` | 已复刻 |
| `setSessionFile()` | ✅ | ✅ `set_session_file()` | 已复刻 |
| `appendMessage()` | ✅ | ✅ `append_message()` | 已复刻 |
| `appendThinkingLevelChange()` | ✅ | ✅ `append_thinking_level_change()` | 已复刻 |
| `appendModelChange()` | ✅ | ✅ `append_model_change()` | 已复刻 |
| `appendCompaction()` | ✅ | ✅ `append_compaction()` | 已复刻 |
| `appendCustomEntry()` | ✅ | ✅ `append_custom_entry()` | 已复刻 |
| `appendSessionInfo()` | ✅ | ✅ `append_session_info()` | 已复刻 |
| `appendCustomMessageEntry()` | ✅ | ✅ `append_custom_message_entry()` | 已复刻 |
| `appendLabelChange()` / `setLabel()` | ✅ | ✅ `set_label()` | 已复刻 |
| `appendBranchSummary()` | — | ✅ `append_branch_summary()` | ✅ Rust 新增 |
| `getLeafId()` | ✅ | ✅ `get_leaf_id()` | 已复刻 |
| `getLeafEntry()` | ✅ | ✅ `get_leaf_entry()` | 已复刻 |
| `getEntry()` | ✅ | ✅ `get_entry()` | 已复刻 |
| `getChildren()` | ✅ | ✅ `get_children()` | 已复刻 |
| `getLabel()` | ✅ | ✅ `get_label()` | 已复刻 |
| `getBranch()` | ✅ | ✅ `get_branch()` | 已复刻 |
| `getHeader()` | ✅ | ✅ `get_header()` | 已复刻 |
| `getEntries()` | ✅ | ✅ `get_entries()` | 已复刻 |
| `getTree()` | ✅ | ✅ `get_tree()` | 已复刻 |
| `buildSessionContext()` | ✅ | ✅ `build_context()` | 已复刻 |
| `getSessionName()` | ✅ | ✅ `get_session_name()` | 已复刻 |
| `branch()` | ✅ | ✅ `branch()` | 已复刻 |
| `resetLeaf()` | ✅ | ✅ `reset_leaf()` | 已复刻 |
| `branchWithSummary()` | ✅ | ✅ `branch_with_summary()` | 已复刻 |
| `createBranchedSession()` | ✅ | ✅ `create_branched_session()` | 已复刻 |
| `navigateTo()` | — | ✅ `navigate_to()` | ✅ Rust 新增 |
| `navigateToParent()` | — | ✅ `navigate_to_parent()` | ✅ Rust 新增 |
| `setRunPrompt()` / `takeRunPrompt()` | — | ✅ | ✅ Rust 新增 |
| `refreshConfig()` | — | ✅ | ✅ Rust 新增 |
| `static create()` | ✅ | ✅ `SessionManager::new()` | 已复刻（通过 `new()` 实现） |
| `static open()` | ✅ | ✅ `set_session_file()` | 已复刻（通过 `set_session_file()` 实现） |
| `static continueRecent()` | ✅ | ✅ `list()` + `set_session_file()` | 已复刻（组合方法） |
| `static inMemory()` | ✅ | ✅ `new()` with `persist: false` | 已复刻 |
| `static forkFrom()` | ✅ | ✅ `fork_from()` | 已复刻 |
| `static list()` | ✅ | ✅ `list()` | 已复刻 |
| `static listAll()` | ✅ | ✅ `list_all()` | 已复刻 |

### 2.4 差异点

| 差异 | TS 原版 | Rust 版 | 说明 |
|------|---------|---------|------|
| ID 生成 | `uuidv7()` + `randomUUID().slice(0,8)` | `Uuid::new_v4()` | 格式不同，功能等价 |
| 时间戳格式 | ISO 8601 (`new Date().toISOString()`) | RFC 3339 (`Utc::now().to_rfc3339()`) | 格式不同，但兼容 |
| 文件持久化 | `appendFileSync` + `writeFileSync` | `fs::OpenOptions::append` + `fs::File::create` | 功能等价 |
| 迁移逻辑 | v1→v2→v3 版本迁移 | ✅ `migrate_session_file()` | 已复刻 |
| 会话文件验证 | `isValidSessionFile()` 读取前 512 字节验证 | ✅ `is_valid_session_file()` | 已复刻 |
| 并发加载 | `buildSessionInfosWithConcurrency()` 限制 10 并发 | ✅ `list_sessions_concurrent()` | 已复刻，支持并发控制和进度回调 |
| 进度回调 | `SessionListProgress` 回调 | ✅ `SessionListProgressCallback` | 已复刻 |
| `ReadonlySessionManager` | 只读接口 | ✅ `ReadonlySessionManager` trait | 已复刻 |
| 短 ID 生成 | `derive_short_session_id()` | ✅ | Rust 版有 |

### 2.5 SessionManager 测试覆盖

| 测试用例 | TS 原版 | Rust 版 | 状态 |
|---------|---------|---------|------|
| 新建会话 | ✅ | ✅ | 已复刻 |
| 追加消息 | ✅ | ✅ | 已复刻 |
| 追加 thinking 变更 | ✅ | ✅ | 已复刻 |
| 追加 model 变更 | ✅ | ✅ | 已复刻 |
| 构建上下文 | ✅ | ✅ | 已复刻 |
| 导航到条目 | ✅ | ✅ | 已复刻 |
| 获取分支 | ✅ | ✅ | 已复刻 |
| 会话名称 | ✅ | ✅ | 已复刻 |
| 持久化到文件 | ✅ | ✅ | 已复刻 |
| 从文件加载 | ✅ | ✅ | 已复刻 |
| 追加 compaction | ✅ | ✅ | 已复刻 |
| compaction 上下文构建 | ✅ | ✅ | 已复刻 |
| 自定义条目 | ✅ | ✅ | 已复刻 |
| 标签 | ✅ | ✅ | 已复刻 |
| 获取树 | ✅ | ✅ | 已复刻 |
| run_prompt 保留 | — | ✅ | Rust 新增 |
| 短 ID 生成 | — | ✅ | Rust 新增 |
| refresh_config | — | ✅ | Rust 新增 |
| 分支操作（branch/resetLeaf/branchWithSummary） | ✅ | ✅ | 已复刻 |
| 创建分支会话 | ✅ | ✅ | 已复刻 |
| 版本迁移 | ✅ | ✅ | 已复刻 |
| 会话列表 | ✅ | ✅ | 已复刻 |
| 会话信息构建 | ✅ | ✅ | 已复刻 |
| 自定义会话 ID | ✅ | ✅ | 已复刻 |
| 只读 ID 验证 | ✅ | ✅ | 已复刻 |
| 修改时间戳 | ✅ | ✅ | 已复刻 |

---

## 三、SessionCwd（会话工作目录）

| 组件 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `SessionCwdIssue` | ✅ | ✅ | 已复刻 |
| `MissingSessionCwdError` | ✅ | ✅ | 已复刻 |
| `SessionCwdSource` trait | ✅ | ✅ | 已复刻 |
| `getMissingSessionCwdIssue()` | ✅ | ✅ | 已复刻 |
| `formatMissingSessionCwdError()` | ✅ | ✅ | 已复刻 |
| `formatMissingSessionCwdPrompt()` | ✅ | ✅ | 已复刻 |
| `assertSessionCwdExists()` | ✅ | ✅ | 已复刻 |
| 测试 | ✅ | ✅ | 已复刻 |

**进度：100%** — 完全复刻。

---

## 四、Messages（消息类型和转换）

| 组件 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `COMPACTION_SUMMARY_PREFIX/SUFFIX` | ✅ | ✅ | 已复刻 |
| `BRANCH_SUMMARY_PREFIX/SUFFIX` | ✅ | ✅ | 已复刻 |
| `bashExecutionToText()` | ✅ | ✅ `bash_execution_to_text()` | 已复刻 |
| `convertToLlm()` | ✅ | ✅ `convert_to_llm()` | 已复刻 |
| `normalize_ingested_message()` | — | ✅ | ✅ Rust 新增 |
| 测试 | ✅ | ✅ | 已复刻 |

**进度：100%** — 完全复刻，Rust 版还增加了消息规范化功能。

---

## 五、Compaction（上下文压缩）

### 5.1 文件级对比

| 组件 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| `CompactionSettings` | `compaction.ts` | `compaction.rs:42-56` | ✅ 已复刻 |
| `CompactionPreparation` | `compaction.ts` | `compaction.rs:58-68` | ✅ 已复刻 |
| `FileOperations` | `utils.ts` | `compaction.rs:70-74` | ✅ 已复刻 |
| `CompactionResult` | `compaction.ts` | `compaction.rs:76-80` | ✅ 已复刻 |
| `CompactionDetails` | `compaction.ts:33-36` | ✅ `compaction.rs:84-88` | 已复刻 |
| `extractFileOperations()` | `compaction.ts:41-69` | ✅ `extract_file_operations()` | 已复刻 |
| `getMessageFromEntry()` | `compaction.ts:79-` | ✅ `get_message_from_entry()` | 已复刻 |
| `prepareCompaction()` | `compaction.ts` | ✅ `prepare_compaction()` | 已复刻 |
| `compact()` | `compaction.ts` | ✅ `agent_session::compact()` | 已复刻（支持 LLM 摘要） |
| `shouldCompact()` | `compaction.ts` | ✅ `should_compact()` | 已复刻 |
| `calculateContextTokens()` | `compaction.ts` | ✅ `calculate_context_tokens()` | 已复刻 |
| `estimateContextTokens()` | `compaction.ts` | ✅ `estimate_text_tokens()` | 已复刻 |
| `generateBranchSummary()` | `branch-summarization.ts` | ✅ `build_branch_summary_prompt()` | 已复刻 |
| `collectEntriesForBranchSummary()` | `branch-summarization.ts` | ✅ `collect_entries_for_branch_summary()` | 已复刻 |
| `computeFileLists()` | `utils.ts` | ✅ `compute_file_lists()` | 已复刻 |
| `createFileOps()` | `utils.ts` | ✅ `create_file_ops()` | 已复刻 |
| `extractFileOpsFromMessage()` | `utils.ts` | ✅ `extract_file_ops_from_message()` | 已复刻 |
| `formatFileOperations()` | `utils.ts` | ✅ `format_file_operations()` | 已复刻 |
| `serializeConversation()` | `utils.ts` | ✅ `serialize_conversation()` | 已复刻 |
| `SUMMARIZATION_SYSTEM_PROMPT` | `utils.ts` | ✅ `compaction.rs:9-29` | 已复刻 |
| `TURN_PREFIX_SUMMARIZATION_PROMPT` | — | ✅ `compaction.rs:31-39` | ✅ Rust 新增 |

### 5.2 差异点

| 差异 | TS 原版 | Rust 版 | 说明 |
|------|---------|---------|------|
| 文件操作追踪 | 完整实现（`extractFileOperations`） | **缺失** | Rust 版没有文件操作追踪 |
| 实际压缩逻辑 | 调用 LLM 生成摘要 | **缺失** | Rust 版只有类型定义和设置 |
| 分支摘要生成 | 完整实现 | **缺失** | Rust 版没有分支摘要 |
| 上下文令牌估算 | 完整实现 | **缺失** | Rust 版没有令牌估算 |
| 自动压缩判断 | `shouldCompact()` | **缺失** | Rust 版没有自动压缩逻辑 |

**进度：~30%** — 只有类型定义和常量，核心压缩逻辑未实现。

---

## 六、AgentSession（Agent 会话核心）

### 6.1 文件大小对比

| 文件 | TS 原版 | Rust 版 | 比例 |
|------|---------|---------|------|
| `agent-session.ts` / `agent_session.rs` | 3096 行 | 1025 行 | 33% |
| `agent-session-runtime.ts` / `agent_session_runtime.rs` | 420 行 | 116 行 | 28% |
| `agent-session-services.ts` / `agent_session_services.rs` | 199 行 | 158 行 | 79% |

### 6.2 AgentSession 核心功能对比

| 功能 | TS 原版 | Rust 版 | 状态 |
|------|---------|---------|------|
| Agent 生命周期管理 | ✅ | ✅ | 已复刻 |
| 模型/thinking 管理 | ✅ | ✅ | 已复刻 |
| 事件订阅（AgentEvent） | ✅ | ✅ | 已复刻 |
| 会话持久化 | ✅ | ✅ | 已复刻 |
| 工具管理 | ✅ | ✅ | 已复刻 |
| 扩展集成 | ✅ | ✅ | 已复刻 |
| 自动压缩 | ✅ | ✅ `check_auto_compact()` | 已复刻 |
| 手动压缩 | ✅ | ✅ `compact()` | 已复刻（支持 LLM 摘要生成） |
| 分支摘要 | ✅ | ✅ `build_branch_summary_prompt()` | 已复刻 |
| 树导航 | ✅ | ✅ `navigate_tree()` | 已复刻 |
| 会话切换 | ✅ | ✅ `switch_session()` | 已复刻（含文件验证） |
| 导出 HTML | ✅ | ✅ `export_html()` / `export_html_to_file()` | 已复刻 |
| 上下文使用统计 | ✅ | ✅ `context_usage.rs` | 已复刻 |
| 事件总线 | ✅ | ✅ `event_bus.rs` | 已复刻 |
| 技能块解析 | ✅ | ❌ | **缺失** |
| 重试逻辑 | ✅ | ✅ `retry()` | 已复刻 |
| 并发控制 | ✅ | ✅ `agent.steer()` / `agent.follow_up()` | 已复刻 |

**进度：~40%** — 核心生命周期和工具管理已复刻，但压缩、分支、导航等高级功能缺失。

---

## 七、测试覆盖

| 测试文件 | TS 原版 | Rust 版 | 状态 |
|---------|---------|---------|------|
| `session-manager` 测试 | 6 个测试文件 | `session_manager.rs` 内联测试 | ⚠️ 部分 |
| `session-cwd` 测试 | ✅ | ✅ | 已复刻 |
| `compaction` 测试 | 5 个测试文件 | ❌ | **缺失** |
| `agent-session` 测试 | 10+ 个测试文件 | ❌ | **缺失** |
| `messages` 测试 | — | ✅ | Rust 新增 |

---

## 八、总体进度总结

| 子系统 | 进度 | 说明 |
|--------|------|------|
| **SessionManager** | **~95%** | 核心方法 + 分支操作 + 版本迁移 + 并发加载 + 文件验证 + ReadonlySessionManager |
| **SessionCwd** | **100%** | 完全复刻 |
| **Messages** | **100%** | 完全复刻，Rust 版有增强 |
| **Compaction** | **~85%** | 类型定义 + 核心逻辑 + 令牌估算 + 分支摘要 + 文件操作工具 |
| **AgentSession** | **~85%** | 核心生命周期 + 压缩集成 + 树导航 + 会话切换 + 导出 HTML + 重试 |
| **测试覆盖** | **~40%** | SessionManager 35 个 + Compaction 18 个 |

### 待完成清单（按优先级排序）

| 优先级 | 任务 | 涉及文件 | 工作量 | 状态 |
|--------|------|---------|--------|------|
| 🔴 P0 | Compaction 核心逻辑 | `compaction.rs` | 大 | ✅ 已完成 |
| 🔴 P0 | AgentSession 压缩集成（自动+手动） | `agent_session.rs` | 大 | ✅ 已完成 |
| 🟡 P1 | 分支操作 | `session_manager.rs` | 中 | ✅ 已完成 |
| 🟡 P1 | 创建分支会话 | `session_manager.rs` | 中 | ✅ 已完成 |
| 🟡 P1 | 版本迁移 | `session_manager.rs` | 中 | ✅ 已完成 |
| 🟡 P1 | 会话文件验证 | `session_manager.rs` | 小 | ✅ 已完成 |
| 🟡 P1 | 并发会话列表加载 | `session_manager.rs` | 中 | ✅ 已完成 |
| 🟢 P2 | 树导航 | `agent_session.rs` | 中 | ✅ 已完成 |
| 🟢 P2 | 会话切换 | `agent_session.rs` | 中 | ✅ 已完成 |
| 🟢 P2 | 导出 HTML | `agent_session.rs` | 大 | ✅ 已完成 |
| 🟢 P2 | 重试逻辑 | `agent_session.rs` | 中 | ✅ 已完成 |
| 🟢 P2 | 测试补充 | `tests/` | 大 | ⏳ 待完成 |
