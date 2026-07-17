pub mod agent;
pub mod agent_loop;
#[cfg(feature = "extraction")]
pub mod extraction;
pub mod harness;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod pi_ai_types;
pub mod proxy;
pub mod types;

/// Re-export pi-ai so downstream crates can access it without a direct dependency.
/// Use `pi_agent_core::pi_ai::...` instead of adding `pi-ai` to your Cargo.toml.
pub use pi_ai;
