# pi-goal 扩展：Rust 实现 vs 原版 TS 对比分析

> 分析日期：2026-08-24
> 分析对象：`crates/pi-extensions/src/goal.rs`（2877 行）+ `src/goal/tests.rs`（841 行）
> 原版 TS：`@narumitw/pi-goal` **v0.52.2**（npm 包源码，19 个 `src/*.ts`，共 6118 行；
> Rust 头注释声明的对齐目标版本）
> 方法：逐文件通读两侧源码；关键行为对照 pi-agent-core / pi-coding-agent
> 的实际事件接线（HookHandler trait、dispatcher、agent_session）核实。

---

## 1. 总体结论

Rust 扩展在**状态机骨架**（6 态、三工具验证链、goal_id guard、自动延续、
会话内持久化）上对齐度较高，主要复刻了 TS 的核心流程。但存在**约 30 处
行为差异**，其中只有约 7 处在 Rust 头注释里登记过；**其余绝大部分没有
登记**，且不属于 DEVIATIONS.md 中"扩展系统内部实现允许偏离原版"这一已确认
偏差能覆盖的范围（DEVIATIONS.md 明确要求扩展对外 interface 和函数行为必须
与原版一致）。

差异按性质分三类：

- **A 类**：由 pi-extension-api 能力缺口造成（基础设施层，可归入"扩展系统
  内部实现偏差"）——修复 pi-extension-api 后 goal 扩展才能对齐。
- **B 类**：状态机/计数语义差异（未登记，直接影响行为）。
- **C 类**：命令/UI/工具契约差异（未登记，多为"对外行为"层面）。
- **D 类**：设置/持久化差异。
- **E 类**：Prompt 文案差异（影响模型行为）。

---

## 2. 差异清单

### A 类：pi-extension-api 能力缺口（基础设施层）

| # | TS 行为 | Rust 实际 | 影响 | 是否登记 |
|---|---------|-----------|------|---------|
| A1 | `before_agent_start` 每次 run 注入 `buildGoalSystemPrompt`（含 `Active /goal:` + 预算用量）到 systemPrompt | 完全没有该系统 prompt 注入（HookHandler trait 无此 hook 的 ctx，且 goal 扩展未实现） | 模型在非 owned run（用户消息轮）里看不到"当前有 goal"的系统提示；预算感知弱化 | 否 |
| A2 | input 拦截：prompt marker 所有权（pending/claimed/cancelled marker、非 goal 输入队列、`preservesOwnedPromptAtTerminalBoundary`、stale-owned-prompt 消费） | 无任何 marker 所有权机制 | 无法区分扩展自有 prompt 与用户输入；并发/竞争下可能误判 | 否 |
| A3 | `tool_call` 事件里 `staleGoalToolCallsBlocked` 会 **block 所有工具 + abort 当前 turn**（goal paused/blocked/usage_limited 时） | `stale_blocked` 标志 **只写不读**（grep 证实 set/clear 后无人 load），实际仅靠各工具的 goal_id 校验；`before_tool_call` hook 无 ctx，无法 abort | 暂停/阻塞后，模型仍能调用**其他任意工具**继续烧钱，且不中断当前 turn | 否 |
| A4 | pause/safety_pause/agent_interruption/tools_unavailable 都会 `abortCurrentTurn()` | Rust 从不调用 abort（RuntimeHandle 里有 abort，但扩展未用） | TS 暂停/出错会立即打断当前生成；Rust 让当前 run 跑完 | 否 |
| A5 | `session_start/shutdown/before_compact/compact`、`message_start`、`context`、`tool_execution_end` 都接线（预算 wrap-up 的 steer 消息、compaction 重试、guardAbortGoalId 等），且**每个事件 handler 都带 ctx** | Rust 只有 agent_end/agent_settled/handle_tool_call 有 ctx；session_start/shutdown/compact、input、message_start、turn_end、before_agent_start、tool_call、tool_execution_end 均无 ctx；无 post-compact 事件 | 见 B 类各项具体缺失 | 部分（扩展系统内部实现偏差） |
| A6 | tools 默认隐藏、首次 /goal 后 reveal、不可用时 pause | 工具始终注册；`toolVisibility` 只影响显示 | 已登记偏差，无碍核心 | 已登记 |

**A3/A4 是"文档宣称已实现但实际未生效"的差异，最先处理。**

### B 类：状态机/计数语义差异（未登记）

