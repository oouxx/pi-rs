# pi-extensions 偏差日志

> 与 `@narumitw/pi-goal` v0.52.2（及 pi-extension-api / pi-coding-agent 接线）
> 的行为差异登记。状态：`已确认保留` = 禁止在"对齐"名义下修改；
> `待确认` = 尚未与用户确认，属于已知差异。

## goal 扩展

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | ------------- | -------- | -------- |
| goal.rs 工具注册 | 工具按 `toolVisibility` 默认隐藏、首次 /goal 后 reveal、不可用时 pause | 工具始终注册；`toolVisibility` 只影响显示语义；未激活/无 goal 时工具按验证链拒绝 | pi-rs 无动态工具 allowlist 通道的等价物；已登记于头注释 | 已确认保留 |
| goal.rs `handle_tool_call` | `session_start/shutdown/before_compact/compact`、`message_start`、`context`、`tool_execution_end` 全部接线（预算 wrap-up steer、compaction 重试、guardAbortGoalId），每个事件 handler 带 ctx；有 post-compact 事件 | 只实现 agent_start/turn_end/agent_end/settled/input/before_agent_start/before_tool_call；session_start（无 ctx）、compact（无 ctx）等未接线；无 post-compact 事件 | pi-extension-api 事件签名不携带 ctx / pi-rs 无 post-compact 事件；compaction 的 continuation 重试由 session 层自行处理 | 已确认保留 |
| goal.rs prompt 所有权 | prompt marker 所有权机制：pending/claimed/cancelled marker、非 goal 输入队列、`preservesOwnedPromptAtTerminalBoundary`、stale-owned-prompt 消费、`pi-goal-prompt:` marker + 指纹防伪 | 无 marker 机制；`pending_run` 同步入队 + `before_agent_start` 按"goal active 即归属"兜底；用户拼造 marker 文本可能被误判为 owned（无指纹校验） | 简化：Rust 事件是同步串行的，无 TS 的异步竞争窗口；伪造 marker 的后果仅是 run 被记入 goal 用量 | 已确认保留 |
| goal.rs token 会计 | 整会话分支 assistant tokens − baseline（用户消息轮也计入 goal 用量） | 只累计 goal-owned run 的 assistant usage 增量（非 goal run 不记账） | 增量累计近似（已登记于头注释）；B4 | 已确认保留 |
| goal.rs budget wrap-up | 预算耗尽走 steer 自定义消息（`BUDGET_WRAP_UP_MESSAGE_TYPE`）+ delivered 标记；**wrap-up 期间允许 goal_complete**（status 非 active 也可） | 只发一条 notify（含 wrap-up 文本）+ 状态 budget_limited；goal_complete 在 budget_limited 一律拒绝 | Rust `RuntimeHandle` 无 custom-message 通道（send_message 只收字符串）；complete-during-wrap-up 依赖该通道，暂不实现 | 已确认保留 |
| goal.rs retryable 判定 | retryable 错误含 context-overflow 判定 + pi-ai `isRetryableAssistantError`（5 条 pattern） | 只有 4 条文本 pattern | session 层 retry 兜底 context overflow；已登记于头注释 | 已确认保留 |
| goal.rs fingerprint | sha256(NFKC 归一 + 去全部 Cc/Cf + \s 折叠) | DefaultHasher（无 NFKC、仅 ASCII 标点判断、部分 format 字符） | 依赖面最小化；仅影响全角字符的"无进展指纹"判定 | 已确认保留 |
| goal.rs wait 到期分发 | `GoalWaitTimer` 到期直接 `dispatchDueGoalWait`，带 `ctx.isIdle()` 检查 + 失败 1s 重试一次 + 两次后 exhausted | waker 到期直接 dispatch（无 idle 检查）；无重试/exhausted（目标已非 waiting 时直接放弃，状态一致） | RuntimeHandle 无 is_idle/pending-messages API；Rust 下 dispatch 失败即状态已变，重试无意义 | 已确认保留 |
| goal.rs `/goal status`（print/json 模式） | `reportGoalStatus` 在 print/json 模式抛错 | 发 warning 通知（含原版错误文案） | Rust 命令注册无错误通道（execute 返回 ()） | 已确认保留 |
| goal.rs legacy 队列 | 支持 legacy queue（`goals-state` entry + 警告 + 迁移提示） | 不支持 | 已登记于头注释；多目标队列已从原版主线移除 | 已确认保留 |
| goal.rs `C13` wrap-up 期间 complete | budget_limited + wrap-up delivered 时允许 goal_complete | 一律拒绝 | 依赖 custom-message 通道（见上） | 待确认 |
| goal.rs `C6` status 模式抛错 | print/json 抛错 | warning 通知 | 无错误通道 | 待确认 |
| goal.rs `B4` 非 goal run 记账 | 用户消息轮计入 goal 用量 | 不计入（只记 goal-owned run） | 增量累计近似 | 待确认 |
| goal.rs `A2` marker 指纹防伪 | pending prompt 指纹 + 边界匹配 | 无 | 简化（见上） | 待确认 |
| goal.rs `D4` legacy queue | 支持 | 不支持 | 已确认保留（头注释） | 已确认保留 |

## 基础设施（pi-extension-api / pi-coding-agent 接线）

| 位置 | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---- | ---------- | ------------- | -------- | -------- |
| HookHandler::before_tool_call | 带 ctx（可 abort） | 已带 ctx（本次修复 A3/A4） | 已对齐 | — |
| HookHandler::on_turn_end | 带 ctx | 已带 ctx（本次修复 B1） | 已对齐 | — |
| RuntimeHandle.abort | 接线到 agent.abort | 已接线（spawn 任务调 agent.abort，不 wait_for_idle） | 同步 Fn 不能 await；abort 后由 agent_end 事件自然收尾 | 已确认保留 |
| session_start 等事件 ctx | 全部带 ctx | 部分不带（on_session_start/on_turn_start/on_message_start 等） | 未改动 | 待确认 |
