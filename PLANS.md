# pi-rs 开发计划

## 总体目标

用 Rust 完整复刻 pi (https://github.com/earendil-works/pi) 的四个核心包：

| Rust crate | 原版 npm package |
|---|---|
| `pi-agent-core` | `@earendil-works/pi-agent-core` |
| `pi-coding-agent` | `@earendil-works/pi-coding-agent` |
| `pi-ai` | `@earendil-works/pi-ai` |
| `pi-tui` | `@earendil-works/pi-tui` |

## 项目结构

```
pi-rs/
├── Cargo.toml              # Cargo workspace（4 个 Rust crates）
├── package.json            # NPM workspace（rpc-host）
├── crates/
│   ├── pi-agent-core/      # Rust — 核心 Agent 框架
│   ├── pi-ai/              # Rust — AI provider 抽象层
│   ├── pi-coding-agent/    # Rust — CLI + SDK + 扩展系统
│   └── pi-tui/             # Rust — TUI 组件库 (Elm 架构)
└── rpc-host/               # TypeScript — Bun 扩展执行引擎
```

## 原版 pi-coding-agent 最近变更 (2026-07-06 ~ 07-07)

| 变更 | 影响 |
|---|---|
| `before_provider_headers` 扩展钩子 | 扩展系统新增 hook |
| `InlineExtension` 类型 | 内联扩展工厂 |
| 清理 label 时间戳缓存 | session-manager |
| 规范化 null 消息内容 | agent-session |
| Project-local pi config 改进 | settings-manager / package-manager |

## pi-coding-agent 复刻进度

### 核心模块 (core/)

| Rust 文件 | 对应原版 TS | 状态 | 说明 |
|---|---|---|---|
| `config.rs` | `config.ts` | ✅ | 配置路径、.pi-rs 目录 |
| `agent_session.rs` + runtime + services | `agent-session.ts` + -runtime + -services | ✅ | Agent 会话管理 |
| `auth_guidance.rs` | `auth-guidance.ts` | ✅ | API Key 引导 |
| `auth_storage.rs` | `auth-storage.ts` | ✅ | 认证持久化 (auth.json) |
| `bash_executor.rs` | `bash-executor.ts` | ✅ | Bash 执行 |
| `compaction.rs` | `compaction/` | ✅ | 会话压缩 |
| `context_usage.rs` | (原版无) | ✅ | Token 用量跟踪 |
| `defaults.rs` | `defaults.ts` | ✅ | 系统文件默认 |
| `diagnostics.rs` | `diagnostics.ts` | ✅ | 诊断信息 |
| `env_api_keys.rs` | (原版无) | ✅ | 环境变量 API Key |
| `event_bus.rs` | `event-bus.ts` | ✅ | 事件总线 |
| `exec.rs` | `exec.ts` | ✅ | 命令执行 |
| `extensions/types.rs` | `extensions/types.ts` | ✅ | 扩展类型定义 |
| `extensions/rpc.rs` | — | ✅ | Bun RPC 桥接 (替代 JS loader) |
| `experimental.rs` | `experimental.ts` | ✅ | 特性开关 |
| `footer_data_provider.rs` | `footer-data-provider.ts` | ✅ | 底部状态栏数据 |
| `http_dispatcher.rs` | `http-dispatcher.ts` | ✅ | HTTP 分发 |
| `messages.rs` | `messages.ts` | ✅ | 消息转换 |
| `model_registry.rs` | `model-registry.ts` | ✅ | 模型注册 |
| `model_resolver.rs` | `model-resolver.ts` | ✅ | 模型解析 |
| `output_guard.rs` | `output-guard.ts` | ✅ | 输出保护 |
| `package_manager.rs` | `package-manager.ts` | ✅ | npm 包管理 |
| `prompt_templates.rs` | `prompt-templates.ts` | ✅ | 提示模板 |
| `project_trust.rs` | `project-trust.ts` | ✅ | 项目信任 |
| `provider_attribution.rs` | `provider-attribution.ts` | ✅ | Provider 归属 |
| `provider_display_names.rs` | `provider-display-names.ts` | ✅ | Provider 展示名 |
| `resolve_config_value.rs` | `resolve-config-value.ts` | ✅ | 配置值解析 |
| `resource_loader.rs` | `resource-loader.ts` | ✅ | 资源加载 |
| `sdk.rs` | `sdk.ts` | ✅ | SDK 入口 |
| `session_cwd.rs` | `session-cwd.ts` | ✅ | 会话目录 |
| `session_manager.rs` | `session-manager.ts` | ✅ | 会话管理 |
| `settings_manager.rs` | `settings-manager.ts` | ✅ | 设置管理 |
| `skills.rs` | `skills.ts` | ✅ | Skill 加载 |
| `slash_commands.rs` | `slash-commands.ts` | ✅ | 斜杠命令 |
| `source_info.rs` | `source-info.ts` | ✅ | 来源信息 |
| `system_prompt.rs` | `system-prompt.ts` | ✅ | 系统提示 |
| `telemetry.rs` | `telemetry.ts` | ✅ | 遥测 |
| `timings.rs` | `timings.ts` | ✅ | 计时 |
| `tools/` | `tools/` | ✅ | 工具系统完整 |
| `trust_manager.rs` | `trust-manager.ts` | ✅ | 信任管理 |

### CLI 入口

| Rust 文件 | 对应原版 TS | 状态 | 说明 |
|---|---|---|---|
| `main.rs` (binary) | `cli.ts` + `main.ts` | ✅ | 二进制入口 |
| `cli/args.rs` | `cli/args.ts` | ✅ | 参数解析 (20+ flags) |
| `cli/run.rs` | `main.ts` → print/json/rpc | ✅ | 执行流程 + 信任/SDK |
| `cli/file_processor.rs` | `cli/file-processor.ts` | ✅ | @file 语法 |
| `cli/initial_message.rs` | `cli/initial-message.ts` | ✅ | 初始消息构建 |
| `migrations.rs` | `migrations.ts` | ✅ | 数据迁移 |

### 模式 (modes/)

| Rust 文件 | 对应原版 TS | 状态 | 说明 |
|---|---|---|---|
| `modes/print_mode.rs` | `modes/print-mode.ts` | ✅ | 一次性文本/JSON 输出 |
| `modes/rpc/` | `modes/rpc/` | ✅ | JSON-RPC 协议 (15+ 命令) |
| `modes/interactive.rs` | `modes/interactive/` | ✅ | 交互 TUI 模式 |
| `modes/agent_bridge.rs` | — | ✅ | AgentSession → TUI 事件桥 |

### 工具函数 (utils/)

| Rust 文件 | 状态 | 说明 |
|---|---|---|
| `paths.rs` | ✅ | 路径解析 |
| `child_process.rs` | ✅ | 子进程 |
| `git.rs` | ✅ | Git URL 解析 |
| `shell.rs` | ✅ | Shell 配置 |
| `frontmatter.rs` | ✅ | Frontmatter 解析 |
| `sleep.rs` | ✅ | Sleep |
| `json.rs` | ✅ | JSON 注释剥离 |
| `ansi.rs` | ✅ | ANSI 码处理 |
| `deprecation.rs` | ✅ | 弃用警告 |
| `pi_user_agent.rs` | ✅ | User-Agent |
| `version_check.rs` | ✅ | 版本检查 |
| `html.rs` | ✅ | HTML 实体解码 |
| `fs_watch.rs` | ✅ | 文件监听 |
| `mime.rs` | ✅ | MIME 检测 |

### TUI 组件库 (pi-tui)

| 模块 | 状态 | 说明 |
|---|---|---|
| `terminal.rs` | ✅ | 终端初始化和事件流 |
| `app.rs` | ✅ | Elm 架构 (Model/Msg/update/view) |
| `keymap.rs` | ✅ | 可配置键位映射 |
| `components/input.rs` | ✅ | 单行输入 (支持 CJK) |
| `components/editor.rs` | ✅ | 多行编辑器 |
| `components/markdown.rs` | ✅ | Markdown 渲染 (ratatui-markdown) |
| `components/select_list.rs` | ✅ | 可选列表 |
| `components/diff.rs` | ✅ | Diff 显示 (similar) |
| `components/completer.rs` | ✅ | / 命令 + @ 文件补全 |
| `components/text.rs` | ✅ | 静态文本 |

### 扩展执行引擎 (rpc-host)

| 功能 | 状态 | 说明 |
|---|---|---|
| jiti 动态加载 TS 扩展 | ✅ | 与 pi 原版兼容 |
| AgentEvent 转换 | ✅ | TextDelta/ToolStart/End/Output |
| ExtensionAPI 桥接 | ✅ | registerTool/Command/Shortcut/Flag |
| ctx.exec() | ✅ | 子进程命令执行 |
| 自动发现 | ✅ | ~/.pi-rs + {cwd}/.pi-rs |
| package.json pi.extensions | ✅ | 清单扫描 |

### 原版已有但 Rust 版未实现

| 原版 TS | 说明 | 优先级 |
|---|---|---|
| `core/extensions/` hooks | `before_provider_headers` 等 | 🟡 扩展钩子系统 |
| `core/keybindings.ts` | 快捷键 (TUI 相关) | 🟢 已在 pi-tui 实现 keymap |
| `core/export-html/` | HTML 导出 | 🔴 低 |
| `cli/config-selector.ts` | TUI 配置选择 | 🔴 等 GUI |
| `cli/session-picker.ts` | TUI 会话选择 | 🔴 等 GUI |
| `cli/startup-ui.ts` | 启动 UI | 🔴 等 GUI |
| `modes/interactive/` (完整版) | 交互模式原版细节 | ✅ 已实现 |
| `utils/` 剩余 (clipboard, image 等) | 剪贴板/图片处理 | 🔴 依赖 TUI |

### 原版最新变更追踪

| 原版 commit | 功能 | 需复刻 |
|---|---|---|
| `244f1dea` | `before_provider_headers` 扩展钩子 | 🟡 扩展钩子桥接 |
| `b3dff19a` | `InlineExtension` 内联扩展工厂 | 🟡 RPC 协议扩展 |
| `8c0ccd14` | 规范化 null 消息内容 | ✅ pi-ai 侧已处理 |
| `6efc09b7` | 清理 label 时间戳缓存 | ✅ session_manager 已完成 |

## 实施状态

- [x] 从 git 历史恢复 pi-coding-agent crate
- [x] 创建复刻进度文档
- [x] 创建 Bun RPC sidecar (rpc-host)
- [x] 重构 Rust 端 extensions 模块集成 RPC
- [x] 编写测试验证扩展执行链路
- [x] 将扩展工具注入 AgentSession 的工具列表
- [x] 复刻 experimental / trust-manager / project-trust 模块
- [x] 实现 CLI 入口 (main.rs + cli/args.rs + cli/run.rs)
- [x] 实现 RPC 模式 (modes/rpc/)
- [x] 实现 package-manager / file-processor / initial-message / migrations
- [x] 提取 print-mode 为独立模块
- [x] 实现 utils/ 工具函数目录 (15 个模块)
- [x] 对接 pi-tui 交互模式
- [x] 精简重构 pi-tui 为 Elm 架构
- [x] 对齐 plans.md TUI 设计
- [x] 恢复 pi-tui crate
