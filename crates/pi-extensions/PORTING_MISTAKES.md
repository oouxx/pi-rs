# pi-extensions 移植错误归档

> 对齐检查（GOAL_TS_COMPARISON.md）中修复的回归 bug。根因模式尽量归到
> PORTING.md 高危陷阱表的分类；新出现的模式已同步回 PORTING.md。

| 位置 | 现象 | 根因模式 | 修复方式 |
| ---- | ---- | -------- | -------- |
| goal.rs resume / edit / on_input | B2：resume 后自动轮数计数仍 ≥25，下一轮 agent_end 立刻再次暂停（resume 无效）；edit/用户输入同样不清零 | 语义遗漏（resetGoalSafetyEpoch / resetActiveSafetyEpoch 未移植） | resume/edit 转 active 时 `reset_safety_epoch()`；用户输入（非 extension/非 /goal）时对 active goal 重置并持久化 |
| goal.rs agent_end 计数 | B1：automatic_model_turns 每个 run 只加 1，而原版每个模型响应（turn）加 1；含 N 次工具调用的 run 差 N 倍；上限语义（25 个 run vs 25 次模型调用）不同 | 计数单位错误（run vs turn） | 新增 `on_turn_end_impl`：自动 run 的每个 assistant 响应计数 + 达上限安全暂停 + abort；agent_end 不再计数；dispatch_continuation 发送前复查上限 |
| goal.rs update_no_progress | B3：手动 run（用户消息）也推高无进展计数，可能误暂停 | 条件遗漏（原版仅 automatic run 调用 recordAutomaticRunProgress） | `run.origin == Automatic` 才更新无进展计数 |
| goal.rs resume | C1：预算耗尽的 goal 可直接 resume，浪费一轮后 agent_end 才转 budget_limited | guard 遗漏（transition 的预算检查被硬置 Active 绕过） | resume 前检查 `tokens_used >= token_budget` 拒绝（状态不变） |
| goal.rs resume prompt | C2：blocked/usage/budget resume 的 prompt 都写 "paused /goal" | 硬编码（未用 stoppedStatus） | `build_resume_prompt(goal, stopped_status)` + 通知用 stoppedStatusLabel |
| goal.rs resume / 菜单 | C3：waiting goal 的 resume 被判 "Goal is already active" no-op | 分支遗漏（resumeWaitingGoal 未移植） | resume 先判 waiting → `resume_waiting_goal`（清 wait + buildWaitingResumePrompt + owned prompt）；菜单复用同路径 |
| goal.rs edit_goal | C4：一律置 Active 发 prompt，绕过预算 guard、不清 waiting、不取消 continuation/waker、无状态保持 | 状态机简化过度 | 按 editedGoalStatus 保持 paused/blocked/usage_limited；budget_limited 编辑过预算 guard；清 wait/continuation/recovery；非 active 不发 prompt |
| goal.rs clear | C5：持久化 complete 占位、不更新状态栏、无 goal 不清、不清 waker（waiting goal 被 clear 后旧 waker 仍到期发消息） | 状态清理遗漏 | 持久化 `{goal: null}`、清状态栏、cancel_continue_work + 清 recovery + 清 stale；无 goal 也清 |
| goal.rs goal_complete | C9：setStatus("complete") 永久残留 | 定时器遗漏 | `schedule_completion_status_clear`（8s 后清空，状态更新时取消旧 timer） |
| goal.rs 工具 details | C10：goalId/repeatedTurns/resumeAfterMs camelCase，破坏公开工具输出契约（LLM 读取 details shape） | 契约偏差（snake_case 未对齐） | 全改 snake_case：goal_id/repeated_turns/resume_after_ms/resume_at/requested_resume_after_ms（仅钳制时），字段按原版截断 |
| goal.rs goal_blocked | C12：terminalReason 存 "{reason} (evidence: {evidence})" | 拼接过度 | terminalReason = reason（原版 blocker_report） |
| goal.rs stale 拦截 | A3：`stale_blocked` 只写不读（死代码）；暂停/阻塞后模型仍可调任意工具烧钱 | 死代码 + hook 无 ctx | `before_tool_call`（新增 ctx 参数）在 stale 且 goal 停止时 Cancel 所有工具 + abort；goal 恢复时清 flag |
| goal.rs 停止路径 | A4：pause/budget_limit/safety_pause/agent_interruption 从不 abort，当前 run 跑完才停 | 行为遗漏（abortCurrentTurn 未移植） | 各停止路径调用 `(ctx.runtime.abort)()`；pi-coding-agent 接线 RuntimeHandle.abort → agent.abort |
| goal.rs wait waker | B6：到期先 queue 一条裸 follow-up（非 owned），多烧一轮模型调用后才续发 owned continuation | 触发方式错误 | waker 到期直接 `dispatch_due_wait`（清 wait + owned continuation） |
| goal.rs 长度校验 | C15：用 `chars().count()`（Unicode 标量），原版用 `.length`（UTF-16 code units），emoji 等非 BMP 字符长度口径不同 | 长度口径错误 | `utf16_len()` / `truncate_utf16()` 全面替换 |
| goal.rs checkpoint_active_time | B9：`u64` 整除累加（每 checkpoint 截断亚秒）；恢复时 activeStartedAt 不重置 | 数值截断（高危陷阱表"数值截断"） | 改 `f64` 浮点累加（毫秒/1000）；normalize_loaded_goal 恢复时重设时钟起点 |
| goal.rs 设置保存 | D2：直接覆写文件，丢失未知字段；文件损坏时静默覆写 | 覆盖式写入 | 读取现有文件合并（保留未知字段/嵌套子字段），损坏时拒绝保存并报错 |
| goal.rs 设置读取 | D1：无效设置静默回退默认，无提示 | 静默 fallback（禁止） | 返回 issue，惰性加载时有 ctx 时 notify 一次 "pi-goal settings ignored" |
| goal.rs 状态恢复 | D3：serde 整体解析，任何字段非法 → 整个 goal 丢弃 | 解析粒度错误 | 逐字段归一化（计数 ≥0、指纹 64-hex、waiting 仅 active、activeStartedAt 重置 now、complete 丢弃） |
| goal.rs 设置缓存 | D5：扩展生命周期内缓存一次，不随 session_start 重读 | 生命周期错误 | on_session_start 作废设置缓存 |
| goal.rs agent_end 无进展持久化 | B3 附带：非自动 run 的 no-progress 更新前未先 set_goal | 顺序问题 | 更新后先 set_goal 再 enforce |
| goal.rs 规则文案 | E：goal_mode_rules 缺 3 条（fresh blocker audit、Never use it merely、resume_after_ms bounded、ordinary unfinished work），预算行多空行 | 文案偏差 | 全 14 条对齐；`\nToken budget: X.` 紧贴 block；新增 buildWaitingResumePrompt |

## 简化删除记录（2026-08-24，用户确认，对齐 codex /goal 风格）

以下条目是上一轮对齐修复的成果，随后因用户确认的简化（无 per-goal 预算、
无 UI、无自动轮数、无设置文件、无 token 统计）被整体删除，**不是回归**：

| 位置 | 原修复 | 简化后状态 |
| ---- | ------ | ---------- |
| goal.rs resume/edit 预算 guard（C1/C4） | 预算耗尽拒绝 | 随预算删除 |
| goal.rs limit_for_budget / BudgetLimited / BUDGET_WRAP_UP_PROMPT | budget_limited 状态机 | 随预算删除 |
| goal.rs 自动轮数（B1） | turn_end 计数 + 上限暂停 | 随自动轮数删除 |
| goal.rs 设置文件（D1/D2/D5） | 合并保存/issue 通知/session 重读 | 随设置文件删除 |
| goal.rs token 会计（B4） | 增量累计近似 | 随 tokens_used 删除 |
| goal.rs TUI 菜单/设置 UI | menu.ts / settings-ui.ts 移植 | 随 UI 删除 |
