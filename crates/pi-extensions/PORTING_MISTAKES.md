# pi-extensions 移植错误归档

> 本文件记录移植过程中发现并修复的错误。`goal` 扩展（原
> `@narumitw/pi-goal` 移植）已于 2026-10-23 按用户要求整体删除，随它一起
> 修复的历史条目（B1/B2/B3、C1-C12、A3/A4 等，依据已归档的
> `docs/archive/GOAL_TS_COMPARISON_2026-08-24.md`）一并移除；索引见
> git 历史。
>
> 新条目按下表追加，根因模式尽量归到 `PORTING.md` 高危陷阱表的分类。

| 位置 | 现象 | 根因模式 | 修复方式 |
| ---- | ---- | -------- | -------- |
| `subagent.rs`（2026-10-26 重写前） | 子进程失败时用户只看到 `subagent produced no output (child agent failed or returned empty)`，没有任何原因（鉴权失败 / 402 / panic / spawn 失败全一个文案） | 静默兜底：`cmd.stderr(Stdio::null())` 丢诊断（注释里只为躲 64KB 管道写满），`exit_code` 与 `stopReason` 都没检查 | 重写为对齐原版：stderr 用独立 task `read_to_string` 持续 drain（既不死锁也不丢）；失败判定 `exitCode != 0 \|\| stopReason ∈ {error, aborted}`；失败输出按 `errorMessage → stderr → 终稿 → "(no output)"` 排序返回 |
| `subagent.rs`（同上） | 每个子 agent 的 token / cost 完全不可见；并行/链式一次调用都会把输出全量灌回父上下文 | 功能翻译遗漏：`message_end` 只取文本，`usage` 全丢；无长度上限 | 解析 `usage`（`input`/`output`/`cacheRead`/`cacheWrite`/`totalTokens`/`cost.total`）并按原版形状放进 `details`；并行输出按 50KB 字节截断并注明"全文在 details" |
| `agent_session.rs` 扩展 `RuntimeHandle::get_model` | 用户 `/model` 切换后，subagent 仍把**旧模型**传给子 agent | 构造期一次性快照（`format!("{provider}/{id}")` 闭包捕获），而 TS 的 `ctx.model` 是 getter | 抽出 `install_ext_agent_state_getters`：通过晚绑定的 `Arc<Mutex<Option<Arc<Agent>>>>` + `Agent::try_state()` 实时读模型与 thinking level，取不到快照时回退构造期值 |
| `pi-extension-api` `HookHandler::handle_tool_call` + `agent_session.rs` 扩展工具适配器 | 扩展工具拿不到中止信号与进度通道：适配器签名里的 `_signal, _on_update` 被直接丢弃，`handle_tool_call` 也没有对应入参 → 长跑工具（subagent）既不能上报进度，Ctrl+C 也停不下子进程 | 功能翻译遗漏：TS `execute(toolCallId, params, signal, onUpdate, ctx)` 的四个入参在移植时只保留了 params/ctx | 新增 `ToolInvocation{tool_call_id, abort, on_update}` + `ToolUpdateFn` + `wait_aborted`；适配器把 agent 的 signal/onUpdate 转成它；subagent 用 `tokio::select!` 等待中止、每条子进程消息 `invocation.update(...)` |
| `pi-tui` / `pi-coding-agent` 工具渲染 | 扩展无法自定义工具块外观（只有内置 bash/read/grep/edit/write 渲染器）：subagent 只能把原始 JSON details 塞给 fallback 渲染 | 缺失能力（TS `renderCall`/`renderResult`） | 新增 `ToolDefinition.render`（`ToolRenderContext` → `ToolRenderOutput`）+ `ToolCall.details`/`Msg::SetToolDetails`/`TranscriptItem::ToolResult.details` + `Model::set_custom_tool_render`（宿主把扩展回调适配成 `pi_tui::CustomToolRenderFn`）+ `ToolRenderer::Custom` 渲染分支 |
| `subagent.rs` 子进程 stderr | 子进程退出后仍等 stderr EOF：孙进程（例如子 agent 起的 bash 后台任务）持有管道写端时，整个工具调用会挂住 | 管道生命周期处理错误（原版只在 child `close` 事件 resolve，不等流结束） | stderr 写入共享缓冲，child 退出后只给 200ms 收尾再 `abort()` 读取任务，结果取缓冲快照 |
| `subagent.rs` 自造 async 后台模式（已删除） | `async: true` 返回 run_id 后没有任何完成通知/取消手段；`run_id` = 毫秒时间戳 + 进程内计数器（重启或多会话并发会撞名）；`{agent_dir}/subagent-runs/` 永不清理 | 擅自增强（原版没有该功能）+ 状态放文件系统当 IPC | 整体删除：工具回到原版"一次调用一次结果"，schema 不再双语义（`task`/`action` 并存） |
