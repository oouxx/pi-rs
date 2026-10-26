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
| HookHandler::on_turn_end | 带 ctx | 已带 ctx；当前内置扩展未使用，API 保留供其他扩展 | 已对齐 | — |
| RuntimeHandle.abort | 接线到 agent.abort | 已接线（spawn 任务调 agent.abort，不 wait_for_idle） | 同步 Fn 不能 await | 已确认保留 |
| 事件 ctx（通知 + transform） | TS `ExtensionHandler<E, R> = (event: E, ctx: ExtensionContext)`：**所有**事件 handler 都收到 `ExtensionContext` | **已对齐（2026-10-13）**：`HookHandler` 全部方法均携带 ctx——`handle_tool_call`、`before_tool_call` 为必填 `&ExtensionContext`；通知类 void hook（session_start/shutdown/info_changed、agent_start、turn_start、message_start/update/end、tool_execution_start/update/end、model_select、thinking_level_select、compact、tree、resources_discover、project_trust、context、user_bash）及 `agent_end`/`agent_settled`/`turn_end` 为 `Option<&ExtensionContext>`（无绑定会话的测试/直接 Agent 场景为 `None`）；transform/modifying hook（`on_message_end_mut`、`on_context_mut`、`on_tool_result_mut`、`after_tool_call`、`before_agent_start`、`on_input`、`before_provider_request`、`before_provider_headers`、`after_provider_response`、`before_session_switch/fork/compact/tree`）为必填 `&ExtensionContext`。无需扩展侧兜底 | 通知类事件签名对齐 TS `(event, ctx)`；required/option 的区分对应“是否绑定真实会话” | 已对齐 |

## web_search 扩展（`src/web_search.rs`）

> 原版对照：npm 包 `@ollama/pi-web-search`（只调用本地 Ollama 实验性 API）。
> DeepSeek 后端是**用户明确要求的新增功能**，行为基准是
> <https://api-docs.deepseek.com/zh-cn/api/search> 与 `/api/fetch`。

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
