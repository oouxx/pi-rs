# pi-extensions 契约级对齐表

> 当前内置扩展：`subagent`、`web_search`。`goal` 扩展（原
> `@narumitw/pi-goal` 移植：`/goal` 命令、`goal_complete`/`goal_blocked`/
> `goal_wait` 工具、状态机与事件序列）已于 2026-10-23 按用户要求整体删除，
> 对应的契约表一并移除。
>
> "否"的差异必须引用 `DEVIATIONS.md`。

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
