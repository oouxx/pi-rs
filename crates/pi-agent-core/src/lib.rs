#![allow(
    clippy::module_name_repetitions,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    clippy::missing_const_for_fn,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::too_long_first_doc_paragraph,
    clippy::redundant_closure,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_clone,
    clippy::unused_self,
    clippy::map_unwrap_or,
    clippy::uninlined_format_args,
    clippy::needless_continue,
    clippy::significant_drop_tightening,
)]

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
