# grok-build TUI 对接 pi-coding-agent 可行性分析

> 分析日期：2026-08-08
> 分析对象：https://github.com/xai-org/grok-build（main 分支，tarball 快照）
> 对接目标：本仓库 `pi-coding-agent`（Rust 移植版）
> 分析环境：worktree `pi-rs-tui-analysis`（branch `analysis/grok-tui-feasibility`）

---

## 1. 结论摘要

**"直接把 grok-build 的 TUI 抄过来"这个前提不成立**：grok-build 的 TUI
（`xai-grok-pager`）不是一个自包含的 TUI 库，而是一个**完整应用**，它和
grok-build 自己的 agent 栈（shell/tools/workspace/config 等，合计约 100 万行）
深度耦合。抄 TUI 等于抄大半个 grok-build。

| 方案 | 内容 | 工作量（单人全职） | 风险 | 结果形态 |
| --- | --- | --- | --- | --- |
| A. 整包抄 pager + pi 说 ACP | 抄 ~100 万行依赖 + 给 pi-coding-agent 实现 ACP 服务端 + 补 xAI 私有协议扩展 | 3~6 个月 | 高 | grok-build 的 fork，不是"我们的 pi" |
| B. 只抄渲染层，重写 app 层 | 抄 render/textarea/inline/tty 约 5.7 万行，重写 agent_view/views/scrollback/acp_handler 约 25 万行 | 2~4 个月 | 中高 | 用 grok 渲染原语写的新 TUI |
| C. pager 当独立二进制，走 pi 的 RPC 协议 | 保留 pager 的 UI 层，重写 agent 对接层（acp_handler + agent.rs + spawn.rs，约 3~5 万行）为 pi RPC 客户端 | 1.5~3 个月 | 中 | 最接近"抄过来"的可行路径 |
| D. 恢复原版 pi-tui（基线） | 修复 ratatui 版本冲突，把已废弃的 pi-tui（~700 行）+ interactive 模式（311 行）补完 | 2~4 周（基础可用）/ 1~2 个月（对齐原版） | 低 | 原版 pi 的 TUI，符合项目移植路线 |

**可行性结论**：技术上可行，但"直接抄"不可行。若目标是"给 pi-coding-agent
一个 grok 级别的 TUI"，方案 C 是唯一现实的"抄"法，且是 1.5~3 个月的项目；
若目标是"让 pi 有 TUI"，方案 D 的工作量是 C 的 1/10 以下，且与项目既有的
"TS → Rust 行为对齐"路线一致。

---

## 2. grok-build 的 TUI 到底是什么

### 2.1 规模（实测，来自 tarball 快照）

| crate | 职责 | 文件数 | LOC |
| --- | --- | --- | --- |
| `xai-grok-pager` | 主 TUI 应用 | 497 | **456,731** |
| `xai-grok-pager-render` | 渲染原语层（terminal/theme/render/appearance/syntax） | — | 39,171 |
| `xai-grok-pager-minimal` | 极简模式变体 | — | 6,744 |
| `xai-ratatui-inline` | 内联滚动渲染 | — | 2,979 |
| `xai-ratatui-textarea` | 多行输入框 | — | 12,722 |
| `xai-tty-utils` | TTY 工具 | — | 2,086 |
| `xai-acp-lib` | ACP 通道封装 | — | 2,277 |
| **TUI 栈合计** | | | **~523,000** |

其中 pager 内部模块分布（LOC）：

- `app/`（事件循环、agent_view、modals、dispatch、effects）：**190,660**
  - 其中 `app/agent_view/`（会话视图模型）：40,151
  - 其中 `app/acp_handler/`（agent 协议处理）：22,564
- `views/`（50+ 个视图：dashboard、permission、plan_approval、queue、todo、
  tasks、subagent_catalog、settings_modal、welcome 等）：**134,750**
- `scrollback/`（滚动缓冲、块、选区、搜索）：**52,913**
- `input/`：5,095；`settings/`：3,372

对比参考：原版 TS `packages/tui` 全库 **16,202** LOC，TS interactive 模式
**17,362** LOC。grok 的 TUI 栈是原版 pi TUI 的 **28 倍**。

