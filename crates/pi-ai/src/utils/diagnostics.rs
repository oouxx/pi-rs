use serde::{Deserialize, Serialize};

/// Diagnostic information for a content block within an assistant message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessageDiagnostic {
    #[serde(rename = "contentIndex")]
    pub content_index: usize,
    pub diagnostic: String,
    pub severity: DiagnosticSeverity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}
