# PORTING_MISTAKES.md — pi-agent-core

对齐检查（v0.80 → v0.81）中修复的回归 bug 归档。根因模式尽量归到
`PORTING.md` 高危陷阱表已有分类；新出现的模式已同步回该表。

| 位置 | 现象 | 根因模式 | 修复方式 |
| ---- | ---- | -------- | -------- |
| `harness/prompt_templates.rs` `substitute_args` | 不支持 `${N:-default}` / `${@:-default}` / `${ARGUMENTS:-default}`（TS v0.81.0 #6695）；且 `${@:N:L}` 把 L 当绝对 end 而非 length（TS 是 `args.slice(start, start+L)`） | 正则翻译遗漏分支 + 切片语义翻译错误 | 重写为单个正则（对齐 TS `substituteArgs`）：default 分支（value 空时用 default，不递归替换）、slice 分支（L 是 length，越界 clamp）、simple 分支；补默认值/切片语义测试 |
| `types.rs` `AgentMessage::ToolResult` / `AgentToolResult` | 缺 `usage` 字段（TS v0.81.0 #6671：工具可报告 LLM usage，透传到 ToolResultMessage） | 类型定义翻译遗漏新增字段 | 加 `usage: Option<Usage>`（serde skip none），`create_tool_result_message` 透传 `finalized.result.usage`，补透传测试 |
| `harness/types.rs` `SessionStorage` trait + `jsonl_storage.rs`/`memory_storage.rs` | 未按 TS v0.81.0 #6594 更新：`getPathToRootOrCompaction`（retainedTail 自包含 checkpoint / firstKeptEntryId 停）、`getSessionName`/`getSessionStats`、cursor-based `getEntries`、`CompactionEntry.retainedTail`/`firstKeptEntryId?` | 公开 API breaking change 未同步 | trait 改名 + 3 个新方法 + cursor 参数；两个 storage 实现新语义（walk 遇 retainedTail 停 / firstKeptEntryId 停）；`CompactionPreparation` 加 `retained_tail`，`prepare_compaction` 生成，`append_compaction` 透传；`build_session_context` 展开 retainedTail；补 3 个测试 |
| `harness/session/*.rs` `generate_entry_id` | entry id 用 `Uuid::new_v4` 前缀 8 字符（TS v0.81.0 #6834 改用 uuidv7 随机尾部 8 字符，避免时间戳前缀近常量） | 高危陷阱：数值语义翻译错误（v4 前缀 vs v7 尾部） | 改用 `uuid_v7()` 尾部 8 字符 |
| `harness/compaction/compaction.rs` + `branch_summarization.rs` | compaction/branch-summary 的 LLM 调用无 retry（TS v0.81.1 #6901：`completeSummarization` 用 `retryAssistantCall` 包裹） | 功能翻译遗漏（新行为） | 加 `complete_summarization`（pi_complete 的 Err 归一化为 error AssistantMessage → retry_assistant_call → Err 还原）；`generate_summary`/`compact`/`generate_branch_summary` 加 retry/callbacks 参数 |
| `agent.rs` `Agent::new` | `stream_fn` fallback 到返回 Err 的函数（TS v0.81.0 #6851/#6915：`getDefaultStreamFn()` 未配置时 throw） | 静默 fallback（CLAUDE.md 高危陷阱：`?? defaultValue` 掩盖异常） | 加 `set_default_stream_fn`/`get_default_stream_fn`（OnceLock 全局）；`Agent::new` 未配置时 panic（对齐 TS throw） |
