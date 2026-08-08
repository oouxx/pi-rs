//! Structured assistant-message diagnostics (match TS `utils/diagnostics.ts`).
//!
//! The `AssistantMessageDiagnostic` type itself lives in `types.rs`; this
//! module provides the small helpers for constructing them.

use crate::types::{AssistantMessage, AssistantMessageDiagnostic, DiagnosticErrorInfo};

/// Extract a `DiagnosticErrorInfo` from a thrown value
/// (match TS `extractDiagnosticError`).
pub fn extract_diagnostic_error(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> DiagnosticErrorInfo {
    let code = error
        .downcast_ref::<crate::providers::pi_messages::PiMessagesResponseError>()
        .and_then(|e| e.code.clone())
        .map(serde_json::Value::String);
    DiagnosticErrorInfo {
        name: None,
        message: error.to_string(),
        stack: None,
        code,
    }
}

/// Create a structured diagnostic (match TS `createAssistantMessageDiagnostic`).
pub fn create_assistant_message_diagnostic(
    type_: &str,
    error: Option<&(dyn std::error::Error + Send + Sync + 'static)>,
    details: Option<serde_json::Map<String, serde_json::Value>>,
) -> AssistantMessageDiagnostic {
    AssistantMessageDiagnostic {
        type_field: type_.to_string(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        error: error.map(extract_diagnostic_error),
        details,
    }
}

/// Append a structured diagnostic to a message (match TS `appendAssistantMessageDiagnostic`).
pub fn append_assistant_message_diagnostic(
    message: &mut AssistantMessage,
    diagnostic: AssistantMessageDiagnostic,
) {
    message.diagnostics.get_or_insert_with(Vec::new).push(diagnostic);
}
