# pi-extensions 契约级对齐表

> 针对 goal 扩展公开 API（工具、命令、状态机事件）的行为对照。
> 依据：`@narumitw/pi-goal` v0.52.2 源码 + GOAL_TS_COMPARISON.md。
> 复核日期：2026-08-24（对齐修复后）。"否"的差异必须引用 DEVIATIONS.md。

## 工具契约

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| goal_complete 无 goal | 拒绝 "no active goal" + notify + details | 同 | 是 | |
| goal_complete stale goal_id | 拒绝（缺失/过长/不匹配） | 同（UTF-16 长度口径） | 是 | |
| goal_complete 非 active | 拒绝 "goal is X, not active" | 同（budget_limited 也拒） | 是 | wrap-up 期间除外，见 DEVIATIONS.md C13 |
| goal_complete 矛盾 summary | 拒绝（3 条 pattern，lookbehind 版） | 同（等价实现无 lookbehind） | 是 | |
| goal_complete 成功 | transition complete + setCompletionSummary + persist + setStatus("complete") + clearActiveGoal + showCompletionStatus（8s 后清） + terminate | 同（8s timer 清空状态栏） | 是 | |
| goal_complete details | `{goal, goal_id, summary}`（截断） | 同（snake_case + UTF-16 截断） | 是 | |
| goal_blocked 校验链 | no goal / not owner / stale id / not active / reason / evidence / repeated_turns（≥3 整数） | 同 | 是 | |
| goal_blocked 成功 | stopActiveGoal(blocker_report) + terminalReason = reason + notify + terminate | 同 | 是 | |
| goal_blocked details | `{goal, goal_id, reason, evidence, repeated_turns}` | 同 | 是 | |
| goal_wait 校验链 | no goal / not owner / stale id / not active / already waiting / reason / resume_after_ms 范围 | 同 | 是 | |
| goal_wait 钳制 | <10s 钳到 10s；details `requested_resume_after_ms` 仅钳制时出现、`resume_after_ms`、`resume_at` | 同 | 是 | |
| goal_wait 拒绝 | reject + notifyTerminal 警告 | 同（C11 修复） | 是 | |
| goal_wait 持久化 | 只持久化 `{reason, resumeAt}` | 同（requested_ms 仅内存） | 是 | |
| 工具注册 | toolVisibility 控制隐藏/显现 | 始终注册 | 否（有意偏差） | DEVIATIONS.md（A6，已确认保留） |

## 命令契约（/goal）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| /goal 启动 | 确认替换 → createGoal → owned prompt → 通知（预算/自动轮数说明） | 同 | 是 | |
| /goal pause | 仅 active 可暂停 + abort 当前 turn + 通知 | 同（A4 修复） | 是 | |
| /goal resume（stopped） | 预算 guard → 轮换 id → 转 active → reset 安全纪元 → resume prompt（真实 stoppedStatus）→ 通知 | 同（C1/C2/B2 修复） | 是 | |
| /goal resume（waiting） | resumeWaitingGoal：清 wait + 恢复时钟 + buildWaitingResumePrompt + owned prompt | 同（C3 修复） | 是 | |
| /goal resume（active） | "Goal is already active." | 同 | 是 | |
| /goal edit | 状态保持（paused/blocked/usage_limited）；budget_limited 过预算 guard；清 wait/continuation/recovery；active 才发 prompt + 重置纪元；通知 "Goal updated" | 同（C4/B2 修复） | 是 | |
| /goal clear | 清 waker/continuation/recovery/stale；持久化 {goal:null}；清状态栏；无 goal 也清 | 同（C5 修复） | 是 | |
| /goal status | 先记用量+持久化+刷新状态栏；print/json 模式抛错 | 同（模式改为 warning 通知） | 否 | DEVIATIONS.md C6（无命令错误通道） |
| 菜单 waiting Resume | 显示 Resume → resumeWaitingGoal | 同（复用 resume 路径） | 是 | |

