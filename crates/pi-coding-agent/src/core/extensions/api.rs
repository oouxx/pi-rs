//! Re-export from pi-extension-api crate.
//!
//! The ExtensionAPI trait and all extension types live in `pi-extension-api`
//! so both `pi-coding-agent` and `pi-extensions` can depend on it without
//! creating a circular dependency.

pub use pi_extension_api::*;
