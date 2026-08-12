# π — 终端里的 AI 编程助手

π 是一个运行在终端里的 AI 编程助手（coding agent）。它可以直接在你的项目目录里工作：读代码、改文件、跑命令，帮你完成从"解释一段代码"到"实现一个功能"的各种任务。

## 安装

从 GitHub Releases 自动安装（自动探测操作系统/架构）：

```bash
curl -fsSL https://raw.githubusercontent.com/oouxx/pi-rs/main/install.sh | bash
```

或手动安装：

```bash
./install.sh install     # 安装
./install.sh update      # 更新到最新版
./install.sh uninstall   # 卸载
```

## 快速开始

```bash
# 直接问（默认交互模式）
pi-rs "帮我写一个 Rust 的斐波那契函数"

# 一次性提问，结果输出到终端（适合脚本/管道）
pi-rs -p "解释一下这个文件" < file.rs

# 列出可用的模型
pi-rs --list-models
```

首次使用前，配置你的模型和 API Key（环境变量或 `~/.pi-rs/agent/settings.json`），然后 `pi-rs --list-models` 确认模型可用。

## 使用方式

### 交互模式（默认）

在终端里启动一个对话会话，支持多轮对话、工具调用（读写文件、执行命令等）、会话历史保存与恢复。

```bash
pi-rs                          # 启动交互会话
pi-rs --continue               # 继续上一次会话
pi-rs --resume                 # 选择并恢复历史会话
pi-rs --fork <ID>              # 从某个会话分叉出新会话
```

### 一次性提问（Print 模式）

适合快速提问、脚本调用、管道处理：

```bash
pi-rs -p "总结这个仓库的结构"
cat error.log | pi-rs -p "帮我分析这个报错"
pi-rs -p -m claude-sonnet-4-6 "用 Python 写一个快速排序"
```

### 编辑器集成（ACP 模式）

在 Zed 等支持 ACP（Agent Client Protocol）的编辑器里，把 π 配置为 ACP 代理，即可在编辑器内直接对话、查看工具执行过程：

```bash
pi-rs --acp
```

编辑器会通过标准输入输出与 π 通信，你可以在编辑器里选择模型、调整思考强度、查看 bash 终端输出和文件 diff。

### 其他模式

```bash
pi-rs --mode json "..."        # JSON 结构化输出，方便程序解析
pi-rs --mode rpc               # RPC 模式（供外部工具调用）
```

## 常用选项

| 选项 | 说明 |
| ---- | ---- |
| `-m, --model` | 指定模型（如 `claude-sonnet-4-6`） |
| `-P, --provider` | 指定提供商 |
| `-t, --thinking` | 思考强度：`off` / `minimal` / `low` / `medium` / `high` / `xhigh` |
| `--tools` / `--exclude-tools` | 允许 / 排除特定工具（如 `read,bash,edit`） |
| `--extension <PATH>` | 加载扩展 |
| `--no-session` | 不保存会话 |
| `-h, --help` | 查看完整帮助 |

## 会话中的斜杠命令

在交互或 ACP 会话中，输入 `/` 开头的命令可以管理会话：

- `/model` — 切换模型
- `/settings` — 查看/修改设置
- `/resume` — 恢复历史会话
- `/export` — 导出会话（HTML / JSONL）
- `/compact` — 压缩上下文
- `/login` — 配置 API Key
- `/new` — 开启新会话
- `/quit` — 退出

## 数据与配置

- 会话记录保存在 `~/.pi-rs/agent/sessions/`
- 配置文件：`~/.pi-rs/agent/settings.json`（模型、API Key、默认参数）
- 模型列表：`~/.pi-rs/agent/models.json`（可手动添加本地模型端点）

## 致谢

π 是 [earendil-works/pi](https://github.com/earendil-works/pi)（TypeScript 版）的 Rust 移植。
原版由 Mario Zechner 开发，采用 MIT 许可证（Copyright (c) 2025 Mario Zechner）。
本项目的设计、行为与公开接口均以原版为基准，感谢原作者的出色工作。