## 状态机/事件契约

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 自动轮数计数 | 每次 turn_end（模型响应）加 1，aborted 不计 | 同（B1 修复） | 是 | |
| 自动轮数上限 | 达上限 turn_end 安全暂停 + abort；settled 分发前复查 | 同（B1 修复） | 是 | |
| 无进展检测 | 仅 automatic run 计数；同指纹 +1，否则重置 1；达上限暂停（不 abort） | 同（B3 修复） | 是 | |
| 无进展指纹 | sha256(NFKC 归一) | DefaultHasher（无 NFKC） | 否 | DEVIATIONS.md C16（已确认保留） |
| resume/edit/用户输入 | 重置安全纪元（自动轮数/无进展/指纹/原因） | 同（B2 修复） | 是 | |
| 用户输入唤醒 waiting | 清 wait → run 归 goal（manual）→ agent_end 记 usage + continuation | 同（B5 修复） | 是 | |
| wait 到期 | 直接 dispatchDueGoalWait（owned continuation） | 同（B6 修复） | 是 | idle 检查/重试见 DEVIATIONS.md（已确认保留） |
| agent_end 已排 continuation | 跳过 iteration+1 | 同（B7 修复） | 是 | |
| 预算耗尽 | budget_limited + abort + 通知 + steer wrap-up | budget_limited + abort + 通知 | 否 | DEVIATIONS.md C13（wrap-up 通道，已确认保留） |
| 暂停/阻塞后工具调用 | stale 拦截 block 所有工具 + abort | 同（A3 修复） | 是 | |
| 停止路径 abort | pause/budget/safety/agent_interruption 均 abort | 同（A4 修复） | 是 | |
| before_agent_start | 每次 run 注入 buildGoalSystemPrompt | 同（A1 修复） | 是 | 首轮恢复延迟见 DEVIATIONS.md A5 |
| prompt 所有权 | marker + 指纹防伪 | pending_run + active 归属 | 否 | DEVIATIONS.md A2（已确认保留） |
| 活跃时钟 | 浮点秒累加；恢复时重置起点 | 同（B9/D3 修复） | 是 | |
| 设置 | 每 session_start 重读；invalid 通知；save 合并保留未知字段 | 同（D1/D2/D5 修复） | 是 | |
| 状态恢复 | 逐字段归一化 | 同（D3 修复） | 是 | |
| 长度口径 | UTF-16 code units | 同（C15 修复） | 是 | |
| 状态栏文案 | waiting reason 经 safeGoalMenuText；continuation_limit 三种文案 | 同（C8 修复） | 是 | |
| goalSummary | Waiting/Resume deadline/Commands 行 | 同（C7 修复） | 是 | |
| complete 状态栏 | 8s 后清空 | 同（C9 修复） | 是 | |
| goal_mode_rules | 14 条 + 预算行格式 | 同（E 修复） | 是 | |
| retryable 判定 | 含 context-overflow / isRetryableAssistantError | 4 条文本 pattern | 否 | DEVIATIONS.md C14（已确认保留） |
| legacy 队列 | 支持 + 警告 | 不支持 | 否 | DEVIATIONS.md D4（已确认保留） |
| token 会计 | 全分支 total − baseline（含非 goal run） | goal-owned run 增量 | 否 | DEVIATIONS.md B4（已确认保留） |

## 事件序列对齐（给定同一输入序列）

| 场景 | TS 事件序列 | Rust 事件序列 | 是否一致 |
| ---- | ----------- | ------------- | -------- |
| /goal start → run | start → owned prompt → agent_start(owned, manual) → turn_end×N → agent_end(记 usage, increment, requestContinuation) → settled → continuation 发送 | 同（pending_run 归属 manual） | 是 |
| 自动延续 run | continuation marker prompt → before_agent_start(markContinuationStarted) → agent_start(automatic) → turn_end 计数 → agent_end → settled → 下一条 continuation | 同（marker 校验 id/iteration） | 是 |
| 自动轮数达上限 | turn_end 计数 → 达上限 → safety_pause(abort) → run aborted → agent_end（goal 已 paused 跳过） | 同 | 是 |
| 用户输入唤醒 waiting | input(清 wait + reset epoch) → message_start/before_agent_start(manual 归属) → agent_end(记 usage + continuation) | 同（B5） | 是 |
| 预算耗尽 | agent_end → budget_limited + abort → 通知 + steer wrap-up | budget_limited + abort + 通知（无 steer） | 否（C13，已登记） |
| waiting 到期 | timer → dispatchDueGoalWait → 清 wait → 续发 continuation | 同（waker 直接 dispatch） | 是 |
