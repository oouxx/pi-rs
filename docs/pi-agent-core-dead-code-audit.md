# pi-agent-core dead_code 审计

逐项对照 TS 原版（`packages/agent/`）检查，判断是"我们自己造的"还是"原版有但没移植"。

---

## 1. `drain_queue` — 自己造了但没接上

**位置:** `harness/agent_harness.rs:460`

**原版:** TS 有 `drainQueuedMessages()`（`agent-harness.ts:387`），并且在 `createLoopConfig()` 的 `getSteeringMessages`、`getFollowUpMessages` 回调中调用（lines 445-446）。

**Rust:** 也定义了 `get_steering_messages` 和 `get_follow_up_messages`（lines 818-847），但把 drain 逻辑**内联写了**，没有调用 `drain_queue` 方法。而且内联版本缺失了 TS 调用 `emitQueueUpdate()` 的行为。

**结论:** 我们自己写了 `drain_queue()` 工具方法但没接上。内联替代品实现了核心功能，只是少了 `emit_queue_update()`。

**修复:** 删除 `drain_queue` 方法（死代码），或者让 `get_steering_messages`/`get_follow_up_messages` 调用它。

---

## 2. `TurnState` 三个字段未使用

**位置:** `harness/agent_harness.rs:50-60`

| 字段 | TS 是否通过 turnState 使用 | Rust 是否通过 turnState 使用 |
|---|---|---|
| `stream_options` | ✅ 是（`turnState.streamOptions`, line 362） | ❌ 从 `self.stream_options` 读 |
| `tools` | ✅ 是（`turnState.tools`, `activeTools` 多次使用） | ❌ 从 `self.tools` 读 |
| `active_tools` | ✅ 是（`turnState.activeTools.slice()`, line 355） | ❌ 从 `self.active_tool_names` 读 |

**结论:** 我们抄了 TS TurnState 的结构但没把读取路径换成 turnState。原版这些字段都是通过 `turnState.xxx` 读取的。Rust 把它们存在 TurnState 里但读的时候绕过了它。

**修复:** 把 `execute_turn()`（`agent_harness.rs:700+`）中对 `self.stream_options`、`self.tools`、`self.active_tool_names` 的引用改为从 `turn_state` 读取。

---

## 3. `resolve_kind` / `add_ignore_rules` — diagnostics 未 push ⚠️ **真实功能缺失**

**位置:** `harness/skill_loader.rs:288`, `:317`

**原版:** TS `resolveKind()`（`skills.ts:319`）在 symlink 解析失败时 push 诊断：
```typescript
// TS skills.ts:328-333
diagnostics.push({
    type: "warning",
    code: "file_info_failed",
    message: canonicalPath.error.message,
    path: info.path,
});
```
同样 `addIgnoreRules()`（`skills.ts:177`）在文件读取失败时也会 push。

**Rust:** `resolve_kind` 和 `add_ignore_rules` 都接受 `diagnostics` 参数，但**从不向其中 push 任何内容**。技能加载时的文件错误被静默吞掉。

**结论:** ❌ 功能缺失。原版会记录文件类型解析失败、忽略文件读取失败的诊断信息，Rust 不会。

**修复:**
```rust
// resolve_kind 在 symlink/canonicalize 失败时:
diagnostics.push(SkillDiagnostic {
    path: info.path.clone(),
    message: format!("Failed to resolve file kind: {e}"),
    code: "file_info_failed",
});

// add_ignore_rules 在文件读取失败时同样 push
```

---

## 4. `Named` trait 可见性

Rust-only。原版没有对应概念。不是问题。

---

## 改进后的汇总

| 项 | 来源 | 严重度 | 修复 |
|---|---|---|---|
| `drain_queue` | 我们造的 | 低 | 删除或用起来 |
| `TurnState` 三个字段 | 抄了但没改全 | 低 | 改为从 `turn_state` 读 |
| `resolve_kind` diagnostics | **功能缺失** | ⚠️ 中 | 补 push |
| `Named` trait | Rust 独有的 | 无 | 不管 |
