pub mod config;
pub mod core;
pub mod migrations;
pub mod modes;
pub mod pi_ai_types;
pub mod utils;

/// Re-export pi_agent_core so downstream crates (pi-cli, pi-tui) can access
/// agent-core and pi-ai types through `pi_coding_agent::pi_agent_core::...`
/// without listing pi-agent-core or pi-ai in their own Cargo.toml.
pub use pi_agent_core;

/// Message and tool execution lifecycle event types (match TS package-root
/// exports of `MessageStartEvent`/`MessageUpdateEvent`/`MessageEndEvent`/
/// `ToolExecutionStartEvent`/`ToolExecutionUpdateEvent`/`ToolExecutionEndEvent`,
/// #6772). In Rust these are variants of the single `AgentEvent` enum.
pub use pi_agent_core::types::AgentEvent;
