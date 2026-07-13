use serde::{Deserialize, Serialize};

// ============================================================================
// Tool Definition
// ============================================================================

/// Tool definition matching the original TypeScript ToolDefinition interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool name (used in LLM tool calls).
    pub name: String,
    /// Human-readable label for UI display.
    #[serde(default)]
    pub label: Option<String>,
    /// Description for the LLM.
    #[serde(default)]
    pub description: String,
    /// Optional one-line prompt snippet.
    #[serde(default)]
    pub prompt_snippet: Option<String>,
    /// Optional prompt guidelines for the LLM.
    #[serde(default)]
    pub prompt_guidelines: Option<Vec<String>>,
    /// JSON Schema for tool parameters.
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
    /// Shell rendering mode.
    #[serde(default)]
    pub render_shell: Option<String>,
    /// Execution mode: "sequential" or "parallel".
    #[serde(default)]
    pub execution_mode: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition_serde() {
        let def = ToolDefinition {
            name: "test".into(),
            label: Some("Test".into()),
            description: "A test tool".into(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: None,
        };
        let json = serde_json::to_string(&def).unwrap();
        let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
    }
}
