//! `SourceInfo` — provenance of an extension/skill/prompt resource.
//!
//! Mirrors TS `core/source-info.ts`. Lives in `pi-extension-api` (the lower
//! crate) so that `RegisteredCommand`/`RegisteredTool` (also here) can carry
//! `source_info` without a reverse dependency on `pi-coding-agent`.
//! `pi-coding-agent` re-exports these from its own `core::source_info`.

use serde::{Deserialize, Serialize};

/// Scope of a resource. Mirrors TS `SourceScope`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
    User,
    Project,
    Temporary,
}

/// Origin of a package file. Mirrors TS `SourceOrigin`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceOrigin {
    Package,
    /// Serializes as `"top-level"` (with hyphen) to match the TS wire format
    /// (`SourceOrigin = "package" | "top-level"`). The container-level
    /// `rename_all = "lowercase"` would produce `"toplevel"` (no hyphen),
    /// a wire-format mismatch; this per-variant rename overrides it.
    #[serde(rename = "top-level")]
    TopLevel,
}

/// Provenance of a resource. Mirrors TS `SourceInfo`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceInfo {
    pub path: String,
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
}

/// Build `SourceInfo` from resolved package metadata. Mirrors TS
/// `createSourceInfo(path, metadata)`.
#[must_use]
pub fn create_source_info(
    path: String,
    metadata_source: String,
    metadata_scope: SourceScope,
    metadata_origin: SourceOrigin,
    metadata_base_dir: Option<String>,
) -> SourceInfo {
    SourceInfo {
        path,
        source: metadata_source,
        scope: metadata_scope,
        origin: metadata_origin,
        base_dir: metadata_base_dir,
    }
}

/// Build a synthetic `SourceInfo` with defaults. Mirrors TS
/// `createSyntheticSourceInfo(path, options)`.
#[must_use]
pub fn create_synthetic_source_info(
    path: String,
    source: String,
    scope: Option<SourceScope>,
    origin: Option<SourceOrigin>,
    base_dir: Option<String>,
) -> SourceInfo {
    SourceInfo {
        path,
        source,
        scope: scope.unwrap_or(SourceScope::Temporary),
        origin: origin.unwrap_or(SourceOrigin::TopLevel),
        base_dir,
    }
}

/// Build a `SourceInfo` for a built-in (compile-time) extension.
///
/// Rust has no runtime loader (see EXTENSION_LOADING_FEASIBILITY.md), so
/// built-in extensions registered via `ExtensionRegistry::register` use this
/// synthetic provenance: source `builtin`, scope `temporary`, origin
/// `top-level`, path `<builtin:name>`.
#[must_use]
pub fn create_builtin_source_info(name: &str) -> SourceInfo {
    create_synthetic_source_info(
        format!("<builtin:{name}>"),
        "builtin".to_string(),
        Some(SourceScope::Temporary),
        Some(SourceOrigin::TopLevel),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_source_info() {
        let info = create_source_info(
            "/tmp/test".into(),
            "user".into(),
            SourceScope::User,
            SourceOrigin::TopLevel,
            None,
        );
        assert_eq!(info.path, "/tmp/test");
        assert_eq!(info.source, "user");
        assert_eq!(info.scope, SourceScope::User);
        assert_eq!(info.origin, SourceOrigin::TopLevel);
        assert!(info.base_dir.is_none());
    }

    #[test]
    fn test_create_synthetic_source_info_defaults() {
        let info = create_synthetic_source_info("/test".into(), "test".into(), None, None, None);
        assert_eq!(info.path, "/test");
        assert_eq!(info.scope, SourceScope::Temporary);
        assert_eq!(info.origin, SourceOrigin::TopLevel);
    }

    #[test]
    fn test_create_synthetic_source_info_explicit() {
        let info = create_synthetic_source_info(
            "/custom".into(),
            "extension".into(),
            Some(SourceScope::Project),
            Some(SourceOrigin::Package),
            Some("/base".into()),
        );
        assert_eq!(info.scope, SourceScope::Project);
        assert_eq!(info.origin, SourceOrigin::Package);
        assert_eq!(info.base_dir, Some("/base".into()));
    }

    #[test]
    fn test_create_builtin_source_info() {
        let info = create_builtin_source_info("goal");
        assert_eq!(info.path, "<builtin:goal>");
        assert_eq!(info.source, "builtin");
        assert_eq!(info.scope, SourceScope::Temporary);
        assert_eq!(info.origin, SourceOrigin::TopLevel);
        assert!(info.base_dir.is_none());
    }

    #[test]
    fn serde_scope_origin_lowercase() {
        let s = serde_json::to_string(&SourceScope::Temporary).unwrap_or_default();
        assert_eq!(s, "\"temporary\"");
        let o = serde_json::to_string(&SourceOrigin::TopLevel).unwrap_or_default();
        assert_eq!(o, "\"top-level\"");
    }
}
