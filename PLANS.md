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
├── Cargo.toml              # Cargo workspace（Rust crates）
├── package.json            # NPM workspace（TypeScript 项目）
├── crates/
│   ├── pi-agent-core/      # Rust — 核心 Agent 框架
│   ├── pi-ai/              # Rust — AI provider 抽象层
│   └── pi-coding-agent/    # Rust — CLI + SDK + 扩展系统
└── rpc-host/               # TypeScript — Bun 扩展执行引擎
    └── package.json        # NPM workspace member
```

- **Cargo workspace** (`crates/*`) 管理所有 Rust crate
- **NPM workspace** (`rpc-host`) 管理 TS 扩展执行引擎
- 两个 workspace 由根目录的 `Cargo.toml` + `package.json` 统一管理

## pi-coding-agent 复刻进度

### 已实现的核心模块

| Rust 文件 | 对应原版 TS | 状态 | 说明 |
|---|---|---|---|
| `config.rs` | `config.ts` | ✅ | 配置路径、获取 agent 目录等 |
| `core/agent_session.rs` + runtime + services | `core/agent-session.ts` + -runtime + -services | ✅ | Agent 会话管理 |
| `core/auth_guidance.rs` | `core/auth-guidance.ts` | ✅ | API Key 引导 |
| `core/auth_storage.rs` | `core/auth-storage.ts` | ✅ | 认证信息持久化 |
| `core/bash_executor.rs` | `core/bash-executor.ts` | ✅ | Bash 执行器 |
| `core/compaction.rs` | `core/compaction/` (目录) | ✅ | 会话压缩 |
| `core/context_usage.rs` | (原版无直接对应) | ✅ | Token 用量跟踪 |
| `core/defaults.rs` | `core/defaults.ts` | ✅ | 系统文件默认内容 |
| `core/diagnostics.rs` | `core/diagnostics.ts` | ✅ | 诊断信息 |
| `core/env_api_keys.rs` | (原版无直接对应) | ✅ | 环境变量 API Key |
| `core/event_bus.rs` | `core/event-bus.ts` | ✅ | 事件总线 |
| `core/exec.rs` | `core/exec.ts` | ✅ | 命令执行 |
| `core/extensions/types.rs` | `core/extensions/types.ts` | ✅ | 扩展类型定义 |
| `core/extensions/rpc.rs` | `worker.ts` + `loader.ts` | ✅ | Bun 子进程 JSON-RPC 桥接 |
| `rpc-host/` (另建) | - | ✅ | Bun 侧扩展执行代理 |
| `core/footer_data_provider.rs` | `core/footer-data-provider.ts` | ✅ | 底部状态栏数据 |
| `core/http_dispatcher.rs` | `core/http-dispatcher.ts` | ✅ | HTTP 分发器 |
| `core/messages.rs` | `core/messages.ts` | ✅ | 消息格式转换 |
| `core/model_registry.rs` | `core/model-registry.ts` | ✅ | 模型注册表 |
| `core/model_resolver.rs` | `core/model-resolver.ts` | ✅ | 模型解析器 |
| `core/output_guard.rs` | `core/output-guard.ts` | ✅ | 输出保护 |
| `core/prompt_templates.rs` | `core/prompt-templates.ts` | ✅ | 提示模板加载 |
| `core/provider_attribution.rs` | `core/provider-attribution.ts` | ✅ | Provider 归属 |
| `core/provider_display_names.rs` | `core/provider-display-names.ts` | ✅ | Provider 展示名 |
| `core/resolve_config_value.rs` | `core/resolve-config-value.ts` | ✅ | 配置值解析 |
| `core/resource_loader.rs` | `core/resource-loader.ts` | ✅ | 资源加载器 |
| `core/sdk.rs` | `core/sdk.ts` | ✅ | SDK 入口 |
| `core/session_cwd.rs` | `core/session-cwd.ts` | ✅ | 会话工作目录 |
| `core/session_manager.rs` | `core/session-manager.ts` | ✅ | 会话持久化管理 |
| `core/settings_manager.rs` | `core/settings-manager.ts` | ✅ | 设置管理 |
| `core/skills.rs` | `core/skills.ts` | ✅ | Skill 加载 |
| `core/slash_commands.rs` | `core/slash-commands.ts` | ✅ | 斜杠命令定义 |
| `core/source_info.rs` | `core/source-info.ts` | ✅ | 来源信息 |
| `core/system_prompt.rs` | `core/system-prompt.ts` | ✅ | 系统提示构建 |
| `core/telemetry.rs` | `core/telemetry.ts` | ✅ | 遥测收集 |
| `core/timings.rs` | `core/timings.ts` | ✅ | 性能计时 |
| `core/tools/` | `core/tools/` | ✅ | 工具系统完整 |
| `lib.rs` | `index.ts` | ✅ | crate 入口 |

### 尚未实现的模块

| 原版 TS 文件 | 状态 | 说明 |
|---|---|---|
| `core/experimental.rs` | `core/experimental.ts` | ✅ | 实验性功能开关 |
| `core/export-html/` | ❌ | HTML 导出 (低优先级) |
| `core/keybindings.ts` | ❌ | 快捷键管理 (TUI 相关) |
| `core/package_manager.rs` | `core/package-manager.ts` | ✅ | 包管理器（npm 子进程） |
| `core/project_trust.rs` | `core/project-trust.ts` | ✅ | 项目信任解析 |
| `core/trust_manager.rs` | `core/trust-manager.ts` | ✅ | 信任决策持久化 |
| `core/extensions/loader.ts` | 🚧 | 扩展加载器（需 Bun 子进程 RPC） |
| `core/extensions/runner.ts` | 🚧 | 扩展运行时（需 Bun 子进程 RPC） |
| `core/extensions/types.ts` | 🚧 | 扩展类型（部分迁移到 Rust） |
| `core/extensions/wrapper.ts` | 🚧 | 工具包装器（需 RPC 桥接） |
| `cli/args.rs` + `cli/run.rs` | `cli/args.ts` + `cli.ts` + `main.ts` | ✅ | CLI 参数解析 + 执行流程 |
| `main.rs` (binary) | `cli.ts` | ✅ | 二进制入口 |
| `cli/file_processor.rs` | `cli/file-processor.ts` | ✅ | `@file` 语法支持（文本 + 图片） |
| `bun/` | ❌ | Bun 特有入口 |
| `modes/rpc/` | `modes/rpc/rpc-types.ts` + `rpc-mode.ts` + `jsonl.ts` | ✅ | RPC 协议类型 + 命令处理器 + JSONL 读写 |
| `modes/print_mode.rs` | `modes/print-mode.ts` | ✅ | 打印模式（从 cli/run.rs 提取独立） |
| `cli/initial_message.rs` | `cli/initial-message.ts` | ✅ | 初始消息构建 |
| `migrations.rs` | `migrations.ts` | ✅ | 数据迁移（oauth/auth/settings） |
| `utils/paths.rs` | `utils/paths.ts` | ✅ | 路径解析、规范化、tilde 展开 |
| `utils/child_process.rs` | `utils/child-process.ts` | ✅ | 子进程创建和等待 |
| `utils/git.rs` | `utils/git.ts` | ✅ | Git URL 解析 |
| `utils/shell.rs` | `utils/shell.ts` | ✅ | Shell 配置、输出清理、进程跟踪 |
| `utils/frontmatter.rs` | `utils/frontmatter.ts` | ✅ | YAML Frontmatter 解析 |
| `utils/sleep.rs` | `utils/sleep.ts` | ✅ | 可中断 sleep |
| `utils/json.rs` | `utils/json.ts` | ✅ | JSON 注释剥离 |
| `utils/ansi.rs` | `utils/ansi.ts` | ✅ | ANSI 码检测/剥离/截断 |
| `utils/deprecation.rs` | `utils/deprecation.ts` | ✅ | 去重弃用警告 |
| `utils/pi_user_agent.rs` | `utils/pi-user-agent.ts` | ✅ | User-Agent 字符串生成 |
| `utils/version_check.rs` | `utils/version-check.ts` | ✅ | 版本比较和远端版本查询 |
| `utils/html.rs` | `utils/html.ts` | ✅ | HTML 实体编解码 |
| `utils/fs_watch.rs` | `utils/fs-watch.ts` | ✅ | 文件系统轮询监听 |
| `utils/mime.rs` | `utils/mime.ts` | ✅ | 图像 MIME 类型检测 |

### 扩展系统执行层（已实现 ✅）

通过 Bun 子进程 + JSON-RPC 桥接原版 TS 扩展生态。

**架构：**

```
Rust pi-coding-agent
       │
       │ spawn bun run rpc-host/src/index.ts
       │ line-delimited JSON-RPC 2.0 over stdin/stdout
       ▼
Bun sidecar process
       │
       │ 使用 jiti 动态 import() TypeScript 扩展
       │ 注册 tools / commands / lifecycle hooks
       │ 响应 Rust 端的 call_tool 请求
       ▼
TypeScript 扩展文件 (.ts / .js)
  - ~/.pi/extensions/ 或
  - {project}/.pi/extensions/
```

**支持的 RPC 方法：**
- `load` — 加载扩展，返回 tools/commands 元数据
- `call_tool` — 执行扩展的 tool handler
- `reload` — 清缓存重载所有扩展
- `shutdown` — 优雅退出
- `ping` — 健康检查

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
- [x] 实现 package-manager
- [x] 实现 utils/ 工具函数目录 (15 个模块)
- [x] 实现 package-manager / file-processor / initial-message / migrations
- [x] 提取 print-mode 为独立模块
