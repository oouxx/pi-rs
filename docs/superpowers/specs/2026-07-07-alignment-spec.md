# pi-coding-agent 对齐原版规范

**目标：** 将 pi-rs 的 pi-coding-agent crate 对齐原版 @earendil-works/pi-coding-agent（TypeScript），包括功能补齐和行为对齐。

## Phase 1：缺失模块补齐（新增 Rust 模块）

| 原版 TS | Rust 实现 | 路径 |
|---|---|---|
| `cli/list-models.ts` | `cli/list_models.rs` | 列出模型 CLI |
| `package-manager-cli.ts` | `cli/package_manager_cli.rs` | install/remove/list/update 子命令 |
| `utils/changelog.ts` | `utils/changelog.rs` | CHANGELOG.md 解析 + 版本比较 + 链接规范化 |
| `utils/open-browser.ts` | `utils/open_browser.rs` | 跨平台打开 URL（open/xdg-open/rundll32） |
| `utils/tools-manager.ts` | `utils/tools_manager.rs` | fd/rg 自动下载管理 |

## Phase 1b：扩展系统审查（extensions RPC 对齐）

审查现有 RPC 桥接是否覆盖原版 loader/runner/wrapper 功能，检查 `before_provider_headers` 钩子。

## Phase 2：行为对齐（回源 7 个原版 commit）

| 优先级 | 原版 commit | Rust 修改位置 |
|---|---|---|
| 🔴 | bash 超时校验（`85b7c24` + `cbcf4e0`） | `bash_executor.rs` |
| 🔴 | 工具刷新保留运行提示（`fd6659d`） | `session_manager.rs` |
| 🔴 | 刷新会话状态再开始下一轮（`e547bb9`） | `agent_session_runtime.rs` |
| 🔴 | 消息内容规范化（`8c0ccd1`） | `agent_session.rs` + `messages.rs` |
| 🟡 | split-turn 压缩摘要序列化（`f58c115`） | `compaction.rs` |
| 🟡 | 短 session ID 派生（`1dac099`） | `session_manager.rs` |
| 🟢 | 移除冗余 record guards（`035ea9c`） | 对应模块 |
