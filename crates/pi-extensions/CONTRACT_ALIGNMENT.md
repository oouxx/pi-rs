# pi-extensions 契约级对齐表

> 针对 goal 扩展公开 API（工具、命令、状态机事件）的行为对照。
> 依据：`@narumitw/pi-goal` v0.52.2 源码 + [`docs/archive/GOAL_TS_COMPARISON_2026-08-24.md`](../../docs/archive/GOAL_TS_COMPARISON_2026-08-24.md)。
> 简化后：无预算、无 UI、无自动轮数，对齐 codex /goal 风格。"否"的差异必须引用 DEVIATIONS.md。

## 工具契约

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| goal_complete 无 goal | 拒绝 "no active goal" + notify + details | 同 | 是 | |
| goal_complete stale goal_id | 拒绝（缺失/过长/不匹配） | 同（UTF-16 长度口径） | 是 | |
| goal_complete 非 active | 拒绝 "goal is X, not active" | 同 | 是 | |
| goal_complete 矛盾 summary | 拒绝（3 条 pattern，lookbehind 版） | 同（等价实现无 lookbehind） | 是 | |
| goal_complete 成功 | transition complete + setCompletionSummary + persist + setStatus("complete") + clearActiveGoal + showCompletionStatus（8s 后清） + terminate | 同（8s timer 清空状态栏） | 是 | |
| goal_complete details | `{goal, goal_id, summary}`（截断） | 同（snake_case + UTF-16 截断） | 是 | |
| goal_blocked 校验链 | no goal / not owner / stale id / not active / reason / evidence / repeated_turns（≥3 整数） | 同 | 是 | |
| goal_blocked 成功 | stopActiveGoal(blocker_report) + terminalReason = reason + notify + terminate | 同 | 是 | |
| goal_blocked details | `{goal, goal_id, reason, evidence, repeated_turns}` | 同 | 是 | |
| goal_wait 校验链 | no goal / not owner / stale id / not active / already waiting / reason / resume_after_ms 范围 | 同 | 是 | |
| goal_wait 钳制 | <10s 钳到 10s；details `requested_resume_after_ms` 仅钳制时出现、`resume_after_ms`、`resume_at` | 同 | 是 | |
| goal_wait 拒绝 | reject + notifyTerminal 警告 | 同 | 是 | |
| goal_wait 持久化 | 只持久化 `{reason, resumeAt}` | 同（requested_ms 仅内存） | 是 | |
| 工具注册 | toolVisibility 控制隐藏/显现 | 始终注册 | 否（有意偏差） | DEVIATIONS.md（已确认保留） |

## 命令契约（/goal）

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| /goal 启动 | 确认替换 → createGoal → owned prompt → 通知 | 同（无预算/自动轮数说明） | 是 | 预算/自动轮数已随简化删除（DEVIATIONS.md） |
| /goal pause | 仅 active 可暂停 + abort 当前 turn + 通知 | 同 | 是 | |
| /goal resume（stopped） | 预算 guard → 轮换 id → 转 active → reset 安全纪元 → resume prompt（真实 stoppedStatus）→ 通知 | 同（无预算 guard） | 是 | 预算 guard 随简化删除（DEVIATIONS.md） |
| /goal resume（waiting） | resumeWaitingGoal：清 wait + 恢复时钟 + buildWaitingResumePrompt + owned prompt | 同 | 是 | |
| /goal resume（active） | "Goal is already active." | 同 | 是 | |
| /goal edit | 状态保持（paused/blocked/usage_limited）；清 wait/continuation/recovery；active 才发 prompt + 重置纪元；通知 "Goal updated" | 同（无预算 guard） | 是 | 预算 guard 随简化删除（DEVIATIONS.md） |
| /goal clear | 清 waker/continuation/recovery/stale；持久化 {goal:null}；清状态栏；无 goal 也清 | 同 | 是 | |
| /goal status | 先记用量+持久化+刷新状态栏；print/json 模式抛错 | 同（模式改为 warning 通知） | 否 | DEVIATIONS.md（无命令错误通道） |
| /goal（无参数） | TUI 模式开菜单 | 直接显示文本状态 | 否（有意偏差） | DEVIATIONS.md（UI 删除，已确认保留） |

