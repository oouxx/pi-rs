# 扩展 UI 协议（Extension UI Protocol）

> 本文档定义 pi-rs 扩展与客户端（RPC / ACP / 未来 GUI）之间的 UI 交互协议。
> 目标：**与 TUI 渲染层解耦**——扩展不直接渲染，而是发出结构化 UI 请求，
> 任何了解协议的客户端都可以消费（TUI 只是未来的一个消费者）。

## 设计原则

1. **TUI 无关**：协议是纯 JSON 数据，不依赖终端渲染。
2. **客户端无关**：RPC、ACP、未来 GUI 通过各自的传输通道承载同一协议。
3. **两种交互**：
   - **fire-and-forget**（通知类）：客户端展示即可，无需回复。
   - **dialog**（交互类）：客户端必须回复（选择/输入/确认），超时可取消。
4. **来源**：行为与消息格式对齐原版 TypeScript 的
   `RpcExtensionUIRequest` / `RpcExtensionUIResponse`
   （`packages/coding-agent/src/modes/rpc/rpc-types.ts`）。

## 2. 协议消息

### 2.1 请求（客户端 → 客户端展示）：`extension_ui_request`

fire-and-forget：

```jsonc
// 通知
{ "type": "extension_ui_request", "method": "notify",
  "message": "string", "notifyType": "info|warning|error" }
// 状态栏
{ "type": "extension_ui_request", "method": "setStatus",
  "statusKey": "string", "statusText": "string|null" }
// 会话内小组件
{ "type": "extension_ui_request", "method": "setWidget",
  "widgetKey": "string", "widgetLines": ["string"] | null,
  "widgetPlacement": "aboveEditor|belowEditor" }   // 可选
// 窗口标题
{ "type": "extension_ui_request", "method": "setTitle", "title": "string" }
// 输入框预填
{ "type": "extension_ui_request", "method": "set_editor_text", "text": "string" }
```

dialog（客户端必须回复）：

```jsonc
{ "type": "extension_ui_request", "id": "uuid", "method": "select",
  "title": "string", "options": ["string"], "timeout": "number" }  // timeout 可选
{ "type": "extension_ui_request", "id": "uuid", "method": "confirm",
  "title": "string", "message": "string", "timeout": "number" }    // timeout 可选
{ "type": "extension_ui_request", "id": "uuid", "method": "input",
  "title": "string", "placeholder": "string", "timeout": "number" }// placeholder/timeout 可选
{ "type": "extension_ui_request", "id": "uuid", "method": "editor",
  "title": "string", "prefill": "string" }
```

### 2.2 响应（客户端 → 扩展）：`extension_ui_response`

```jsonc
{ "type": "extension_ui_response", "id": "uuid", "value": "string" }      // select/input/editor
{ "type": "extension_ui_response", "id": "uuid", "confirmed": true }      // confirm
{ "type": "extension_ui_response", "id": "uuid", "cancelled": true }      // 任意 dialog 取消
```

## 3. 传输层

| 模式 | 通道 | fire-and-forget | dialog |
| --- | --- | --- | --- |
| **RPC**（`--mode rpc`） | stdout 独立 JSON 行 `extension_ui_request`；客户端从 stdin 回 `extension_ui_response`（`id` 关联） | ✅ | ✅（`confirm` 因同步接口限制暂回默认） |
| **ACP**（`--acp`） | `session/update` 通知的 `update.sessionUpdate = "session_info_update"` + `_meta.extensionUiRequest` 字段 | ✅ | ❌（ACP 无客户端回复通道，回默认值） |
| **print / json** | stderr（无 UI 客户端时的 fallback） | 仅日志 | ❌（回默认） |
| **GUI（未来）** | 直接消费协议行（建议与 RPC 相同的 JSON 行传输） | ✅ | ✅ |

## 4. 扩展侧入口

扩展通过 `ExtensionUIContext` 发出 UI 请求（`ctx.ui.*`），运行时按当前模式
接线到协议传输层：

| 扩展 API | 协议 method |
| --- | --- |
| `ctx.ui.notify(msg, {level})` | `notify` |
| `ctx.ui.set_status(key, text)` | `setStatus` |
| `ctx.ui.set_widget(key, lines, opts)` | `setWidget` |
| `ctx.ui.set_title(title)` | `setTitle` |
| `ctx.ui.set_editor_text(text)` | `set_editor_text` |
| `ctx.ui.confirm(title, msg)` | `confirm` |
| `ctx.ui.select(title, options, opts)` | `select` |
| `ctx.ui.input(title, placeholder, opts)` | `input` |

接线位置：

- RPC：`crates/pi-coding-agent/src/modes/rpc/mod.rs::create_rpc_ui_context`
- ACP：`crates/pi-coding-agent/src/modes/acp/session.rs::create_acp_ui_context`
- 无 UI 客户端 fallback：`agent_session.rs::default_extension_ui`

## 4. 客户端实现指引（GUI）

1. 打开 pi-rs（`--mode rpc`），从 stdout 逐行读 JSON。
2. 遇到 `type == "extension_ui_request"` 时按 `method` 展示 UI。
3. dialog 类必须回一行 `{"type":"extension_ui_response","id":<请求的 id>,...}`。
4. `setStatus`/`setWidget` 按 `statusKey`/`widgetKey` 更新对应区域。
