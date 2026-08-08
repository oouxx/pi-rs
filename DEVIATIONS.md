# DEVIATIONS.md — pi-coding-agent

按 CLAUDE.md 阶段四的偏差日志格式记录。这里的条目状态均为"已确认
保留"：对齐检查（无论人工还是 Claude Code 自动跑）遇到下面两类差异
时，直接跳过，不得尝试"纠正"回原版实现。

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| 扩展系统（extension 模块整体） | `packages/coding-agent` 里扩展系统的具体实现方式（如插件加载、hook 注册的内部机制） | 内部实现方式不按原版逐行翻译，改用 Rust 生态更合适的机制（如 WASM / subprocess IPC，具体选型见 PORTING.md） | 用户决定 | 已确认保留 |
| TUI 渲染层（对应原 `packages/coding-agent` 内 TUI 组件，即 `pi-tui`） | 原版 TUI 组件的具体渲染实现 | 本次范围内不复刻 | 用户决定：TUI 不在 pi-coding-agent 当前移植范围内 | 已确认保留 |

## 对"已确认保留"条目的额外约束

### 扩展系统

**允许偏离的范围：内部实现。不允许偏离的范围：对外 interface 和函数
行为。** 具体来说：

- 扩展系统暴露给上层（`pi-coding-agent` 其他模块、以及未来 `pi-tui`
  如果要接入）的公开函数签名、参数、返回类型、错误类型、事件
  variant，必须和原 TS 版本在语义上一致——即阶段一分析文档里"对外
  接口"那一节列出的东西，不能因为内部实现变了就跟着变
- 阶段三的"契约级：接口行为对齐"对扩展系统模块**仍然适用，不能因为
  这条已经登记在 DEVIATIONS.md 就跳过**。DEVIATIONS.md 里免检的是
  "内部怎么实现"，不是"对外行为对不对"。换句话说，行为对照表里
  "是否一致"这一列，扩展系统模块依然要填真实结果，不能因为有这条
  登记就默认填"是"
- 如果后续发现内部实现方式的改变导致对外行为出现了不该有的偏差
  （比如错误类型变了、事件触发时机变了），这属于新的、未登记的偏差，
  走 CLAUDE.md 4.2 节的"未记录偏差"流程处理，不能套用这条已确认的
  登记

### TUI

- 本条目只覆盖"不逐行复刻/不在本次范围内实现"这件事本身，不代表
  TUI 以后也不需要对齐检查——一旦 TUI 部分被纳入某次任务范围，需要
  单独走阶段一到阶段三，这条 DEVIATIONS 记录届时应该更新或移除，不
  要留着一条过期的"已确认保留"误导后续判断
- 在此之前，任何对齐检查、契约对照表遇到 TUI 相关的公开接口缺失，
  视为"范围外"，不算作差异，不需要在对照表里体现
