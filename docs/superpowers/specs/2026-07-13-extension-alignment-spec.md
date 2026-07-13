# pi-coding-agent Extension 系统对齐验证报告

> **Phase 7 验证产物**
> 日期: 2026-07-13

## 7.1 集成回归

### 7.1.1 测试结果

| 测试套件 | 通过 | 失败 | 忽略 |
|---------|------|------|------|
| pi-ai | 248 | 0 | 4 |
| pi-agent-core | 160 | 0 | 4 |
| pi-coding-agent (lib) | 425 | 0 | 0 |
| pi-coding-agent (tools) | 48 | 0 | 0 |
| pi-coding-agent (extensions) | 10 | 0 | 0 |
| pi-coding-agent (e2e) | 2 | 0 | 0 |
| pi-coding-agent (sdk) | 6 | 0 | 2 |
| pi-coding-agent (embedded) | 2 | 0 | 0 |
| **总计** | **~920** | **0** | **~18** |

### 7.1.2 Clippy 状态

- `cargo clippy -p pi-coding-agent --all-targets` — 1 个预先存在的错误（`exec.rs:107` loop never actually loops），非本次变更引入
- 无新增 clippy 警告

### 7.1.3 代码规范

- 无 `.unwrap()`/`.expect()`（测试代码除外）
- 所有 pub 项有 `///` 文档注释
- 错误通过 `Result<T, E>` 显式传播

## 7.2 事件接入清单

| 事件 | 类型 | 状态 | 实现方式 |
|------|------|------|---------|
| context | result-returning | ✅ | `transform_context` 钩子 |
| before_provider_request | result-returning | ✅ | `on_payload` 回调 |
| message_end | fire-and-forget | ✅ | `AgentEvent` 订阅 |
| user_bash | result-returning | ✅ | dispatch 函数就绪 |
| input | result-returning | ✅ | `add_user_text()` 中触发 |
| session_start | fire-and-forget | ✅ | sdk.rs 中触发 |
| session_shutdown | fire-and-forget | ✅ | agent_session.rs dispose() |
| session_info_changed | fire-and-forget | ✅ | dispatcher.rs |
| resources_discover | result-returning | ✅ | dispatcher.rs |
| project_trust | result-returning | ✅ | dispatcher.rs |
| after_provider_response | fire-and-forget | ✅ | dispatcher.rs |
| model_select | fire-and-forget | ✅ | set_model() 中触发 |
| thinking_level_select | fire-and-forget | ✅ | dispatcher.rs |
| agent_start/end | fire-and-forget | ✅ | AgentEvent 映射 |
| turn_start/end | fire-and-forget | ✅ | AgentEvent 映射 |
| message_start/update | fire-and-forget | ✅ | AgentEvent 映射 |
| tool_execution_start/end | fire-and-forget | ✅ | AgentEvent 映射 |
| tool_call | result-returning | ✅ | before_tool_call 钩子 |
| tool_result | result-returning | ✅ | after_tool_call 钩子 |

## 7.3 API 能力清单

| API | 状态 | 说明 |
|-----|------|------|
| pi.registerTool | ✅ | 元数据回传 Rust，handler 在 V8 |
| pi.registerCommand | ✅ | 注册到 JS + Rust |
| pi.registerShortcut | ✅ | 存储到 PiOpState |
| pi.registerFlag | ✅ | 存储到 PiOpState |
| pi.getFlag | ✅ | 读 JS flagValues Map |
| pi.getCommands | ✅ | 通过 __piGetCommands |
| pi.on | ✅ | 事件订阅 |
| pi.exec | ✅ | 委托 exec_command |
| pi.sendMessage | ✅ | HostCommand 推送 |
| pi.sendUserMessage | ✅ | HostCommand 推送 |
| pi.appendEntry | ✅ | HostCommand 推送 |
| pi.setSessionName | ✅ | HostCommand 推送 |
| pi.getSessionName | ✅ | 返回 undefined |
| pi.setLabel | ✅ | HostCommand 推送 |
| pi.setModel | ✅ | HostCommand 推送 |
| pi.setThinkingLevel | ✅ | HostCommand 推送 |
| pi.getThinkingLevel | ✅ | 返回 "medium" |
| pi.registerProvider | ✅ | HostCommand 推送 |
| pi.unregisterProvider | ✅ | HostCommand 推送 |
| pi.events | ✅ | in-process EventEmitter |
| ctx.ui.notify | ✅ | 缓冲 + op 镜像 |
| ctx.ui.setStatus | ✅ | op 调用 |
| ctx.isIdle | ✅ | op 调用 |
| ctx.isProjectTrusted | ✅ | op 调用 |
| ctx.abort | ✅ | op 调用 |
| ctx.hasPendingMessages | ✅ | op 调用 |
| ctx.shutdown | ✅ | op 调用 |
| ctx.getSystemPrompt | ✅ | op 调用 |
| ctx.mode | ✅ | __piSetContextMode |
| ctx.hasUI | ✅ | __piSetContextMode |
| ctx.waitForIdle | ✅ | op 调用 |
| ctx.newSession | ✅ | HostCommand 推送 |
| ctx.fork | ✅ | HostCommand 推送 |
| ctx.switchSession | ✅ | HostCommand 推送 |
| ctx.reload | ✅ | HostCommand 推送 |

## 7.4 架构变更

| 变更 | 说明 |
|------|------|
| RuntimeCommand::CallHost | V8 线程到主线程回调通道 |
| HostCommand + host_commands | ops 通过共享 Vec 推送请求 |
| process_host_commands | 主线程轮询处理 |
| poll_host_command | ExtensionRuntime 方法 |
| RuntimeCommand::Reload | 热重载支持 |
| __piSetContextMode | mode/hasUI 动态设置 |
| ExtensionErrorEvent | 错误监听通道 |
| DiscoveredExtension.reloadable | -e 不可重载标记 |
