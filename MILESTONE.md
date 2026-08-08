# 对齐里程碑（Alignment Milestone）

> 本文件记录 pi-rs 与 TS 原版（earendil-works/pi）的对齐目标与进度基准。
> 变更日志参考：https://github.com/earendil-works/pi/releases

## 目标版本

| 项 | 值 |
| --- | --- |
| 对齐目标 | earendil-works/pi **v0.80**（0.80.x 系列） |
| 最终补丁 | v0.80.10 |
| 目标 commit | `8dc78834cde4e329284cf505f9e3f99763df5529`（v0.80.10，2026-07-16） |
| 起始版本 | v0.80.0（`f08e968c83d92bce5f5fd2f7f20ef37f8cf04a39`，2026-06-23） |
| 参考 changelog | https://github.com/earendil-works/pi/releases |

## 背景

- 开始对齐时 pi-rs 自身版本：**v1.79.x**（2026-07-30 前后）。
- 当时 `../pi` 处于 v0.80.x 时代：v0.80.10 于 2026-07-16 发布，v0.81.0 尚未发布。
- 当前 pi-rs 版本：v1.80.2（2026-08-01 之后）。

## v0.80.x 变更摘要（来自 releases changelog）

| 版本 | 发布日期 | 关键变更 |
| --- | --- | --- |
| v0.80.0 | 2026-06-23 | pi-ai 旧全局 API（`stream`/`complete`/`getModel`/`registerApiProvider` 等）迁至 `@earendil-works/pi-ai/compat`；新增 Ctrl+J 换行；session 名称换行符规范化；OpenAI Responses 流在缺失终止事件前失败；Codex Responses WebSocket 断线重连；Bedrock 尊重 scoped `AWS_PROFILE`；移除 `/base` 选择性入口 |
| v0.80.1 | 2026-06-23 | Bedrock scoped `AWS_PROFILE` endpoint 修复；Fireworks Anthropic 兼容请求默认值；Together MiniMax M2.7 元数据修复 |
| v0.80.2 | 2026-06-23 | `ApiKeyCredential` discriminator 改为 `type: "api_key"`；`ExecutionEnvExecOptions` 重命名为 `ShellExecOptions`；Anthropic 兼容自定义模型改用显式 compat 元数据；恢复 legacy stream 别名（`streamSimpleOpenAICompletions` 等）与 openai-completions `detectCompat` fallback |
| v0.80.3 | 2026-06-30 | Claude Sonnet 5 支持；`outputPad`/`externalEditor` 设置；RPC `get_entries`/`get_tree`；扩展 `session_info_changed` 事件；Azure Foundry endpoint；默认 OpenAI 模型改为 gpt-5.5；`Usage.reasoning` token 计数 |
| v0.80.4 | 2026-07-09 | tag 存在，无独立 release changelog |
| v0.80.5 | 2026-07-09 | 无 changelog（占位 release） |
| v0.80.6 | 2026-07-10 | 新增 `max` thinking level；input-based 定价 tier（GPT-5.4/5.5/5.6 长上下文计费）；`shellPath` 支持 `~` 展开；Anthropic 空 thinking 文本保留 |
| v0.80.7 | 2026-07-14 | **Breaking**：移除 `compat.sendSessionIdHeader`，改为 `compat.sessionAffinityFormat`；cache-friendly dynamic tool loading；Ctrl+X 复制消息；Fable 5 `xhigh`/`max` thinking；Responses `toolChoice` 支持 |
| v0.80.8 | 2026-07-16 | **Breaking**：`ModelRuntime` 统一模型运行时与 provider 认证（`authStorage`/`modelRegistry` → `modelRuntime`）；live model catalog refresh（`pi update --models`）；xAI device-code OAuth + Grok 4.5 Responses |
| v0.80.9 | 2026-07-16 | Kimi K3 + deferred tool loading；xAI 默认模型改为 Grok 4.5；移除 Grok 3 / Grok 3 Fast / Grok 4.20 等 |
| v0.80.10 | 2026-07-16 | Kimi Coding adaptive thinking 兼容；K3 仅暴露 `max` thinking level；恢复 0.80.9 误删的 xAI 模型 |

## 对齐状态

- 已确认偏差：见各 crate 的 `DEVIATIONS.md`（`crates/pi-ai/DEVIATIONS.md`、`crates/pi-coding-agent/DEVIATIONS.md`）。
- 已知未覆盖项（相对 v0.80，已确认为最终范围）：
  - `openai-responses` API 后端：已移植（openai 官方 provider 走 `/v1/responses`）。
  - `azure-openai-responses` / `openai-codex-responses` 后端：**范围外**（用户拍板：主流仅需 anthropic + openai 格式）。
  - `pi-messages` 后端：已移植（`providers/pi_messages.rs`，Radius gateway 协议；见 DEVIATIONS #7）。
  - v0.80.8 的 `ModelRuntime` / live model catalog refresh：手动刷新已实现（`pi refresh`），自动刷新/可用性检查/credential 同步未做（范围外）。
  - 模型覆盖度：用户确认仅保留 anthropic + openai 主流范围（见 `crates/pi-ai/DEVIATIONS.md` #2，已确认保留）。

> 本文件是里程碑基准，不是偏差日志。发现行为不一致时仍按 CLAUDE.md 阶段四流程处理：先查 `DEVIATIONS.md`，未登记再分类处理。
