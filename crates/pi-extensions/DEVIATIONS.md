# pi-extensions 偏差日志

> 状态约定：`已确认保留` = 禁止在"对齐"名义下修改；`待确认` = 尚未与用户
> 确认，属于已知差异。
>
> **2026-10-23：`goal` 扩展按用户要求整体删除**（原 `@narumitw/pi-goal`
> 移植，含 `/goal` 命令、`goal_complete`/`goal_blocked`/`goal_wait` 工具、
> 状态机与旁路 prompt 所有权机制）。随之删除的还有它专用的偏差条目；
> `pi-extension-api` 与 `pi-coding-agent` 的接线能力（事件 ctx、
> `RuntimeHandle` 各通道）保留，供其他扩展使用。

## 基础设施（pi-extension-api / pi-coding-agent 接线）

| 位置 | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---- | ---------- | ------------- | -------- | -------- |
| HookHandler::before_tool_call | 带 ctx（可 abort） | 已带 ctx（A3/A4 修复） | 已对齐 | — |
| HookHandler::handle_tool_call | `execute(toolCallId, params, signal, onUpdate, ctx)` | `handle_tool_call(tool_name, params, ctx, &ToolInvocation)`；`ToolInvocation` 携带 `tool_call_id` / `abort`（watch 信号）/ `on_update`，语义一一对应 | 已对齐（2026-10-26） | — |
| ExtensionContext::hasUI / isProjectTrusted | `ctx.hasUI`（是否有真实 UI）+ `ctx.isProjectTrusted()`（项目信任） | 同名字段 `has_ui` / `is_project_trusted`：`has_ui` 在**宿主传入真实 UI context**（interactive TUI / RPC / ACP，`ui_context.is_some()`）或之后 `set_extension_ui_context` 换绑时置位（2026-10-26 修正：此前只有 interactive 换绑路径置位，RPC/ACP 下恒 false）；`is_project_trusted` 按 TS `resolve_project_trusted` 解析（含 `--trust`/`--no-trust` override，2026-10-26 接线） | 已对齐（2026-10-26） | 项目资源门控本身仍未实现（见 `pi-coding-agent/DEVIATIONS.md`） |
| HookHandler::on_turn_end | 带 ctx | 已带 ctx；当前内置扩展未使用，API 保留供其他扩展 | 已对齐 | — |
| RuntimeHandle.abort | 接线到 agent.abort | 已接线（spawn 任务调 agent.abort，不 wait_for_idle） | 同步 Fn 不能 await | 已确认保留 |
| 事件 ctx（通知 + transform） | TS `ExtensionHandler<E, R> = (event: E, ctx: ExtensionContext)`：**所有**事件 handler 都收到 `ExtensionContext` | **已对齐（2026-10-13）**：`HookHandler` 全部方法均携带 ctx——`handle_tool_call`、`before_tool_call` 为必填 `&ExtensionContext`；通知类 void hook（session_start/shutdown/info_changed、agent_start、turn_start、message_start/update/end、tool_execution_start/update/end、model_select、thinking_level_select、compact、tree、resources_discover、project_trust、context、user_bash）及 `agent_end`/`agent_settled`/`turn_end` 为 `Option<&ExtensionContext>`（无绑定会话的测试/直接 Agent 场景为 `None`）；transform/modifying hook（`on_message_end_mut`、`on_context_mut`、`on_tool_result_mut`、`after_tool_call`、`before_agent_start`、`on_input`、`before_provider_request`、`before_provider_headers`、`after_provider_response`、`before_session_switch/fork/compact/tree`）为必填 `&ExtensionContext`。无需扩展侧兜底 | 通知类事件签名对齐 TS `(event, ctx)`；required/option 的区分对应“是否绑定真实会话” | 已对齐 |

## subagent 扩展（`src/subagent.rs`、`src/subagent/agents.rs`）