| # | TS | Rust | 影响 |
|---|----|------|------|
| B1 | `automaticModelTurns` 在 **每次 turn_end**（模型每轮响应）加 1 | 在 **每次 agent_end（整个 run）** 加 1 | 一轮含 N 次工具调用时：TS 记 N，Rust 记 1。自动轮数上限（默认 25）语义不同：Rust 实际允许 25 个 run（远多于 25 次模型调用） |
| B2 | resume/edit/用户输入都会 `resetGoalSafetyEpoch` / `resetActiveSafetyEpoch`（清零自动轮数、无进展计数、指纹、safetyPauseCause） | resume/edit/用户输入**都不重置**这些计数 | **resume 一个因 continuation_limit 暂停的 goal：TS 计数器归零；Rust 计数仍 ≥25 → 下一轮 agent_end 立刻再次暂停**（等于 resume 无效） |
| B3 | no-progress 只对 **automatic run** 计数（`run.origin === "automatic"` 才 `recordAutomaticRunProgress`） | 对所有 run（含手动）计数 | 用户手动发消息产生相似无工具输出也会推高无进展计数 |
| B4 | token 会计 = 整个会话分支 assistant tokens − baseline（**含非 goal run**，用户消息轮也计入 goal 用量） | 只累计 goal-owned run 的 assistant usage 增量（非 goal run 不记账） | 用户穿插聊天时：TS 扣 goal 预算，Rust 不扣（已登记为"增量累计近似"） |
| B5 | 用户输入唤醒 waiting goal：input 清 wait → run 归 goal（`beginNonGoalFollowUp` 设 agentRunGoalId）→ agent_end 记 usage 并 `requestContinuation` | 用户输入只清 waiting；该 run 不归 goal → agent_end 提前 return → **不产生 continuation** | **用户"唤醒"后 goal 停在 active 但不再自动推进**，必须再等一次工具/消息 |
| B6 | wait deadline 到期：GoalWaitTimer 直接 `dispatchDueGoalWait`（idle 才发 owned continuation，失败 1s 重试一次、两次后 exhausted） | tokio waker 到期先 queue 一条**裸 follow-up**（非 owned），多烧一轮模型调用后才在 settled 边界续发 owned continuation；无 idle 检查/重试/exhausted | 行为不一致（多一轮无主 run、无闲置保护） |
| B7 | agent_end 若已有 continuation work（如 compaction 排队的）→ **跳过 iteration+1** | 无条件 `increment()` | 特定时序下 iteration 多算 1 |
| B8 | waiting 时 wait 信息持久化只有 `{reason, resumeAt}` | Rust `GoalWaiting` 多持久化 `requested_ms` 字段 | 持久化条目形状不一致 |
| B9 | 活跃时钟：`activeStartedAt` 恢复时重设为 now（`normalizeLoadedGoal`）；`timeUsedSeconds` 浮点累加（毫秒/1000） | serde 原样恢复（不重设时钟起点）；`u64` 整数整除累加（每 checkpoint 截断） | 重启后活跃耗时口径不同；累积有亚秒级偏差 |

### C 类：命令/UI/工具契约差异（未登记）

