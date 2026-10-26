//! pi-extensions — Rust 原生扩展实现。
//!
//! 当前包含：
//! - `subagent` — subagent 工具，委派独立任务给子 pi 进程
//! - `web_search` — web_search / web_fetch 工具（Ollama / DeepSeek 后端）

pub mod subagent;
pub mod web_search;