> 对齐基准：pi-subagents / pi 上游
> `examples/extensions/subagent/{index.ts,agents.ts}`。2026-10-26 按用户
> 要求"去掉自造功能、完全对齐原版"重写：agent 定义发现、single/parallel/
> chain 三模式、exit code/stopReason 失败判定、stderr 诊断、usage 聚合、
> 50KB 并行输出上限均为原版行为。2026-10-26 第二轮补齐了原版的自定义渲染
> （语义块 + 行内配色）、`onUpdate` 进度、abort 传播、`hasUI`/`isProjectTrusted`
> 确认门；下表只剩"组件树形态"与少量平台差异。

| 位置 | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---- | ---------- | ------------- | -------- | -------- |
| 自定义渲染的形态（`renderCall` / `renderResult`） | 返回任意组件树（`Container`/`Markdown`/`Text`，可直接读 theme 着色、自由嵌套） | `ToolDefinition.render` 返回**语义块序列** `ToolRenderOutput{title: Vec<ToolRenderSpan>, blocks: Vec<ToolRenderBlock>}`：`Line(spans)` / `Spacer` / `Markdown(text)`，span 用语义 token（`Default`/`Muted`/`Dim`/`Accent`/`Warning`/`Error`/`Success`/`Bold`）而不是颜色值。subagent 已按原版布局拼装：`✓`/`✗` 图标分色、`subagent <desc> [scope]` 三段标题、`─── Task ───`/`─── Output ───`/`─── Step N: agent ✓`、每步 `Task:` + 工具调用 + Markdown 终稿 + 该步 usage、`Total:`、折叠提示 | 让扩展不依赖 ratatui/主题内部类型（pi-tui 不反向依赖扩展 API）；任意组件树/嵌套容器/自定义 widget 需要把整个 TUI 布局能力暴露给扩展，收益不抵复杂度 | 已确认保留（仅剩"无任意组件树/无 theme 直读"） |
| 中止的传播范围 | single/chain 抛错；parallel 由父级 `AbortSignal` 触发 `Promise.all` 拒绝 → 整次工具调用报 `Subagent was aborted`；子 agent 自己返回 `stopReason=aborted` 时只是普通失败结果 | 同左：single 返回 `Agent aborted: Subagent was aborted`、chain 返回 `Chain stopped at step N`；parallel 仅在 `ToolInvocation` 已中止时整次报错，子结果自报 aborted 时保留其他任务并汇总为 `failed (aborted)` | — | 已对齐（2026-10-27） |
| 中止的杀进程方式 | `SIGTERM`，5s 未退再 `SIGKILL` | 直接 `start_kill()`（SIGKILL），随后结果标记 `stopReason: "aborted"` + `errorMessage: "Subagent was aborted"`（文案对齐原版抛出的错误）。另保留 `kill_on_drop(true)` 作为父 future 被 drop 时的兜底 | pi 的 print 模式子进程没有需要优雅收尾的持久状态（`--no-session`），先 SIGTERM 只是礼貌；SIGKILL 更快且不留半死进程 | 已确认保留 |
| persona 注入方式 | agent 正文写 `mkdtemp()` 临时文件（`mode: 0o600`）后传 `--append-system-prompt <path>`，调用结束删文件 | 直接 `--append-prompt <正文>`（pi-rs 该 flag 收字面文本，不接受路径）。语义相同（追加到 system prompt）；差异是正文出现在子进程 argv 里，且受 `ARG_MAX` 限制（agent 定义通常几 KB，安全） | pi-rs 的 `-A/--append-prompt` 不接受路径；写临时文件还要额外清理逻辑，收益只有 argv 保密性 | 已确认保留 |
| 项目 agent 确认条件 | `scope ∈ {project, both} && confirmProjectAgents && ctx.hasUI && !ctx.isProjectTrusted()` | 同左（逐字一致）：`ctx.has_ui` = 宿主换绑真实 UI 时置位（interactive TUI、带 UI 的客户端），`ctx.is_project_trusted` = 宿主按 `resolve_project_trusted`（显式 override → 项目无需信任的资源 → 已存决策 → `defaultTrust`）算出。注意 pi-rs 仍未实现"项目资源门控"本身（见 `pi-coding-agent` DEVIATIONS），所以 `trust.json` 里没有决策时按 `defaultTrust=ask` + 无 UI 保守判未信任 → TUI 下仍会弹确认 | — | 已对齐 |
| 项目配置目录名 | `<最近祖先>/.pi/agents`（`CONFIG_DIR_NAME = ".pi"`） | `<最近祖先>/.pi-rs/agents`（`agents::PROJECT_CONFIG_DIR`） | 配置目录名不同（pi `.pi` / pi-rs `.pi-rs`）；pi-extensions 不依赖 `pi-coding-agent`，常量独立成本地常量 | 已对齐（目录名差异同 `pi-coding-agent` 的配置目录条目） |
| agent 列表顺序 | 依赖 `readdirSync` 顺序（文件系统相关） | 按 `name` 排序 | 确定性输出，便于测试与展示；只影响列表顺序，不影响按 `name` 查找 | 已确认保留 |
| spawn 失败诊断 | `proc.on("error", () => resolve(1))`，错误对象丢弃，最终只能看到 `(no output)` | `exitCode = 1`，并把 `Failed to spawn pi: {e} (binary: {path})` 写进 `result.stderr`（走原版的失败输出通道返回） | 原实现完全丢失 spawn 失败原因；改动只影响失败诊断文本 | 已确认保留 |
| 被信号杀死时的退出码 | `proc.on("close", code => resolve(code ?? 0))`，`code` 为 null 时按 0 | 同左（`status.code().unwrap_or(0)`），失败判定交给 `stopReason` | 对齐原版 | 已对齐 |
| 子进程 stderr 保留量 | 无上限字符串拼接，读到流结束 | 持续读到 **EOF**（正常情况子进程退出即返回），只保留前 256KB（超出丢弃并附 `[stderr truncated]`）；只有在孙进程仍持有写端的病态情况下最多等 2s 后放弃（原版只在 `close` 事件上 resolve，同样不等流结束） | 子进程刷日志时原版会无限吃内存；上限不影响正常诊断（失败原因在开头） | 已确认保留 |
| 超时 | 无（只有 abort） | 无（此前 pi-rs 自造的 300s 硬超时 + `timeoutSeconds` 参数已删除） | 完全对齐原版 | 已对齐 |

