# pi-extensions 偏差日志

> 与 `@narumitw/pi-goal` v0.52.2（及 pi-extension-api / pi-coding-agent 接线）
> 的行为差异登记。状态：`已确认保留` = 禁止在"对齐"名义下修改；
> `待确认` = 尚未与用户确认，属于已知差异。
>
> **2026-08-24 简化（用户确认，对齐 codex /goal 风格）**：per-goal 预算
> 限制整体删除、TUI 菜单/设置 UI 整体删除、自动轮数限制删除、设置文件
> 删除、tokens_used 统计删除。以下相关条目随简化一并作废。

## goal 扩展

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | ------------- | -------- | -------- |
| goal.rs 整体 | per-goal 预算：`--tokens`、`token_budget`、`BudgetLimited` 状态、`limit_for_budget`、prompt 预算行、resume/edit 预算 guard、budget wrap-up | **无预算**：5 态状态机（active/paused/blocked/usage_limited/complete），无 `--tokens`，prompt 无预算行 | 用户确认简化，对齐 codex（codex 的 budget-limited 是全局 token/cost 预算，非 per-goal） | 已确认保留 |
| goal.rs 整体 | TUI 菜单（menu.ts）+ 设置 UI（settings-ui.ts）：/goal 无参数开菜单、Settings… 菜单、预算选择屏 | **无 UI**：/goal 无参数直接显示文本状态；`pi-goal.json` 设置文件删除，无进展阈值硬编码 `NO_PROGRESS_TURNS=3` | 用户确认简化，对齐 codex（codex 无菜单，仅状态栏） | 已确认保留 |
| goal.rs 整体 | 自动轮数上限（automaticTurns 25，turn_end 计数 + 达上限暂停 abort） | **无自动轮数限制**：`automatic_model_turns` 计数、turn_end 计数、`continuation_limit` 暂停原因全部删除 | 用户确认简化，对齐 codex（codex 无自动轮数上限，只有无工具延续抑制） | 已确认保留 |
| goal.rs 整体 | token 会计（tokens_used / baseline_tokens / 状态栏用量显示） | **无 token 统计**：`tokens_used`、`baseline_tokens`、`format_token_count` 删除 | 用户确认简化 | 已确认保留 |
| goal.rs 工具注册 | 工具按 `toolVisibility` 默认隐藏、首次 /goal 后 reveal、不可用时 pause | 工具始终注册；未激活/无 goal 时工具按验证链拒绝 | pi-rs 无动态工具 allowlist 通道的等价物 | 已确认保留 |
| goal.rs `handle_tool_call` | `session_start/shutdown/before_compact/compact`、`message_start`、`context`、`tool_execution_end` 全部接线，每个事件 handler 带 ctx；有 post-compact 事件 | 只实现 agent_start/agent_end/settled/input/before_agent_start/before_tool_call；session_start（无 ctx）、compact（无 ctx）等未接线；无 post-compact 事件 | pi-extension-api 事件签名不携带 ctx / pi-rs 无 post-compact 事件 | 已确认保留 |
| goal.rs prompt 所有权 | prompt marker 所有权机制：pending/claimed/cancelled marker、非 goal 输入队列、`preservesOwnedPromptAtTerminalBoundary`、stale-owned-prompt 消费 | 无 marker 机制；`pending_run` 同步入队 + `before_agent_start` 按"goal active 即归属"兜底 | 简化：Rust 事件同步串行，无 TS 的异步竞争窗口 | 已确认保留 |
| goal.rs wait 到期分发 | `GoalWaitTimer` 到期直接 `dispatchDueGoalWait`，带 `ctx.isIdle()` 检查 + 失败 1s 重试一次 + 两次后 exhausted | waker 到期直接 dispatch（无 idle 检查）；无重试/exhausted | RuntimeHandle 无 is_idle/pending-messages API | 已确认保留 |
| goal.rs `/goal status`（print/json 模式） | `reportGoalStatus` 在 print/json 模式抛错 | 发 warning 通知（含原版错误文案） | Rust 命令注册无错误通道（execute 返回 ()） | 已确认保留 |
| goal.rs legacy 队列 | 支持 legacy queue（`goals-state` entry + 警告 + 迁移提示） | 不支持 | 多目标队列已从原版主线移除 | 已确认保留 |
| goal.rs retryable 判定 | retryable 错误含 context-overflow 判定 + pi-ai `isRetryableAssistantError`（5 条 pattern） | 只有 4 条文本 pattern | session 层 retry 兜底 context overflow | 已确认保留 |
| goal.rs fingerprint | sha256(NFKC 归一 + 去全部 Cc/Cf + \s 折叠) | DefaultHasher（无 NFKC、仅 ASCII 标点判断、部分 format 字符） | 依赖面最小化；仅影响全角字符的"无进展指纹"判定 | 已确认保留 |

## 基础设施（pi-extension-api / pi-coding-agent 接线）

| 位置 | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---- | ---------- | ------------- | -------- | -------- |
| HookHandler::before_tool_call | 带 ctx（可 abort） | 已带 ctx（A3/A4 修复） | 已对齐 | — |
| HookHandler::on_turn_end | 带 ctx | 已带 ctx（B1 修复时加）；goal 扩展已不再使用（自动轮数删除），API 保留供其他扩展 | 已对齐 | — |
| RuntimeHandle.abort | 接线到 agent.abort | 已接线（spawn 任务调 agent.abort，不 wait_for_idle） | 同步 Fn 不能 await | 已确认保留 |
| session_start 等事件 ctx | 全部带 ctx | 部分不带（on_session_start/on_turn_start/on_message_start 等） | 未改动 | 待确认 |
