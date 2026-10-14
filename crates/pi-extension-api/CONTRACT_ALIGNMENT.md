# pi-extension-api 契约级对齐表

> 对齐基准：见根目录 [`.version`](../../.version)。
> 结构性的实现差异（程序化注册 vs 文件驱动加载、V8 shim、无热重载）登记在根目录
> [`DEVIATIONS.md`](../../DEVIATIONS.md) 的「扩展系统」小节，本文件只记录**公开 API 与
> 事件语义**的契约。

本 crate 是扩展系统的对外 API 层：TS 侧对应 `packages/coding-agent/src/core/extensions/`
的 `types.ts`（事件与结果类型）、`runner.ts`（分发器）与 `ExtensionRunner` 的公开面。

## 核心公开面

| 行为场景 | TS 版本行为 | Rust 版本行为 | 是否一致 | 差异原因（如有） |
| -------- | ----------- | ------------- | -------- | ---------------- |
| 扩展注册 | `pi.on(event, handler)` / `pi.registerTool/Command/Shortcut/Flag` 由加载器驱动 | `HookHandler` trait：`name`/`priority` + `register_tools`/`register_commands`/`register_shortcuts`/`register_flags` + 各 `on_*` hook，`ExtensionRegistry::register(Box<dyn HookHandler>, SourceInfo)` | 是（接口语义） | 注册方式本身是结构性偏差，见根 DEVIATIONS「扩展系统」 |
| 分发顺序 | 按 priority 排序，modifying hook 顺序执行、首个 Cancel 生效；通知类并行 | 同左（`HookRunner` 按 `priority()` 降序，`run_*` 顺序、`fire_*` 并行） | 是 | |
| `context` 相位 | handler 只见对话（无 system 消息），Pi 跑完用 `getCurrentSystemMessage` 补回 prompt/工具声明 | 同左（`run_on_context`） | 是 | 修复见 PORTING_MISTAKES.md #145 |
| `context_with_system` 相位 | 在 `context` 之后运行，看到完整 transcript，结果原样生效；丢掉首条 system 要报错 | 同左（`on_context_with_system` + `run_context_with_system` 返回违规 handler 名） | 是 | 同上 |
| `turn_end` / `agent_before_settle` 边界 | `emitBoundary(baseEvent, buildContext)`；handler 返回 `{entries, continue}`；`turn_end` 不参与普通 `emit()` | `run_boundary(BoundaryKind, BoundaryState, build_context)` + `BoundaryState`/`BoundaryResult`；void `on_turn_end` 通知保留 | 是（有意偏差：void `on_turn_end` 保留） | 见根 DEVIATIONS 扩展边界条目 |
| `cache_warming_decision` | handler 可返回 `{action}` 覆盖 pi 的判断 | 同左（`on_cache_warming_decision` + `fire_cache_warming_decision`，最后一个非空生效） | 是 | |
| `user_bash` | fail-closed：handler 抛错或返回非法结果即中止；返回 `undefined` 继续传播 | 同左（`on_user_bash` 返回 `Option<UserBashResult>`，非法结果 fail-closed）；`{operations}` 形式不支持 | 是（`operations` 除外） | 见根 DEVIATIONS `user_bash operations` |
| 工具参数 schema | 注册期拒绝无 object schema 的工具（#9300） | `ExtensionRegistry::validate_tools()` 逐条返回违规消息，`sdk.rs` 并入 `ExtensionsResult.errors`，会话丢弃该工具 | 是（Rust 为程序化注册，取「拒绝工具 + 上报」） | 修复见 PORTING_MISTAKES.md #148 |
| 事件/结果类型导出 | 包入口导出 hook 事件与结果类型（#9642） | 全部 `pub`（`lib.rs` 重导出 `hook::*` 与各事件类型） | 是 | |
