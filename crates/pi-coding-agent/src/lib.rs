#![allow(clippy::type_complexity)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::await_holding_lock)]
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