## 状态机/事件契约

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 自动轮数计数/上限 | turn_end 计数 + 达上限暂停 | 无此机制 | 否（有意偏差） | DEVIATIONS.md（自动轮数删除，已确认保留） |
| 无进展检测 | 仅 automatic run 计数；同指纹 +1，否则重置 1；达上限暂停（不 abort） | 同（阈值硬编码 3） | 是 | |
| 无进展指纹 | sha256(NFKC 归一) | DefaultHasher（无 NFKC） | 否 | DEVIATIONS.md（已确认保留） |
| resume/edit/用户输入 | 重置安全纪元（无进展/指纹/原因） | 同 | 是 | |
| 用户输入唤醒 waiting | 清 wait → run 归 goal（manual）→ agent_end 续发 continuation | 同 | 是 | |
| wait 到期 | 直接 dispatchDueGoalWait（owned continuation） | 同 | 是 | idle 检查/重试见 DEVIATIONS.md（已确认保留） |
| agent_end 已排 continuation | 跳过 iteration+1 | 同 | 是 | |
| 预算耗尽 | budget_limited + abort + steer wrap-up | 无此机制 | 否（有意偏差） | DEVIATIONS.md（预算删除，已确认保留） |
| 暂停/阻塞后工具调用 | stale 拦截 block 所有工具 + abort | 同 | 是 | |
| 停止路径 abort | pause/agent_interruption 均 abort | 同 | 是 | |
| before_agent_start | 每次 run 注入 buildGoalSystemPrompt | 同（无预算行） | 是 | 首轮恢复延迟见 DEVIATIONS.md |
| prompt 所有权 | marker + 指纹防伪 | pending_run + active 归属 | 否 | DEVIATIONS.md（已确认保留） |
| 活跃时钟 | 浮点秒累加；恢复时重置起点 | 同 | 是 | |
| 状态恢复 | 逐字段归一化 | 同 | 是 | |
| 长度口径 | UTF-16 code units | 同 | 是 | |
| 状态栏文案 | waiting reason 经 safeGoalMenuText；continuation_limit 三种文案 | waiting 消毒；paused 显示原因 | 是 | continuation_limit 文案随自动轮数删除 |
| goalSummary | Waiting/Resume deadline/Commands 行 | 同 | 是 | |
| complete 状态栏 | 8s 后清空 | 同 | 是 | |
| goal_mode_rules | 14 条 + 预算行格式 | 14 条（无预算行） | 是 | 预算行随简化删除 |
| retryable 判定 | 含 context-overflow / isRetryableAssistantError | 4 条文本 pattern | 否 | DEVIATIONS.md（已确认保留） |
| legacy 队列 | 支持 + 警告 | 不支持 | 否 | DEVIATIONS.md（已确认保留） |
| token 会计 | 全分支 total − baseline | 无统计 | 否（有意偏差） | DEVIATIONS.md（tokens_used 删除，已确认保留） |

## 事件序列对齐（给定同一输入序列）

| 场景 | TS 事件序列 | Rust 事件序列 | 是否一致 |
| ---- | ----------- | ------------- | -------- |
| /goal start → run | start → owned prompt → agent_start(owned, manual) → turn_end×N → agent_end(记 usage, increment, requestContinuation) → settled → continuation 发送 | 同（无 usage 记账） | 是 |
| 自动延续 run | continuation marker prompt → before_agent_start(markContinuationStarted) → agent_start(automatic) → turn_end 计数 → agent_end → settled → 下一条 continuation | 同（无 turn_end 计数） | 是 |
| 无进展达上限 | agent_end → no_progress 暂停（不 abort）→ 不再延续 | 同 | 是 |
| 用户输入唤醒 waiting | input(清 wait + reset epoch) → before_agent_start(manual 归属) → agent_end(续发 continuation) | 同 | 是 |
| waiting 到期 | timer → dispatchDueGoalWait → 清 wait → 续发 continuation | 同（waker 直接 dispatch） | 是 |

