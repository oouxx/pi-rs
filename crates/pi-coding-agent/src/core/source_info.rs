//! Re-export of `SourceInfo` from `pi-extension-api`.
//!
//! `SourceInfo` now lives in the lower `pi-extension-api` crate so that
//! `RegisteredCommand`/`RegisteredTool` (also there) can carry it without a
//! reverse dependency. This module re-exports the canonical definitions so
//! existing `use crate::core::source_info::*` call sites keep working.

pub use pi_extension_api::source_info::{
    create_builtin_source_info, create_source_info, create_synthetic_source_info, SourceInfo,
    SourceOrigin, SourceScope,
};