| # | TS | Rust | 说明 |
|---|----|------|------|
| C1 | `/goal resume` 预算仍超时**拒绝**（状态不变） | 直接转 active 发 prompt，无预算 guard（`next_instance()` 后硬置 Active） | 预算耗尽的 goal 可被 resume，浪费一轮后 agent_end 再转 budget_limited |
| C2 | resume prompt 用真实 stoppedStatus（`buildResumePrompt(goal, stoppedStatus)`，blocked→"blocked /goal"） | 硬编码 `GoalStatus::Paused`（blocked/usage/budget resume 的 prompt 都写 "paused /goal"） | prompt 语义错误 |
| C3 | waiting goal 的 `/goal resume` 走 `resumeWaitingGoal`（buildWaitingResumePrompt + clearGoalWait） | `is_active()` 直接判 "Goal is already active."，**菜单里对 waiting goal 显示的 Resume 也是 no-op** | waiting 无法用命令/菜单恢复 |
| C4 | 编辑：paused/blocked/usage_limited 保持原状态（`editedGoalStatus`），只有转 active 才发 prompt；budget_limited 编辑仍过预算 guard；会 `clearGoalWaitTimer + cancelContinuationWork + queueGoalSafetyReset`；send 失败回滚 previousGoal | 一律置 Active 发 prompt，**绕过预算 guard、不清 waiting、不取消 continuation/wait waker、不重置安全计数、无回滚** | 编辑 waiting/budget_limited/paused goal 行为完全不同；残留旧 waker |
| C5 | clear：持久化 `{goal: null}` + setStatus(undefined) + 无 goal 时也清 persisted | 持久化 complete 占位、**不更新状态栏**（旧状态残留）、无 goal 时不清、不清 continuation/wait waker | waiting goal 被 clear 后旧 waker 仍会到期发过期消息 |
| C6 | `/goal status` 先 recordGoalUsage+persist+updateStatus 再显示；print/json 模式抛错 | 只 notify；始终 notify | 模式行为不同 |
| C7 | goalSummary 含 `Waiting:` + `Resume deadline:`（ISO）+ `Commands:` 行；safety pause 文案区分原因 | 缺这三行；safety 文案统一 | |
| C8 | formatStatus：waiting reason 经 `safeGoalMenuText`（剥终端转义/折叠空白/截 120）；paused by continuation_limit 有专用文案（`automatic limit X/Y` / `previous automatic limit at X`） | waiting reason 原样显示；paused 恒为 `paused · continuation_limit · automatic X/Y` | 状态栏可被终端转义污染 |
| C9 | 完成后 setStatus("complete") 8 秒后自动清空 | setStatus("complete") **永久残留** | |
| C10 | 工具 details 用 **snake_case**：`goal_id`、`repeated_turns`、`resume_after_ms`、`resume_at`、`requested_resume_after_ms` | Rust 用 **camelCase**：`goalId`、`repeatedTurns`、`resumeAfterMs` | 公开工具输出契约不一致（对调 LLM 工具结果很重要） |
| C11 | goal_wait 拒绝时 `notifyTerminal` 警告 | Rust goal_wait 的 reject 不 notify（只有 content） | |
| C12 | goal_blocked 存 terminalReason = reason | Rust 存 `"{reason} (evidence: {evidence})"` | |
| C13 | 预算耗尽走 steer 自定义消息（`BUDGET_WRAP_UP_MESSAGE_TYPE`）+ delivered 标记；**wrap-up 期间允许 goal_complete**（status 非 active 也可） | 只发一条 notify（含 wrap-up 文本）+ 状态 budget_limited；goal_complete 在 budget_limited 一律拒绝 | 已登记"以拒绝文本 + 状态呈现"，但 **complete-during-wrap-up 这个功能差异未登记** |
| C14 | retryable 错误还包含 context-overflow 判定 + pi-ai `isRetryableAssistantError`（第 5 条 pattern） | 只有 4 条文本 pattern | 已登记 |
| C15 | 长度用 `.length`（UTF-16 code units） | `chars().count()` | emoji 等非 BMP 字符长度口径不同 |
| C16 | fingerprint：sha256(NFKC 归一 + 去全部 Cc/Cf + \s 折叠) | DefaultHasher（无 NFKC、仅 ASCII 标点判断、部分 format 字符） | 全角字符的"无进展指纹"可能不同 |

### D 类：设置/持久化差异

| # | TS | Rust |
|---|----|------|
| D1 | 无效设置文件 → notify "pi-goal settings ignored: ..." + 回退默认 | 静默回退默认（注释说"调用方提示"但实际没有） |
| D2 | save 合并原文件保留未知字段、校验失败抛错 | 直接覆写，丢失未知字段 |
| D3 | 逐字段归一化恢复（tokenBudget 校验、计数 ≥0、指纹必须 64-hex、waiting 仅 active 保留、activeStartedAt 重置 now） | serde 整体解析，任何字段非法 → 整个 goal 丢弃；不重置时钟 |
| D4 | 支持 legacy queue（`goals-state` entry + 警告） | 不支持（已登记） |
| D5 | settings 每次 session_start 重读 | 扩展生命周期内缓存一次 |

### E 类：Prompt 文案差异（影响模型行为）

- `goalModeRules` TS 15 条 vs Rust 12 条（缺 "After a blocked goal is resumed, start a fresh three-turn blocker audit..."、"...Never use it merely because work is hard/slow/uncertain..."、"Use resume_after_ms only as a bounded safety wake-up, not polling interval"、"Do not use it for ordinary unfinished work..."）→ 模型拿到指令少 3 条
- budget 行格式：TS `\nToken budget: X.`（预算下接无空行）vs Rust `\n\nToken budget: X.\n`（多空行）
- TS 有专用 `buildWaitingResumePrompt`（`<goal_wait_reason>` 转义块，标注 untrusted）；Rust 没有
- continuation marker 无随机 UUID 段（仅内部使用，影响不大）