## web_search / web_fetch 工具契约（`src/web_search.rs`）

> 工具名、参数名、输出文本与 `details` 结构以 `@ollama/pi-web-search` 为准
> （Ollama 后端逐字对齐）；DeepSeek 后端复用同一套工具契约，差异登记在
> `DEVIATIONS.md`。DeepSeek 接口依据
> <https://api-docs.deepseek.com/zh-cn/api/search>、`/api/fetch`。

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 工具注册 | `web_search`（`query` 必填、`max_results` 默认 5）+ `web_fetch`（`url` 必填） | 同名同 schema（`label` 分别为 `Web Search` / `Web Fetch`） | 是 | |
| 未处理工具 | 返回 undefined | `handle_tool_call` 返回 `None` | 是 | |
| 搜索请求（Ollama） | `POST {host}/api/experimental/web_search`，body `{query, max_results}` | 同左 | 是 | |
| 搜索请求（DeepSeek） | —（原版无此后端） | `POST {base}/v1/tools/search`，body `{query, count}`，`Authorization: Bearer <key>`；`count` = `max_results` clamp 到 1～50 | 否（新增后端） | DEVIATIONS.md「web_search 扩展」 |
| 搜索输出 | `1. title\n   URL: url\n   content`（多条空行分隔），空结果 `No results found.` | 同左；DeepSeek 用 `summary`→`snippet`→`content` 取正文 | 是 | 取正文顺序见 DEVIATIONS.md |
| 搜索结果 details | `{results: <API 原始数组>}` | 同左（DeepSeek 保留 `rerank_score`/`site_name`/`date`/`authority` 等字段） | 是 | |
| 抓取请求 | `POST {host}/api/experimental/web_fetch`，body `{url}` | 同左；DeepSeek 为 `POST {base}/v1/tools/fetch` | 是 | |
| 抓取输出 | `Title/Content/Links found: N` + 前 10 条链接 | 同左（DeepSeek 无 links → `Links found: 0`） | 是 | DEVIATIONS.md |
| 抓取 details | `{title, content, links}` | 同左（DeepSeek `links: []`） | 是 | |
| 缺参 | `query`/`url` 非字符串 → 文本错误 | `web_search requires a 'query' string parameter.` / `web_fetch requires a 'url' string parameter.` | 是 | |
| query 校验（DeepSeek） | — | 空白或 >128 字符时直接返回可执行错误（不发请求） | 否（新增后端） | DEVIATIONS.md |
| 401 | `Unauthorized. Run \`ollama signin\` to authenticate.` | Ollama 同左；DeepSeek 提示配置 `webSearch.deepseek.apiKey` / `DEEPSEEK_API_KEY` | 是（Ollama） | DeepSeek 文案见 DEVIATIONS.md |
| 非 2xx | `{Tool} API error (status N): {body \|\| statusText}` | Ollama 同左；DeepSeek 用 `error.message` 并按状态码给 402/403/404/408/413/429/5xx 提示 | 是（Ollama） | DEVIATIONS.md |
| 连接失败 | `Could not connect to Ollama at {host}. Make sure Ollama is running and {tool} is enabled.` | Ollama 同左；DeepSeek 为 `Could not connect to DeepSeek at {base}. Check your network connection or DEEPSEEK_BASE_URL.` | 是（Ollama） | DEVIATIONS.md |
| 超时 | 无显式超时 | 60s 超时后返回 `{tool} request to {label} at {endpoint} timed out after 60s.` | 否（有意增强） | DEVIATIONS.md |
| 后端选择 | 固定 Ollama | settings `webSearch` / 环境变量（`PI_WEB_SEARCH_PROVIDER` 等），优先级 settings → env → 默认 | 否（新增） | DEVIATIONS.md |