| `reload()` / `_buildRuntime()` | TS `reload()` 调用 `_buildRuntime()` 重建整个 ExtensionRunner（重新从磁盘加载扩展文件、重建工具注册表、重新绑定所有回调） | Rust `reload()` 只调用 `settings_manager.reload()`，不重建 ExtensionRegistry | Rust 扩展通过 `Arc<ExtensionRegistry>` 在构造时一次性注册，运行时不支持热重载。TS 扩展是文件驱动的动态加载（`.ts`/`.js` 文件 → ResourceLoader → ExtensionRunner），Rust 扩展是程序化注册的静态引用（`registry.register()` → `Arc<ExtensionRegistry>`），没有"运行时重建"的概念 | 已确认保留 |
| `bind_extensions()` | TS `bindExtensions()` 设置 `_extensionUIContext`、`_extensionMode`、`_extensionCommandContextActions`、`_extensionAbortHandler`、`_extensionShutdownHandler`、`_extensionErrorListener`，然后调用 `_applyExtensionBindings()` 和 emit `session_start` | Rust `bind_extensions()` 接受 `ExtensionBindings` 结构体，存储相关字段但不执行动态绑定 | Rust 扩展通过 `ExtensionContext` 和 `EventPublisher` 直接通信，没有 TS 的 ExtensionRunner 回调注册机制。`_applyExtensionBindings()` 的等价逻辑在构造时通过 `ExtensionContext::new()` 完成 | 已确认保留 |
| `create_replaced_session_context()` | TS 从 `_extensionRunner.createCommandContext()` 创建 `ReplacedSessionContext`，附加 `sendMessage`/`sendUserMessage` 方法 | Rust 返回一个简化的 `ReplacedSessionContext` 结构体，包含 `send_message`/`send_user_message` 闭包 | Rust 没有 ExtensionRunner 的 `createCommandContext()` 方法。等价功能通过 `ExtensionContext` 直接暴露给扩展 | 已确认保留 |
| `ExtensionEvent` / `ExtensionAPI` → `HookHandler` / `HookRunner` | TS 使用 `ExtensionEvent` enum + `ExtensionAPI` trait 的事件分发机制，通过 `event_from_agent_event()` 转换层将 `AgentEvent` 映射为扩展事件 | Rust 使用 `HookHandler` trait（所有方法有默认空实现）+ `HookRunner`（按 priority 排序分发），void hooks 并行 fire-and-forget，modifying hooks 顺序执行可 Cancel。参考 ZeroClaw 的 Hook 系统设计 | 消除 `ExtensionEvent` enum 的转换层开销，简化扩展实现（只需实现关心的方法）。事件语义等价：`on_session_start` ≈ `session_start` event，`before_tool_call` ≈ `tool_call` event 等 | 已确认保留 |
| `export_html()` 主题支持 | TS 使用 `settingsManager.getTheme()` + `createToolHtmlRenderer()` + `exportSessionToHtml()` 生成带主题和工具渲染的 HTML | Rust 使用内联 CSS 生成简化版 HTML，不支持主题切换和工具自定义渲染 | 主题系统和工具 HTML 渲染器是 TUI 层功能，不在 pi-coding-agent 当前范围内。基础 HTML 导出功能已实现 | 已确认保留 |

## Bun 子进程扩展运行时（方案 A，`bun-runtime` feature）

> 2026-08-09：V8 手写 shim 方案（`js-runtime`，deno_core + js_shims）已整体删除，
> 只保留 Bun 子进程方案。原 V8 方案的偏差条目（typebox/pi-ai/pi-tui/pi-coding-agent/
> node builtins/AbortController shim 等）随代码一并移除，不再适用。


> 与 V8 手写 shim 路径（`js-runtime`）**互斥**：启用 `bun-runtime` 时扩展
> 跑在真实 Bun 子进程里（真实 node_modules 解析 + 真实 Node API），不再需要
> Node 内置模块 shim。SDK 包（`@earendil-works/pi-*`、typebox）仍由宿主在
> 临时工作区提供 JS shim（`bun/shims/`），但面是有限的、文档化的；Node 内置
> 模块与第三方依赖由 Bun 原生解决——这是与 V8 方案的本质区别。