### 2026-10-26 删除的自造功能

以下能力是此前 pi-rs 版本自己加的，原版没有，已整体删除：

- `async: true` 后台运行 + `action: "status"` + `runId` 轮询（`subagent-runs/`
  `status.json`、`RUN_COUNTER`、`parse_output_jsonl` 及对应目录清理缺口）
- `PI_SUBAGENT_DEPTH_ENV` 深度限制（`MAX_DEPTH = 3`）与子进程强制
  `--no-extensions`（子 agent 现在可以再委派，与原版一致）
- 300s 硬超时 + `timeoutSeconds` 参数 + 超时 partial output 截断
- `task`-only + 临时 `model`/`tools` 参数（改为 agent 定义携带 `model`/`tools`，
  模型与 thinking level 从派发会话继承）

## web_search 扩展（`src/web_search.rs`）— 新增功能，非移植偏差

> 原版对照：npm 包 `@ollama/pi-web-search`（只调用本地 Ollama 实验性 API）。
> DeepSeek 后端是**用户明确要求的新增功能**，行为基准是
> <https://api-docs.deepseek.com/zh-cn/api/search> 与 `/api/fetch`；
> 它没有 TS 对照物，所以下面这张表描述的是**新功能的行为约定**，
> 不是"与原版不同"。对齐检查时不要拿它当偏差处理，也不要用它当
> 借口跳过其他条目的检查。

