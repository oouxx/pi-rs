#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::large_futures,
    clippy::type_complexity,
    clippy::derivable_impls,
    clippy::collapsible_match,
    clippy::collapsible_if,
    clippy::await_holding_lock,
    clippy::unwrap_used,
    clippy::expect_used,
)]

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