### 2.2 架构：不是"TUI 库"，是"完整应用 + 协议客户端"

`xai-grok-pager` 是一个**独立应用**，不是可嵌入的 UI 库：

- 自带完整应用生命周期：`app/event_loop.rs`（tokio::select! 主循环）、
  `app/app_view.rs`、`app/cli.rs`、`app/session_startup.rs`、`app/leader_cluster/` 等
- 自带 settings 系统（`settings/`，3,372 行）、配置热重载、主题系统
- 自带 voice、mermaid 渲染、inline media、plugin marketplace、worktree、
  share/export、memory、telemetry、crash handler 等横切能力
- 自带 PTY e2e 测试基建（`xai-grok-pager-pty-harness`、`xai-grok-test-support`）

### 2.3 与 agent 的耦合方式：ACP + xAI 私有扩展

pager 通过 **ACP（Agent Client Protocol，crates.io 上的
`agent-client-protocol` 0.10.4）** 与 agent 通信，但：

1. **当前只支持 in-process 模式**：`acp/spawn.rs` 注释明确写着
   "Simplified to only support GrokShell (in-process) mode. Subprocess and
   remote modes can be added later if needed."——它直接实例化
   `xai_grok_shell::agent::MvpAgent`（360,170 行的 agent）在**同一进程**里跑，
   通过内存 channel 传 ACP 消息。**没有现成的"外部 agent 走 stdio"通道**。
2. **pager 代码直接引用 grok-shell 类型**，不只是通过 ACP 协议：
   - `xai_grok_shell::extensions::notification::SessionNotification`（xAI 私有会话通知）
   - `xai_grok_shell::tools::todo::todo_item_from_plan_entry`（todo 面板）
   - `xai_grok_workspace::permission::bash_command_splitting::BashCommandHighlights`
   - `xai_grok_shell::sampling::types::ReasoningEffort` 等
   这些是 ACP 标准协议之外的 xAI 私有扩展（`x.ai/session_notification`、
   `x.ai/session/update`），pi-coding-agent 不会产生这些事件。
3. pager 用到的 ACP 类型面：86 个（Initialize/Prompt/NewSession/LoadSession/
   SetSessionModel/SetSessionMode/RequestPermission/Authenticate/ExtRequest/
   ExtNotification/ToolCallUpdate/SessionUpdate/SessionNotification/
   AvailableCommandsUpdate/CurrentModeUpdate/Cancel 等）。

### 2.4 依赖面

`xai-grok-pager` 有 **143 个直接依赖**，其中约 25 个是 grok-build workspace
内部 crate（41 处引用含 dev/feature）：

| 依赖 crate | LOC |
| --- | --- |
| `xai-grok-shell`（agent 本体） | 360,170 |
| `xai-grok-tools` | 132,792 |
| `xai-grok-workspace` | 96,561 |
| `xai-grok-agent` | 22,599 |
| `xai-grok-markdown` | 20,609 |
| `xai-grok-telemetry` | 15,824 |
| `xai-grok-config` | 11,156 |
| `xai-grok-mcp` | 10,837 |
| `xai-grok-memory` | 9,918 |
| `xai-grok-compaction` | 7,609 |
| `xai-grok-sandbox` | 5,441 |
| `xai-grok-voice` | 3,747 |
| 其余（mermaid/update/file-utils/fast-worktree/plugin-marketplace/hooks-plugins-types/crash-handler/prompt-queue/announcements/token-estimation/version 等） | 数万 |

grok-build 整个 workspace 约 **1,536,461** LOC。构建要求：Rust **1.94.0**
（pi-rs 是 1.80.0）、`[patch.crates-io]` 指向 async-openai 私有 fork、protoc 等。

---

## 3. pi-coding-agent 现状

### 3.1 现有接口

- **print 模式**（`modes/print_mode.rs`，324 行）：纯文本输出，可用。
- **RPC 模式**（`modes/rpc/`，1,307 行）：JSONL over stdio，命令面包括
  prompt/steer/follow_up/abort/abort_bash/bash/new_session/get_state/set_model/
  cycle_model/get_available_models/set_thinking_level/cycle_thinking_level/
  set_steering_mode/set_follow_up_mode/get_commands 等，事件面包括
  text_delta/message_end/tool_start/tool_end/tool_output 等。**可用**。
