# THIRD-PARTY-NOTICES

本文件记录 pi-rs 仓库内 **vendored（整体复制）的第三方代码**，以及对其所做的
本地适配。Vendored 代码保留上游许可证，修改处按下表记录，便于将来从上游
`vendor_rev` 重新同步。当前同步基线（`vendor_repo` / `vendor_rev` /
`vendor_source_rev`）记录在仓库根 `.version`。

`crates/vendor/**` 不参与 `scripts/fmt.sh`（Makefile `fmt` / pre-commit 使用该
脚本），以保持与上游逐字节一致，让每次同步的 diff 只包含下表的真实适配。

## 1. xai-grok-markdown（+ xai-grok-markdown-core）

| 项 | 值 |
| --- | --- |
| 上游 | https://github.com/xai-org/grok-build（Apache-2.0） |
| 上游路径 | `crates/codegen/xai-grok-markdown`、`crates/codegen/xai-grok-markdown-core` |
| 本地路径 | `crates/vendor/xai-grok-markdown`、`crates/vendor/xai-grok-markdown-core` |
| 许可证 | Apache License 2.0（上游 LICENSE 随包保留） |
| 引入原因 | 流式 markdown 渲染管线（pulldown-cmark 解析 + syntect 高亮 + checkpoint 流式冻结 + LaTeX/mermaid 渲染），pi-tui 最小可用版直接复用，避免自研 |
| 同步依据 | `xai-org/grok-build` commit `97f190f644ae1ba07fd6ee185ef54c650e142666`（2026-09-29），其 `SOURCE_REV=559751fdcec02d413e4c57c8832ab275e4f44980`；以 `.version` 的 `vendor_rev` 为准 |

### 本地适配清单（相对上游的改动）

| 位置 | 改动 | 原因 |
| --- | --- | --- |
| `Cargo.toml`（两个 crate） | edition 2024 保留但版本说明；移除 `playground` feature 及可选依赖（crossterm/xai-ratatui-textarea）、移除 benches/bin/fuzz 目录；`build.rs` 保留，build-dependencies 使用 `syntect`/`two-face` 的 onig 后端 | 上游源码使用 let-chains（edition 2024 专属，rustc ≥ 1.85；本仓库 rustc 1.97）；playground 工具不属于 pi-tui 需要；onig 后端与上游 `build.rs` 生成的语法 dump 一致 |
| `Cargo.toml` | dev-dependencies 增加 `pretty_assertions = "1.4"` | 上游通过 workspace 继承，本仓库无该 workspace 依赖 |
| `src/mermaid.rs:parse_note_anchor` | `else if let Some(r) = ... { ... } else { return None; }` 改为 `let r = ...?;` | `clippy::question_mark`（`-D warnings` 全局要求；行为等价） |
| `src/parse.rs` | `sort_by` 改为 `sort_by_key(Reverse)` | `clippy::unnecessary_sort_by`（行为等价） |

### 其余说明

- 上游 `Syntect::new` 内的 `expect("Failed to load theme")` 保留原样：theme
  bytes 经 `include_bytes!` 编译期嵌入，加载失败仅当资源损坏，属防御性断言
  （vendored 代码的例外，AGENTS.md 的 unwrap/expect 禁令适用于本仓库自研代码）。
- 上游 `build.rs` 随包保留：它在构建期把 two-face 语法集 + Swift 语法写入
  `OUT_DIR/syntaxes.bin`，运行时由 `syntax.rs` 读取。
- 上游测试（498 个）随包保留并在 CI 中运行。

## 2. xai-ratatui-textarea / xai-ratatui-inline / xai-tty-utils

| 项 | 值 |
| --- | --- |
| 上游 | https://github.com/xai-org/grok-build（Apache-2.0） |
| 上游路径 | `crates/codegen/xai-ratatui-textarea`、`crates/codegen/xai-ratatui-inline`、`crates/codegen/xai-tty-utils` |
| 本地路径 | `crates/vendor/xai-ratatui-textarea`、`crates/vendor/xai-ratatui-inline`、`crates/vendor/xai-tty-utils` |
| 许可证 | Apache License 2.0 |
| 引入原因 | 多行编辑器 widget（textarea，替换 pi-tui 自研 editor）、内联滚动渲染（inline）、TTY 进程工具（tty-utils，供后续 /doctor 与进程管理） |
| 同步依据 | 同第 1 节（`vendor_rev=97f190f…`）；tty-utils 本次起为完整拷贝（含 `child_wait` / `bundled_git` / `display_char` / `kill_on_drop`） |

### 本地适配清单

| 位置 | 改动 | 原因 |
| --- | --- | --- |
| 三个 `Cargo.toml` | edition 2024、依赖改为 crates.io 具体版本（ratatui-core 0.1 / tui-scrollbar 0.2 / nix 0.30 / windows 0.61 等）；去掉上游 `examples/` 与 `benches/`；tty-utils 的 tokio 增加测试所需 features | pi-rs 工具链 rustc 1.97；workspace 依赖面不同 |
| textarea `src/textarea_tests.rs`（4 处）、`src/wrapping.rs`、`src/editor_tests/planning.rs` | 测试断言改写：`&*lines`→`*lines`、单元素 range 断言改用 `first()`、`vec![1..6]` 加 `#[allow]` | `clippy::useless_borrows_in_formatting` / `indexing_slicing` / `single_range_in_vec_init`（`-D warnings`；行为等价） |
| tty-utils `src/runtime_tests.rs`、`src/lib.rs` | Linux 专用测试的 import / `Mutex` 加 `#[cfg(target_os = "linux")]` | 上游 CI 只在 Linux 跑；macOS 上会报 unused import（行为等价） |


## 3. pi-tui/src/render/wrap.rs（文件级抄）

| 项 | 值 |
| --- | --- |
| 上游 | grok-build `xai-grok-pager-render` 的 `render/wrapping.rs` + `render/line_utils.rs` + `util.rs`（Apache-2.0） |
| 本地 | `crates/pi-tui/src/render/wrap.rs`（777 行） |
| 内容 | `RtOptions`、`word_wrap_line(s)_with_joiners`、`slice_line_spans`（O(rows+spans) 单调游标）、`is_table_line`（表格行不折行）、blockquote 续行前缀、`fit_line_to_width`/`truncate_line` 等 |
| 适配 | 去掉 `tool_paths`/`util` 模块依赖（`byte_offset_at_width`/`truncate_to_width` 内联）；`RtOptions` 补 `#[derive(Clone)]`；剔除搜索高亮映射函数（`byte_range_to_row_cols` 等，scrollback 搜索时再抄） |
| 用途 | Markdown 组件的 wrap 从简化版升级为 grok joiner 版；joiners 缓存在 `Markdown::joiners()`，为复制保真预留 |

## 4. ~~pi-tui/src/scrollback/~~（已移除，2026-08-24）

| 项 | 值 |
| --- | --- |
| 上游 | grok-build `xai-grok-pager` 的 `src/scrollback/`（Apache-2.0，SOURCE_REV `3e620a76a5f374ce644dc7c87f7e990c68348218`） |
| 历史 | 曾文件级抄入 `crates/pi-tui/src/scrollback/`（折叠扫描/投影、`DisplayMode` 三态、"N tool calls" 聚合）；**用户决定删除折叠功能并复刻 TS 原版后，模块整体删除**，本节仅作历史记录 |

## 5. 本仓库内其他第三方代码

| 位置 | 上游 | 说明 |
| --- | --- | --- |
| `third_party/`（如有） | — | 当前无其他 vendored 代码 |
