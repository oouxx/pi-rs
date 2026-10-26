//! pi-extensions — Rust 原生扩展实现。
//!
//! 当前包含：
//! - `subagent` — subagent 工具：agent 定义发现 + single/parallel/chain 三种
//!   委派模式（子 agent 跑独立 pi 进程）
//! - `web_search` — web_search / web_fetch 工具（Ollama / DeepSeek 后端）

pub mod subagent;
pub mod web_search;
