# pi-extensions 契约级对齐表

> 当前内置扩展：`subagent`、`web_search`。`goal` 扩展（原
> `@narumitw/pi-goal` 移植：`/goal` 命令、`goal_complete`/`goal_blocked`/
> `goal_wait` 工具、状态机与事件序列）已于 2026-10-23 按用户要求整体删除，
> 对应的契约表一并移除。
>
> "否"的差异必须引用 `DEVIATIONS.md`。

## subagent 工具契约（`src/subagent.rs` + `src/subagent/agents.rs`）

> 对齐基准：pi-subagents / pi 上游
> `examples/extensions/subagent/{index.ts,agents.ts}`。

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 工具注册 | 名 `subagent`，`label: "Subagent"`，description 拼上用户 agent 目录并提示 `agentScope` | 同名同 label，description 结构一致（目录提示为 `<agent_dir>/agents`，项目目录 `.pi-rs/agents`） | 是 | 目录名差异见 DEVIATIONS.md |
| 参数 schema | `agent?`、`task?`、`tasks?[{agent,task,cwd?}]`、`chain?[{agent,task,cwd?}]`、`agentScope?(user\|project\|both)`、`confirmProjectAgents?`、`cwd?` | 同左（JSON Schema 逐个字段对齐，无 `required`，模式运行时校验） | 是 | |
| 模式校验 | 恰好一种模式，否则返回 `Invalid parameters. Provide exactly one mode.\nAvailable agents: a (user), b (project)` | 同左（空串 agent/task 视作未提供，对齐 JS 真值语义） | 是 | |
| step 参数校验 | `tasks`/`chain` 的每个元素都必须提供合法的 `agent`、`task`；非法参数在派发前拒绝 | 逐项 `Result` 校验，缺字段、空串或类型错误立即拒绝整次调用，返回 `Invalid parallel/chain step N: ...` 与可用 agent；不启动任何子进程 | 是 | 单测：`invalid_task_steps_are_rejected_without_spawning` |
| agent 定义发现 | `~/.pi/agent/agents/*.md` + 最近 `.pi/agents/*.md`；frontmatter `name`/`description`/`tools`/`model`，正文为 system prompt；单文件坏掉只跳过该文件 | 同左（YAML frontmatter 解析规则：去 BOM、CRLF→LF、`---` 包裹；`tools` 接受字符串与数组；缺 `name`/`description` 跳过；符号链接文件也读） | 是 | 目录名为 `.pi-rs` |
| agent 作用域与同名覆盖 | 默认 `user`；`project`/`both`；同名 agent 按 `Map.set` 后发现的定义覆盖前者，`both` 下项目 agent 覆盖用户 agent | 同左；用户/项目目录内均按稳定文件遍历顺序 `upsert`，保留首次插入位置，后读到的同名定义覆盖前者 | 是 | 目录内读取顺序固定为文件名排序，避免文件系统顺序造成非确定性 |
| 未知 agent | 返回错误并列出所有可用 agent名 | `Unknown agent: "x". Available agents: "a", "b".` | 是 | |
| 三种模式 | single `{agent,task}`；parallel `{tasks}`（≤ 8 任务，4 路并发，`mapWithConcurrencyLimit`）；chain `{chain}`（顺序，`{previous}` 代入上一步输出，失败即停） | 同左（`MAX_PARALLEL_TASKS=8`、`MAX_CONCURRENCY=4`、`JoinSet` 并发池） | 是 | |
| 子进程命令 | `pi --mode json -p --no-session [--model m] [--thinking lvl] [--tools a,b] --append-system-prompt <tmpprompt> "Task: <task>"` | `pi --mode json -p --no-session [--model m] [--thinking lvl] [--tools a,b] [--append-prompt <正文>] "Task: <task>"` | 是 | persona 传递方式见 DEVIATIONS.md |
| cwd 预校验 | `cwd` 不存在时子进程不会启动，`runSingleAgent` 形成失败结果并保留诊断 | spawn 前检查 `Path::is_dir()`；非法时返回 `Invalid cwd (not a directory)` 且列出 binary/cwd，不调用 spawn | 是 | 单测：`invalid_cwd_is_rejected_before_spawn` / `spawn_error_reports_binary_and_cwd` |
| 模型 / thinking 继承 | agent 的 `model` 缺省或为空串（falsy）时继承 `ctx.model`（`provider/id`）与 `ctx.thinkingLevel`；`model` 非空时不继承 thinking | 同左（空串归一为 `None`；`get_model` / `get_thinking_level` 为**实时**读取，`/model`、`/thinking` 切换后立即生效） | 是 | 单测：`empty_agent_model_inherits_dispatch_config` |
| 事件解析 | 读 stdout JSONL：`message_end` 收消息与 usage，`tool_result_end` 收工具结果 | 读 stdout JSONL 的 `message_end`（pi-rs 的工具结果同样以消息形式出现在 `message_end`，无独立 `tool_result_end` 事件） | 是 | 事件名差异源于 pi-rs 的事件枚举 |
| 失败判定 | `exitCode != 0 \|\| stopReason ∈ {error, aborted}` | 同左 | 是 | |
| 子 agent 自报 aborted | single/chain 将该结果作为失败；parallel 只把该 task 标为 `failed (aborted)`，其他任务照常执行并汇总 | 同左；只有父级 `ToolInvocation` 中止才使整次调用报 `Subagent was aborted` | 是 | 单测：`child_aborted_result_does_not_abort_parallel_batch` |
| 失败文案 | single：`Agent {stopReason} failed: {errorMessage \|\| stderr \|\| 终稿 \|\| (no output)}`；chain：`Chain stopped at step N (agent): ...` | 同左 | 是 | |
| 并行汇总 | `Parallel: N/M succeeded` + 每任务 `### [agent] completed/failed [(stopReason)]`，每任务输出 50KB 字节上限 + `[Output truncated: N bytes omitted. Full output preserved in tool details.]` | 同左 | 是 | |
| usage 统计 | 每个 assistant 消息累计 turns / input / output / cacheRead / cacheWrite / `cost.total` / `contextTokens`（= `totalTokens`）；聚合后仅在 TUI 渲染展示 | 同左，结构化放在 `details.results[].usage`（模型可见文本里不含 usage，与原版一致） | 是 | 展示层缺失见 DEVIATIONS.md |
| details 结构 | `{mode, agentScope, projectAgentsDir, results[{agent, agentSource, task, exitCode, messages, stderr, usage, model, stopReason, errorMessage, step?}]}` | 同左（字段名逐一对齐） | 是 | |
| 项目 agent 确认 | `scope ∈ {project,both} && confirmProjectAgents && ctx.hasUI && !ctx.isProjectTrusted()` 时 `ctx.ui.confirm("Run project-local agents?", ...)`，拒绝返回 `Canceled: project-local agents not approved.` | 同左（逐字一致，含 `has_ui` / `is_project_trusted` 语义） | 是 | 单测：`project_agents_require_confirmation` / `trusted_project_skips_confirmation` |
| 自定义渲染 | `renderCall(args, theme, ctx)` + `renderResult(result, {expanded}, theme, ctx)` 返回组件树，逐段 `theme.fg(...)` 着色 | `ToolDefinition.render` → `ToolRenderOutput{title: Vec<ToolRenderSpan>, blocks: Vec<ToolRenderBlock>}`（`Line(spans)` / `Spacer` / `Markdown(text)`，span 用语义色 token）；TUI 每帧按 `expanded` 求值，`None` 回落 fallback。subagent 布局逐项对齐：折叠 10 条 / 每步 5 条、`─── Task/Output/Step N ───` 分节、每步 `Task:` + 工具调用 + Markdown 终稿 + 该步 usage、`Total:` + `(Ctrl+O to expand)` | 是（有意偏差：语义块而非任意组件树，见 DEVIATIONS.md） | |
| 流式进度（`onUpdate`） | 每收到子进程消息 `emitUpdate(partial)`，父界面实时刷新 | `ToolInvocation::update(&ToolCallOutput)` → core `tool_execution_update` → TUI 即时刷新该工具块（含 partial `details`） | 是 | 事件源只有 `message_end`（DEVIATIONS.md）；E2E：`tui_custom_tool_render_uses_extension_layout` |
| 中止传播 | `AbortSignal` → SIGTERM → 5s → SIGKILL；single/chain 抛错，parallel 由 `Promise.all` 拒绝 → 整次调用报 "Subagent was aborted" | `ToolInvocation::abort`（watch 信号）：`tokio::select!` 于读取循环及 `child.wait()`，父级中止即 kill child 并让 single/chain/parallel 整次报错；子进程自己返回的 `stopReason=aborted` 不触发整批中止 | 是（有意偏差：直接 SIGKILL，见 DEVIATIONS.md） | 单测 `abort_kills_child_and_marks_aborted` / `abort_after_stdout_eof_kills_child` / `parallel_abort_fails_whole_call`；E2E `tui_extension_tool_abort_signal_reaches_tool` |

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