- **interactive 模式**（`modes/interactive.rs`，311 行）：**死代码**。
  被 `#[cfg(feature = "interactive")]` 门控，而该 feature 是空的
  （`Cargo.toml` 注释：pi-tui 依赖因 ratatui 0.29 硬锁 unicode-width 0.2.0
  冲突被移除）。`pi -i` 会直接报"requires building with the interactive feature"。
- **AgentSession**（`core/agent_session.rs`）：公开 API 很全——prompt、
  add_user_text、set_model、cycle_model、set_thinking_level、new_session、
  session 管理、tools、extensions、context_usage、compaction 等。这是对接
  任何 TUI 的天然接口。
- **agent_bridge**（`modes/agent_bridge.rs`，67 行）：把 AgentSession 事件
  转成 TUI 事件（TextDelta/MessageEnd/ToolStart/ToolEnd/ToolOutput），
  已存在但只被 interactive 模式引用。

### 3.2 原版 pi-tui 的移植状态

- 原版 TS：`packages/tui` 16,202 行 + interactive 模式 17,362 行。
- Rust 移植：`crates/pi-tui` 在历史 commit `f248c62`（2026-07-07）精简为
  Elm 架构后约 **700 行**（app.rs/terminal.rs + 5 个组件），随后因 ratatui
  版本冲突被移出 workspace（commit `04adde6`）。
- 也就是说：**pi-rs 目前没有任何可运行的 TUI**，interactive 模式只移植了
  原版的一小部分（311 行 vs 17,362 行）。

---

## 4. 对接方案与工作量

### 方案 A：整包抄 pager，让 pi-coding-agent 说 ACP

步骤：
1. 把 pager + 其 ~25 个 grok workspace 依赖（约 80~100 万行）搬进 pi-rs
   workspace（或 fork grok-build）。
2. 给 pi-coding-agent 实现 ACP 服务端（或写一个薄 wrapper 二进制），
   覆盖 pager 用到的 86 个 ACP 类型 + xAI 私有扩展
   （session_notification/session_update/plan/todo 等）。
3. 改 `acp/spawn.rs` 支持 subprocess 模式（当前只有 in-process）。
4. 处理 grok 专属的 settings/config/telemetry/voice/plugin-marketplace 与
   pi 的对应物冲突。

工作量：**3~6 个月**（单人全职）。风险：高——结果是一个 grok-build fork，
pi-coding-agent 只是其中一个被 ACP 化的后端；且 xAI 私有协议扩展没有文档，
只能靠读 grok-shell 源码逆向。

### 方案 B：只抄渲染层（render/textarea/inline/tty），重写 app 层

步骤：
1. 搬 `xai-grok-pager-render`（39,171）+ `xai-ratatui-textarea`（12,722）+
   `xai-ratatui-inline`（2,979）+ `xai-tty-utils`（2,086），约 5.7 万行，
   这些是相对干净的"表现层原语"。
2. 重写 app 层：agent_view（40,151）+ views（134,750）+ scrollback（52,913）
   + acp_handler（22,564）——这些直接引用 ACP/grok-shell 类型，需要改成
   直接调用 AgentSession + agent_bridge 事件。
3. 决定哪些 grok 视图（plan/todo/subagent/voice/mermaid/plugin）砍掉或重写。

工作量：**2~4 个月**。风险：中高——"重写 app 层"实际是写一个新 TUI，
只是借用了 grok 的渲染原语；views/scrollback 里对 grok-shell 类型的引用
比预想多。

### 方案 C：pager 当独立二进制，走 pi 的 RPC 协议（最接近"抄"的可行路径）

步骤：
1. 把 pager 及其依赖作为独立 workspace（fork grok-build 或 vendor），
   保持 UI 层（views/scrollback/input/render）不动。
2. 重写 agent 对接层：`acp_handler`（22,564）+ `app/agent.rs` + `acp/spawn.rs`
   + `app/agent_view` 的数据来源，从 ACP 换成 pi 的 RPC（JSONL over stdio）
   或直接 in-process 调 AgentSession。