| 位置 | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---- | ---------- | ------------- | -------- | -------- |
| `web_search.rs` 后端 | 只有本地 Ollama（`POST {host}/api/experimental/web_search` / `web_fetch`），无鉴权 | 两个后端：**DeepSeek**（`POST https://api.deepseek.com/v1/tools/search` / `fetch`，`Authorization: Bearer`）+ **Ollama**（原行为不变） | 用户要求 web_search 支持 DeepSeek Search API | 已确认保留（新增功能） |
| 后端选择 | 无（固定 Ollama） | settings.json `webSearch.provider` / 环境变量 `PI_WEB_SEARCH_PROVIDER`，取值 `auto`（默认）/ `deepseek` / `ollama`；`auto` 时有 DeepSeek 凭据用 DeepSeek，否则 Ollama | 配置化（用户在 settings 里切后端，不依赖环境变量） | 已确认保留 |
| 端点/凭据配置 | `OLLAMA_HOST` / `OLLAMA_BASE_URL`（原版 `getOllamaHost()`） | 每档优先级「settings → 环境变量 → 内置默认」：`webSearch.provider` → `PI_WEB_SEARCH_PROVIDER`；`webSearch.deepseek.apiKey`（支持 `${ENV}`）→ `DEEPSEEK_API_KEY`；`webSearch.deepseek.baseUrl` → `DEEPSEEK_BASE_URL` → `https://api.deepseek.com`；`webSearch.ollama.host` → `OLLAMA_HOST`/`OLLAMA_BASE_URL` → `http://localhost:11434`。宿主 `create_agent_session` 还会把 `ModelRuntime` 解析到的 DeepSeek 凭据（auth.json / models.json / 环境变量）填进 `apiKey`，所以 `pi /login deepseek` 后无需环境变量 | 设置文件比环境变量更适合"选后端/换网关"；`${ENV}` 模板让密钥仍可留在环境里 | 已确认保留 |
| 配置无效 | （无对应） | `provider` 取值非法，或显式 `deepseek` 但无凭据 → 保留错误并在工具调用时返回，**不静默回落**到另一个后端；错误文案指明该改哪个配置项 | 静默 fallback 会让用户误以为在用 DeepSeek | 已确认保留 |
| 搜索参数映射 | `max_results` 原样透传（缺省 5） | Ollama 路径不变；DeepSeek 路径把 `max_results` 映射为 `count`（缺省 5，clamp 到 1～50），并按 DeepSeek 约束预校验 `query`（1～128 字符、不能全空白，否则返回可执行提示而非等 API 400）。不发送 `need_content`（默认 false：`summary` 已是给模型读的 600～800 字摘要）；`ori_question` / `localization` 在工具参数里没有对应字段 | 工具参数契约保持 Ollama 语义（模型无需区分后端），DeepSeek 侧做等价映射 | 已确认保留 |
| 搜索输出 | 用 `content` 字段 | DeepSeek 结果按 `summary` → `snippet` → `content` 取正文，输出文本与 Ollama 路径完全一致（`N. title / URL / body`）；`details.results` 保留 API 原始结果（含 `rerank_score` / `site_name` / `site_icon` / `date` / `authority`）；不把 `date`/`authority` 拼进文本 | 复用同一套输出契约，模型与渲染层无需分支 | 已确认保留 |
| 抓取输出 | `Title/Content/Links found` 三行 + 前 10 条链接 | 同左；DeepSeek Fetch 只返回 `title`+`content`，故为 `Links found: 0`、`details.links = []` | 输出结构统一 | 已确认保留 |
| 错误映射 | 连接失败 → `Could not connect to Ollama at …`；401 → `Unauthorized. Run \`ollama signin\`…`；非 2xx → `{tool} API error (status N): {body}`；pi-rs 另加 60s 超时文案 | Ollama 路径不变。DeepSeek：连接失败 → `Could not connect to DeepSeek at …`；401 → 提示 `webSearch.deepseek.apiKey` / `DEEPSEEK_API_KEY` / `/login deepseek`；按文档给 402（余额）/403（Fetch 被安全策略拒绝）/404（无可提取内容）/408、413/429（限流，建议稍后重试）/5xx（服务端错误）的可执行建议，并把 API 的 `error.message`（含 `request_id`）附在末尾；其余状态码走 `{tool} API error (status N): {error.message}` | DeepSeek 状态码语义与 Ollama 不同，模型/用户需要能照做的提示 | 已确认保留 |
| 鉴权头 | 无 | `Authorization: Bearer <key>` + `Content-Type: application/json`（reqwest `bearer_auth` + `json`） | DeepSeek 文档要求 | 已确认保留 |
| 请求重试 | 无（原版依赖调用方 signal） | 仍无自动重试（429/5xx 仅在错误文案里建议重试），保持与原版一致的"一次调用一次结果"语义 | 重试应由上层/用户决定；避免一次工具调用产生多次计费请求 | 已确认保留 |
