//! Extension system tests for pi-coding-agent.
//!
//! These tests cover ToolDefinition serialization/deserialization.
//! Tests for the old load_extensions/LoadedExtension/ToolInfo system
//! were removed in Phase 6.6 cleanup (those types were dead code
//! replaced by the embedded deno_core JS runtime).

use pi_coding_agent::core::extensions::ToolDefinition;

// ============================================================================
// Helper functions
// ============================================================================

/// Create a basic tool definition for testing.
fn make_tool_def(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        label: None,
        description: description.to_string(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: None,
        execute: None,
    }
}

// ============================================================================
// ToolDefinition tests
// ============================================================================

#[test]
fn test_tool_definition_serialization_roundtrip() {
    let def = make_tool_def("read_file", "Read a file from the filesystem");
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "read_file");
    assert_eq!(parsed.description, "Read a file from the filesystem");
}

#[test]
fn test_tool_definition_minimal_serialization() {
    let def = ToolDefinition {
        name: "minimal".into(),
        label: None,
        description: String::new(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: None,
        execute: None,
    };
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "minimal");
    assert!(parsed.description.is_empty());
}

#[test]
fn test_tool_definition_with_execution_mode() {
    let def = ToolDefinition {
        name: "sequential_tool".into(),
        label: None,
        description: "A sequential tool".into(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: Some("sequential".into()),
        execute: None,
    };
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.execution_mode, Some("sequential".to_string()));
}

#[test]
fn test_tool_definition_with_parallel_mode() {
    let def = ToolDefinition {
        name: "parallel_tool".into(),
        label: None,
        description: "A parallel tool".into(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: None,
        render_shell: None,
        execution_mode: Some("parallel".into()),
        execute: None,
    };
    let json = serde_json::to_string(&def).unwrap();
    let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.execution_mode, Some("parallel".to_string()));
}