---

## 3. 与已登记偏差的关系

Rust 头注释（goal.rs 第 1-45 行）已登记的有意简化：

| 登记项 | 对应差异 | 状态 |
|--------|----------|------|
| 工具始终注册（toolVisibility 只影响显示语义） | A6 | 已确认保留，合规 |
| TUI 菜单/设置 UI 已补全（menu.ts / settings-ui.ts） | — | 已确认保留 |
| `pi.events` 协议不做（managed-run RPC） | A5（部分） | 已确认保留 |
| legacy 实验性队列（多目标）不支持 | D4 | 已确认保留 |
| token 会计用 assistant usage 增量累计（原版 total-baseline 的等价近似） | B4 | 已确认保留（近似，非完全等价） |
| budget wrap-up 以拒绝文本 + 状态呈现 | C13（部分） | 已登记呈现方式；**wrap-up 期间允许 complete 未登记** |
| retryable 不做 context overflow 判定 | C14 | 已登记 |

**未登记且不属于上述偏差的差异**：A1-A5（能力缺口）、B1/B2/B3/B5/B6/B7/B8/B9、
C1-C12、C13 的 complete-during-wrap-up、C15/C16、D1/D2/D3/D5、E 类全部。

---

## 4. 优先处理建议（按"像 bug"程度排序）

1. **B2（resume/edit 不清零自动轮数）** — 自动轮数暂停后 resume 立即再暂停，功能上"恢复"失效；TS 侧明确有 `queueGoalSafetyReset` / `resetGoalSafetyEpoch` 语义。
2. **A3/A4 stale 工具拦截 + abort 未实现** — `stale_blocked` 写而不读是死代码；TS 会在暂停/阻塞时阻止任意工具调用并中断当前 turn。
3. **C10 工具 details 字段名 snake_case vs camelCase** — 直接破坏工具输出契约（LLM 读取的细节 shape）。
4. **C1/C2/C3 resume 语义（预算 guard、stoppedStatus、waiting resume）** — 命令行为与原版不一致。
5. **C4 编辑语义（状态保持、预算 guard、waiting 清理）** — 差异大且含潜在 bug（编辑 waiting goal 残留旧 waker）。
6. **B1/B3/B4/B6 计数与唤醒语义** — 影响"自动延续/预算/无进展"核心状态机，需按阶段三做事件序列对齐。

---

## 5. 后续动作

- [x] 将未登记差异补录进 `crates/pi-extensions/DEVIATIONS.md`（状态"待确认"）
- [x] 修复 pi-extension-api 能力缺口（A1/A3/A4 已修；A2/A5 已登记为有意保留）
- [x] 按阶段四规则修复 B/C 类回归 bug，每修一个补 `PORTING_MISTAKES.md`
- [x] 阶段三"契约级对齐表"与"事件序列对齐"覆盖 goal 扩展（`CONTRACT_ALIGNMENT.md`）

## 6. 修复状态（2026-08-24 对齐修复后）

> **2026-08-24 简化（用户确认，对齐 codex /goal 风格）**：per-goal 预算
> 限制整体删除、TUI 菜单/设置 UI 整体删除、自动轮数限制删除、设置文件
> 删除、tokens_used 统计删除。以下状态表按简化后口径更新。

