use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
    User,
    Project,
    Temporary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SourceOrigin {
    Package,
    TopLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub path: String,
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
}

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
}
