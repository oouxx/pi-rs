# DEVIATIONS.md — pi-agent-core

按 CLAUDE.md 阶段四的偏差日志格式记录。对齐检查遇到下列差异时按对应
"确认状态"处理。

| # | 位置（文件/函数） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
|---|------------------|-----------|---------------|----------|----------|
| 1 | `AgentLoopConfig` / `StreamFnOptions`（`agent_loop.rs`、`types.rs`、`agent.rs`） | `AgentLoopConfig extends SimpleStreamOptions`：temperature/maxTokens/headers/timeoutMs/websocketConnectTimeoutMs/maxRetries/cacheRetention/toolChoice/serviceTier/metadata 等全量透传到每次 LLM 调用 | **已补齐**：`AgentLoopConfig`/`Agent`/`AgentOptions`/`StreamFnOptions` 增加同名字段并在 run_loop 的 stream_options 构建中透传（含 `test_simple_stream_options_passthrough` 端到端断言）；`pi_ai_types` 补 re-export `ToolChoice`/`ToolChoiceMode` | 对齐 TS 的 SimpleStreamOptions 透传契约 | 已确认保留 |
| 2 | `harness/session/state.rs`（SessionState/SessionMutation，TS `session/state.ts` 344 行） | TS 会话核心为 **lane 模型**：多 lane（main + 分支）、`SessionState` 状态机（entry/record/operation/sequence/labels/stats）、`SessionMutation` 变更协议，被 jsonl codec/storage/memory 依赖 | pi-rs 为**单 lane 简化**：`jsonl_storage.rs`/`memory_storage.rs` 直接提供 append_entry/get_entry/find_entries/get_label/get_leaf_id/set_leaf_id/get_path_to_root + repo（list/delete/fork），覆盖单 lane 会话的完整生命周期；无多 lane/record log/operation 协议 | 单 lane 场景行为一致（TS 默认 lane 即 main）；多 lane 是 harness-v2 前瞻能力（含未接线的 reducer.ts），pi-rs 当前不面向多 lane 场景 | 已确认保留 |
| 3 | `harness/session/search.rs`（SessionSearch，TS `session/search.ts` 71 行） | `createScanningSessionSearch` 用于会话选择器搜索 + sqlite session-backend | pi-rs 无会话搜索实现；无 sqlite backend | 会话搜索依赖 sqlite backend（pi-rs 范围外），会话选择器搜索属 TUI 层（范围外） | 已确认保留 |
| 4 | `harness/reducer.rs`（TS `harness/reducer.ts` 667 行） | `validateRecordLog`/`reduceLaneState` lane record log 归约 + `RecordLogCorruption` 检测 | 未移植 | **TS 生产代码也未 import**（仅 reducer.test.ts 引用，属 harness-v2 前瞻、未接线），pi-rs 不移植 | 已确认保留 |
| 5 | `harness/telemetry.rs`（TS `harness/telemetry.ts` 615 行） | AI span 级遥测（`AI_TELEMETRY_SCHEMA`：span 名/属性/事件 schema） | 未移植；pi-coding-agent 有简化 install-telemetry（开关 + 基础事件） | span 级遥测 schema 是 TS 内部可观测性协议，pi-rs 暂无对端消费者；需要时按 schema 移植 | 已确认保留 |
| 6 | `AgentHarness`（`agent_harness.rs`） | TS `AgentHarness implements AgentLane`：多 lane 创建/移动/挂起操作（createLane/move_lane/suspended）、lane 级错误（LaneBusy/NoActiveRun 等） | pi-rs 为单 lane AgentHarness：完整 API 面（model/thinking/tools/steer/follow_up/next_turn/prompt/subscribe/on）但无 lane 参数 | 与 #2 同源：多 lane 是 harness-v2 前瞻；pi-rs 面向单 lane 会话 | 已确认保留 |

> 备注：审查确认 agent_loop 核心（run_agent_loop/continue、事件序列 agent_start→agent_end、
> steering/follow-up/should_stop_after_turn/prepare_next_turn、length→fail truncated、
> 工具批量执行顺序/并行）、messages、compaction、session 单 lane 生命周期、proxy 均与
> TS 原版对齐。差异集中在多 lane 架构（#2/#6）及其下游（#3/#4），单 lane 行为一致。
| 7 | `harness/` 内置工具（TS `harness/tools/read.ts`/`write.ts`/`edit.ts`/`bash.ts`，v0.82.0 #e32c1491b） | TS harness 内置 context-aware 工具对象：`createReadTool`/`createWriteTool`/`createEditTool`/`createBashTool`，通过 `ExecutionToolContext`（`{env: ExecutionEnv}`）执行，应用可注入自定义 toolContext；`ShellExecOptions.inheritEnv`、`~`/`file://` 路径处理、shell 输出捕获（子进程退出后 drain）、跨平台进程清理 | pi-rs harness 工具为**事件转发模式**：工具是名字列表（`tools: Vec<String>`），执行通过 `before_tool_call`/`after_tool_call` hooks 转发给外部事件处理器，无内置工具对象；`ExecutionEnv` trait 已有（文件系统 + shell 抽象），`env/nodejs.rs` 实现缺 `~`/`file://` 路径处理与 `inheritEnv` | pi-rs harness 无外部使用者（孤儿模块，pi-cli/pi-coding-agent/pi-tui 均不引用），工具由使用方注入；补内置工具对象等于改变 harness 架构而非补缺失功能。`ExecutionEnv` 行为对齐（`~`/`file://`/`inheritEnv`）属可选小增强，待 harness 有实际使用者时再补 | 已确认保留（架构差异） |
