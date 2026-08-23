# THIRD-PARTY-NOTICES

本文件记录 pi-rs 仓库内 **vendored（整体复制）的第三方代码**，以及对其所做的
本地适配。Vendored 代码保留上游许可证，修改处按下表记录，便于将来从上游
`SOURCE_REV` 重新同步。

## 1. xai-grok-markdown（+ xai-grok-markdown-core）

| 项 | 值 |
| --- | --- |
| 上游 | https://github.com/xai-org/grok-build（Apache-2.0） |
| 上游路径 | `crates/codegen/xai-grok-markdown`、`crates/codegen/xai-grok-markdown-core` |
| 本地路径 | `crates/vendor/xai-grok-markdown`、`crates/vendor/xai-grok-markdown-core` |
| 许可证 | Apache License 2.0（上游 LICENSE 随包保留） |
| 引入原因 | 流式 markdown 渲染管线（pulldown-cmark 解析 + syntect 高亮 + checkpoint 流式冻结 + LaTeX/mermaid 渲染），pi-tui 最小可用版直接复用，避免自研 |
| 同步依据 | grok-build tarball `SOURCE_REV`（见上游仓库根 `SOURCE_REV`） |

### 本地适配清单（相对上游的改动）

| 位置 | 改动 | 原因 |
| --- | --- | --- |
| `Cargo.toml`（两个 crate） | edition 2024 保留但版本说明、移除 `playground` feature 及可选依赖（crossterm/xai-ratatui-textarea）、移除 benches/bin/fuzz 目录 | 上游源码使用 let-chains（edition 2024 专属，rustc ≥ 1.85；本仓库 rustc 1.97）；playground 工具不属于 pi-tui 需要 |
| `Cargo.toml` | dev-dependencies 增加 `pretty_assertions = "1.4"` | 上游通过 workspace 继承，本仓库无该 workspace 依赖 |
| `src/mermaid.rs:3185` | `else if let Some(r) = ... { ... } else { return None; }` 改为 `let r = ...?;` | `clippy::question_mark`（`-D warnings` 全局要求；行为等价） |
| `src/parse.rs:1682` | `sort_by` 改为 `sort_by_key(Reverse)` | `clippy::unnecessary_sort_by`（行为等价） |

### 其余说明

- 上游 `Syntect::new` 内的 `expect("Failed to load theme")` 保留原样：theme
  bytes 经 `include_bytes!` 编译期嵌入，加载失败仅当资源损坏，属防御性断言
  （vendored 代码的例外，AGENTS.md 的 unwrap/expect 禁令适用于本仓库自研代码）。
- 上游测试（约 476 个）随包保留并在 CI 中运行。

## 2. 本仓库内其他第三方代码

| 位置 | 上游 | 说明 |
| --- | --- | --- |
| `third_party/`（如有） | — | 当前无其他 vendored 代码 |