3. 砍掉/降级 xAI 专属功能：plan mode、todo pane、subagent catalog、
   voice、mermaid、plugin marketplace、leader cluster。
4. pi 侧补 RPC 命令：permission 请求/响应、auth 列表、tool call 状态、
   session list/rename/delete、available commands 等（现有 RPC 只有
   prompt/steer/abort/model/thinking 等核心命令）。

工作量：**1.5~3 个月**。风险：中。这是"抄过来"最现实的解释：UI 是 grok
的，agent 是 pi 的，中间用协议/适配层缝合。

### 方案 D（基线）：恢复原版 pi-tui

步骤：
1. 解决 ratatui 版本冲突（当前 Cargo.lock 里 unicode-width 已是 0.2.2，
   冲突可能已不存在，需要实测）。
2. 把 `crates/pi-tui`（历史 commit 里有 ~700 行）恢复进 workspace。
3. 把 `modes/interactive.rs`（311 行）+ `agent_bridge.rs`（67 行）接上，
   按原版 TS（17,362 行）补齐组件：first-time-setup、model-search、
   thinking-selector、theme-selector、extension-selector、external-editor、
   user-message 等。

工作量：**2~4 周**（基础可用）/ **1~2 个月**（对齐原版功能面）。风险：低。

---

## 5. 可行性评估

| 维度 | 评估 |
| --- | --- |
| 技术可行性 | **可行**。ACP 是公开协议（crates.io 有现成 crate），pi 的 AgentSession/RPC 能力足够支撑一个 TUI 的会话/模型/工具/权限语义。 |
| "直接抄"可行性 | **不可行**。pager 不是自包含库，抄它 = 抄 ~100 万行 grok 依赖 + 逆向 xAI 私有协议扩展。 |
| 版本兼容 | grok 需要 Rust 1.94 + async-openai 私有 fork + protoc；pi-rs 是 1.80。直接 vendor 会拉高整个 workspace 的工具链要求。 |
| 维护成本 | 方案 A/B 的结果是"grok 的代码 + pi 的 agent"，两套生态的升级/对齐都要做，长期维护成本高。 |
| 与项目路线的关系 | CLAUDE.md 明确 TUI 本轮不复刻（DEVIATIONS.md 已登记）。grok TUI 是"另一个项目的完整应用"，与"TS → Rust 行为对齐"的移植路线无关。 |

---

## 6. 建议

1. **如果目标是"让 pi 有 TUI"**：走方案 D。工作量小一个数量级，且
   pi-tui 的 Rust 骨架（Elm 架构）在 git 历史里还在，interactive 模式代码
   也在，只是被 feature 门控。先实测 ratatui 冲突是否已消失。
2. **如果目标是"grok 级别的 TUI 体验"**：走方案 C，但要把它当独立项目
   （fork grok-build + 适配层），不要塞进 pi-rs workspace。先做 PoC：
   让 pager 以 subprocess 方式连上 pi 的 RPC 模式，验证 prompt/stream/
   abort/model 切换这条主链路，再决定是否投入。
3. **不建议**方案 A/B：前者是 fork 整个 grok-build，后者等于重写一个新
   TUI，两者都不比"从零写一个基于 AgentSession 的 TUI"省事。
4. 无论哪个方案，先确认需求：是"要一个能用的 TUI"还是"要 grok 的
   具体某个交互（如 plan mode / todo pane / 多会话 dashboard）"——后者
   可以只挑对应视图移植，工作量再降一个量级。

---

## 附：数据来源

- grok-build tarball：`/Users/wxx/Desktop/github/grok-build-main`
  （`https://codeload.github.com/xai-org/grok-build/tar.gz/refs/heads/main`）
- pi-rs 分析 worktree：`/Users/wxx/Desktop/github/pi-rs-tui-analysis`
- 原版 TS：`/Users/wxx/Desktop/github/pi/packages/tui`、
  `packages/coding-agent/src/modes/interactive/`
- LOC 统计：`find <crate>/src -name "*.rs" -exec wc -l {} +`