| 位置（文件:行/函数名） | 原 TS 行为 | Rust 实际行为 | 修改原因 | 确认状态 |
| ---------------------- | ---------- | -------------- | -------- | -------- |
| `bun/mod.rs` — Bun 二进制嵌入 | TS 原版 `pi` 是 `bun build --compile` 产物，Bun 内嵌 | `assets/runtime/bun-{os}-{arch}`（xtask `fetch-bun` 下载）经 `include_bytes!` 嵌入，运行时解压到 `{agent_dir}/runtime/bun` 后 spawn | 用户环境无需安装 Node/Bun；与 TS 原版架构同构（原版就是 Bun 跑扩展） | 已确认保留 |
| `bun/mod.rs` — 扩展工作区 | jiti 从扩展所在 node_modules 解析依赖 | 临时工作区 `{agent_dir}/runtime/ws-{pid}-{nanos}/`：node_modules 放 SDK shim，扩展目录 symlink 为 `ext/`，Bun 按 node_modules 规则解析 | 扩展的 peerDependencies（`@earendil-works/pi-*`）由宿主提供（TS 原版用 virtualModules/aliases 同理）；扩展自身 node_modules（第三方依赖）经 symlink 仍可解析 | 已确认保留 |
| `bun/mod.rs` — SDK shim（`bun/shims/`） | 扩展 import 真实 SDK 包 | 提供 `@earendil-works/pi-ai`（StringEnum/isContextOverflow/isRetryableAssistantError/Type）、`@earendil-works/pi-coding-agent`（defineTool/VERSION/CONFIG_DIR_NAME/getAgentDir）、`@earendil-works/pi-tui`（stripTerminalSequences 真实实现，组件 stub）、`typebox`（Type.* 工厂）的 JS shim | SDK 面有限且文档化；Node 内置模块与第三方依赖由 Bun 原生解决，不再手写 Node shim。真实 SDK dist 的打包（需 TS 仓库构建管线 + 模型数据）是后续项 | 已确认保留 |
| `bun/mod.rs` — stdio JSON-RPC | TS 扩展与宿主同进程（virtualModules + 回调） | 宿主 ↔ Bun 子进程走 stdio JSON-RPC（newline-delimited）：注册/工具执行/事件触发/命令执行/action 方法（sendMessage/exec/getCommands/...）全部 RPC 往返。宿主请求 id 从 `HOST_ID_BASE` 起、Bun 侧从 1 起，两侧永不冲突 | 子进程架构的必然通信方式；协议一次性定义，之后不再补丁 | 已确认保留 |
| `bun/mod.rs` — action 方法 | 扩展调用 `pi.sendMessage`/`pi.exec`/`pi.getCommands`/`pi.setModel` 等 | Bun→宿主请求由 `read_stdout` 分发到 `bind_actions()` 安装的闭包（读类读共享状态快照、写类入 action bus、`exec` 宿主跑 shell、`registerProvider` 接 ModelRegistry），session 创建后绑定（镜像 TS `bindCore()`） | 与 V8 时代 `RuntimeActions` 同构；已实测 `getCommands`/`exec` 返回正确结果 | 已确认保留 |
| `bun/mod.rs` — 工具工厂（createBashTool 等） | 扩展可调用 SDK 工具工厂 | `createBashTool`/`createReadTool`/`createWriteTool`/`createEditTool` 返回 ToolDefinition，其 execute 经 `pi.__runBuiltinTool` RPC 到宿主跑**真实内置工具**（read/bash/edit/write） | 已实测 createBashTool 的 execute 返回真实 bash 输出 | 已确认保留 |
| `bun/mod.rs` — pi-ai 桥接 | 扩展调用 `complete`/`streamSimple`/`getModel`/`registerApiProvider` | 经 `pi.__piAiComplete` 等 RPC 到宿主：宿主从 ModelRegistry 解析模型并跑 `pi_ai::stream::complete`；`getModel` 读共享状态快照的 model_id | 与宿主 provider 层桥接 | 已确认保留 |
| `assets/sdk/` — 真实 SDK bundle | 扩展 import 真实 SDK 包 | `xtask build-sdk` 用 Bun 从 TS 仓库源码打包真实纯函数到 `assets/sdk/`（pi-ai-bundle.js / pi-coding-agent-bundle.js / pi-tui-bundle.js + 真实 typebox 包），运行时经 rust-embed 写入工作区 node_modules。wrapper（`sdk_wrappers/`）叠加 RPC 桥接（complete/getModel/registerApiProvider/工具工厂）与 TUI 组件 stub | 纯函数（StringEnum/isContextOverflow/uuidv7/parseFrontmatter/convertToLlm/serializeConversation/withFileMutationQueue/stripTerminalSequences/visibleWidth/...）来自真实 TS 源码转译，不再手写；桥接函数与 TUI stub 是宿主边界 | 已确认保留 |
