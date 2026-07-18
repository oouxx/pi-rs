# AgentSession TS → Rust 对齐 — 剩余任务

跟踪 `crates/pi-coding-agent/src/core/agent_session.rs` 中仍需对齐的 TS 方法和行为。

总进度：~85% 公开方法已对齐。以下 7 项有意保持简化。

---

## 1. `get_error_message()` — 返回错误信息

**状态:** ❌ always `None`

**TS 行为:**
```typescript
// 从 agent.state.error_message 读取
get errorMessage(): string | undefined {
    return this.agent.state.errorMessage;
}
```

**Rust 当前:**
```rust
pub fn get_error_message(&self) -> Option<&str> {
    None
}
```

**原因:** `AgentState` 有 `error_message: Option<String>` 字段，但 Rust 的 `AgentState` 在创建后不会更新此字段。需要在 agent loop 中注入错误记录逻辑。

**工作量:** 小。改为 `self.agent.state().await.error_message.map(|s| s.as_str())`，但需要 async + 生命周期处理。

---

## 2. `get_context_usage()` — 上下文使用统计

**状态:** ❌ 返回 `ContextUsage::default()`

**TS 行为:**
```typescript
getContextUsage(): ContextUsage | undefined {
    const model = this.model;
    if (!model) return undefined;
    const contextWindow = model.contextWindow ?? 0;
    if (contextWindow <= 0) return undefined;
    // 从最新 assistant 消息的 usage 计算
    // 考虑 compaction 边界后的 token 统计
}
```

**Rust 当前:**
```rust
pub fn get_context_usage(&self) -> ContextUsage {
    ContextUsage::default()
}
```

**依赖:**
- `model.context_window` ✅ 已有
- assistant 消息的 token usage 跟踪（需要 `AgentMessage::Assistant` 在 Rust 端携带 usage 信息）
- compaction 边界检测

**工作量:** 中。主要是 messages 中提取 usage 的逻辑。

---

## 3. `should_compact()` — 压缩检查

**状态:** ❌ always `false`

**TS 行为:**
```typescript
// 调用 compaction.shouldCompact(contextTokens, contextWindow, settings)
```

**Rust 当前:**
```rust
pub fn should_compact(&self) -> bool {
    false
}
```

**依赖:**
- `compaction::should_compact()` 已存在（在 `check_auto_compact()` 中使用）
- 但此方法是 `&self`（同步），而获取 messages 需要 async
- 异步版本 `check_auto_compact()` 已实现（`pub async fn`）

**工作量:** 小。可以将当前实现改为：
```rust
pub async fn should_compact(&self) -> bool {
    self.check_auto_compact().await
}
```
需要调用方改为 `.await`。

---

## 4. `steering_mode` / `set_steering_mode()` / `followUpMode` / `set_follow_up_mode()` — 队列模式

**状态:** ❌ 未实现

**TS 行为:**
```typescript
get steeringMode(): "all" | "one-at-a-time" { return this.agent.steeringMode; }
setSteeringMode(mode: "all" | "one-at-a-time"): void {
    this.agent.steeringMode = mode;
    this.settingsManager.setSteeringMode(mode);
}
```

**Rust 当前:** 不存在

**依赖:**
- Agent 已有 `steering_mode()` / `set_steering_mode()`（`pi-agent-core/src/agent.rs:331`）
- Agent 已有 `follow_up_mode()` / `set_follow_up_mode()`（`pi-agent-core/src/agent.rs:339`）
- 但 Rust 的队列模式类型是 `QueueMode`（enum），TS 用 `"all" | "one-at-a-time"` 字面量

**工作量:** 小。映射：
```rust
pub async fn steering_mode(&self) -> QueueMode {
    self.agent.steering_mode().await
}
pub async fn set_steering_mode(&self, mode: QueueMode) {
    self.agent.set_steering_mode(mode).await;
}
```

---

## 5. `set_auto_compaction_enabled()` — 自动压缩开关

**状态:** ❌ 未实现

**TS 行为:**
```typescript
setAutoCompactionEnabled(enabled: boolean): void {
    this.settingsManager.setCompactionEnabled(enabled);
}
get autoCompactionEnabled(): boolean {
    return this.settingsManager.getCompactionEnabled();
}
```

**依赖:**
- Rust AgentSession 没有 `settingsManager` 引用
- `CompactionSettings` 存在但存储在 AgentSession 中，没有同步到外部配置
- TS 版本会持久化到磁盘配置

**工作量:** 小。可以直接操作 `self.compaction_settings`，但不会持久化。

---

## 6. `reload()` — 运行时重新加载

**状态:** ❌ 未实现

**TS 行为:**
```typescript
async reload(options?: { beforeSessionStart?: () => void }): Promise<void> {
    // 1. 发送 session_shutdown 事件
    // 2. 重新加载 settings / resourceLoader
    // 3. 重建 runtime（包括 ExtensionRunner）
    // 4. 重发 session_start 事件
    // 5. 重新发现扩展资源
}
```

**依赖:**
- 需要存储完整的构造参数或重建逻辑
- 需要重新创建 ExtensionRegistry（TS 通过 ExtensionRunner 的重建实现）
- 当前 Rust 架构没有在 AgentSession 中保存可重入的构造配置

**工作量:** 大。架构级改动 —— 需要让 AgentSession 可重建。

---

## 7. `export_to_jsonl()` — 导出会话

**状态:** ❌ 未实现

**TS 行为:**
```typescript
exportToJsonl(outputPath?: string): string {
    // 写 session header + branch entries 到 JSONL 文件
}
```

**Rust 当前:** 不存在

**工作量:** 小。可以直接复用 `session_manager` 的数据。

---

## 优先级建议

| 优先级 | 项目 | 工作量 | 影响面 |
|---|---|---|---|
| P0 | steering_mode / follow_up_mode | 小 | 中等（TUI 可用） |
| P0 | should_compact → async | 小 | 低（auto-compact 已通过 check_auto_compact 工作） |
| P1 | get_context_usage | 中 | 中等 | |
| P1 | export_to_jsonl | 小 | 低 |
| P2 | get_error_message | 小 | 低 |
| P2 | set_auto_compaction_enabled | 小 | 低 |
| P3 | reload | 大 | 低（极少使用） |