| 项 | 状态 | 说明 |
|----|------|------|
| B1 | ✅ 已修（后随简化删除） | 自动轮数计数/上限已按 turn_end 对齐，随后用户确认删除（codex 无此机制） |
| B2 | ✅ 已修 | resume/edit/用户输入重置安全纪元（无进展/指纹/原因） |
| B3 | ✅ 已修 | 无进展计数仅对 automatic run |
| B4 | 已登记保留（后随简化删除） | token 会计/tokens_used 统计已随简化删除 |
| B5 | ✅ 已修 | before_agent_start 无归属 run 归属 goal；唤醒后 agent_end 续发 |
| B6 | ✅ 已修 | waker 直接 dispatchDueGoalWait（owned continuation）；idle 检查/重试/exhausted 因无 is_idle API 保留为偏差 |
| B7 | ✅ 已修 | 已有 continuation work 时跳过 iteration+1 |
| B8 | ✅ 已修 | requested_ms 不再持久化 |
| B9 | ✅ 已修 | 浮点秒累加 + 恢复重置时钟起点 |
| C1 | ✅ 已修（后随简化删除） | resume 预算 guard 已按原版对齐，随后随预算删除 |
| C2 | ✅ 已修 | resume prompt 用真实 stoppedStatus |
| C3 | ✅ 已修 | waiting goal resume（命令 + 菜单） |
| C4 | ✅ 已修 | edit 状态保持 + waiting/continuation 清理（预算 guard 随简化删除） |
| C5 | ✅ 已修 | clear 持久化 {goal:null} + 清状态栏 + 无 goal 也清 |
| C6 | ✅ 已修（部分） | 先记用量再显示；print/json 模式改为 warning 通知（无错误通道，见 DEVIATIONS.md） |
| C7 | ✅ 已修 | goalSummary 加 Waiting/Resume deadline/Commands 行 |
| C8 | ✅ 已修 | waiting reason 消毒 + paused 原因显示（continuation_limit 文案随自动轮数删除） |
| C9 | ✅ 已修 | complete 状态栏 8s 后清空 |
| C10 | ✅ 已修 | details 全 snake_case |
| C11 | ✅ 已修 | goal_wait 拒绝 notify |
| C12 | ✅ 已修 | terminalReason = reason |
| C13 | 随简化删除 | 预算 wrap-up 整体随预算删除（不再需要） |
| C14 | 已登记保留 | 不做 context-overflow 判定 |
| C15 | ✅ 已修 | UTF-16 code units 口径 |
| C16 | 已登记保留 | 指纹不做 NFKC/sha256 |
| D1 | ✅ 已修（后随简化删除） | 设置文件整体删除，无进展阈值硬编码 |
| D2 | ✅ 已修（后随简化删除） | 同上 |
| D3 | ✅ 已修 | 逐字段归一化恢复 |
| D4 | 已登记保留 | legacy 队列不支持 |
| D5 | ✅ 已修（后随简化删除） | 设置文件整体删除 |
| E 类 | ✅ 已修 | 14 条规则、buildWaitingResumePrompt、continuation marker 含 UUID（预算行随简化删除） |
| A1 | ✅ 已修 | before_agent_start 注入 buildGoalSystemPrompt（每次 run） |
| A2 | 已登记保留 | prompt marker 所有权机制不做（pending_run + active 归属兜底） |
| A3 | ✅ 已修 | before_tool_call（新增 ctx）stale 时 block 所有工具 + abort |
| A4 | ✅ 已修 | pause/budget/safety/agent_interruption 停止路径 abort（RuntimeHandle.abort 已接线） |
| A5 | 已登记保留 | session_start/compact 等事件无 ctx / 无 post-compact；首轮恢复延迟 |
| A6 | 已登记保留 | 工具始终注册 |

## 7. 简化记录（2026-08-24，用户确认）

对齐 codex /goal 风格（codex 无 per-goal 预算、无菜单、无自动轮数上限）：

| 删除项 | 涉及代码 |
|--------|----------|
| per-goal 预算 | `--tokens` 参数、`GoalState.token_budget`、`GoalStatus::BudgetLimited`（6 态→5 态）、`limit_for_budget`、`BUDGET_WRAP_UP_PROMPT`、prompt 预算行、resume/edit 预算 guard、`format_budget`/`format_token_count`/`parse_token_budget` |
| TUI 菜单/设置 UI | `show_goal_menu`/`build_menu_state`/`display_status`/MENU_*、`show_settings_menu`/`choose_*`/`apply_settings`、`choose_budget`/`increase_budget_flow`/`edit_goal_flow`、`is_tui` 分支（/goal 无参数直接文本 status） |
| 自动轮数限制 | `automatic_turns`、`automatic_model_turns` 计数、`on_turn_end` 计数、`enforce_automatic_limit`、`continuation_limit` 暂停原因、状态栏 `automatic X/Y` |
| 设置文件 | `pi-goal.json` 读写（`GoalSettings`/`read_goal_settings`/`save_goal_settings`/`settings()` 缓存），无进展阈值硬编码 `NO_PROGRESS_TURNS=3` |
| token 统计 | `tokens_used`/`baseline_tokens`/`cumulative_assistant_tokens`/`assistant_usage_tokens` |

**保留**：edit 文本命令、无进展抑制（codex spin 抑制等价物）、3 个工具、自动延续状态机、waiting 机制、goal_id guard、stale 拦截、abort 停止路径、系统 prompt 注入。
