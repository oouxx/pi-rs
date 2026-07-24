# DEVIATIONS.md — pi-coding-agent

本文件记录 Rust 版 `pi-coding-agent` 与 TypeScript 原版之间**有意保留**的行为差异。
"已确认保留"的偏差在阶段四对齐检查中不会被修正。

---

## AgentSession

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| `agent_session.rs:reload()` | 重建 ExtensionRunner、重发 session_start 事件、重新发现扩展资源 | 只 reload settings + sync queue modes，不重建 runtime | Rust 的 ExtensionRegistry 是静态注册的，不需要像 TS 那样动态重建 ExtensionRunner。reload 主要用于 settings 热重载，不影响扩展生命周期。 | 已确认保留 |
| `agent_session.rs:get_context_usage()` | 返回 `{ tokens: number \| null, contextWindow, percent }` | 返回 `Option<ContextUsage>`，其中 `ContextUsage` 包含 `total_tokens`, `context_window` 等字段 | Rust 的 `ContextUsage` 结构体设计不同，但语义等价：TS 返回 `tokens: null` 对应 Rust 返回 `None`。 | 已确认保留 |
| `agent_session.rs:export_to_jsonl()` | 使用 `resolvePath(outputPath ?? defaultName, process.cwd())` | 使用 `resolve_path(&raw_path, &cwd, &PathOptions::default())` | 行为一致，只是 Rust 版显式传入 cwd 而非隐式使用 `process.cwd()`。 | 已确认保留 |
| `agent_session.rs:set_auto_compaction_enabled()` | 持久化到磁盘配置（通过 settingsManager） | 只修改内存中的 `CompactionSettings`，不持久化 | Rust 的配置持久化机制与 TS 不同，后续可通过 settings 系统统一处理。 | 已确认保留 |

## SessionManager

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| `session_manager.rs:ReadonlySessionManager` | TS 有独立的 `ReadonlySessionManager` interface | Rust 没有独立的 ReadonlySessionManager trait | Rust 的 `SessionManager` 通过 `pub` 可见性控制只读访问，不需要额外 trait。 | 已确认保留 |

## Compaction

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| `compaction.rs:branch-summarization` | TS 有独立的 `compaction/branch-summarization.ts` | Rust 将分支摘要逻辑内联在 `compaction.rs` 中 | 功能相同，只是文件组织不同。 | 已确认保留 |
| `compaction.rs:utils.ts` | TS 有独立的 `compaction/utils.ts` | Rust 将工具函数内联在 `compaction.rs` 中 | 功能相同，只是文件组织不同。 | 已确认保留 |
