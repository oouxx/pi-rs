use std::sync::Arc;

use pi_agent_core::types::AgentTool;

use crate::core::extensions::ToolDefinition;

/// Wrap a ToolDefinition into an AgentTool for the core runtime.
pub fn wrap_tool_definition<TDetails>(
    definition: ToolDefinition,
) -> AgentTool<serde_json::Value, TDetails>
where
    TDetails: Clone + Send + Sync + 'static,
{
    let params = definition.parameters.clone().unwrap_or(serde_json::Value::Null);
    let exec_mode = definition
        .execution_mode
        .clone()
        .and_then(|m| match m.as_str() {
            "sequential" => Some(pi_agent_core::pi_ai_types::ToolExecutionMode::Sequential),
            _ => None,
        });

    AgentTool {
        name: definition.name,
        description: definition.description,
        label: String::new(),
        parameters_schema: params,
        execution_mode: exec_mode,
        prepare_arguments: None,
        execute: Arc::new(|_id, _args, _signal, _on_update| {
            Box::pin(async move {
                Err("Tool not implemented via definition wrapper (execute not wired)".into())
            })
        }),
    }
}

/// Wrap multiple ToolDefinitions into AgentTools for the core runtime.
pub fn wrap_tool_definitions(
    definitions: &[ToolDefinition],
) -> Vec<AgentTool<serde_json::Value, serde_json::Value>> {
    definitions
        .iter()
        .map(|def| wrap_tool_definition::<serde_json::Value>(def.clone()))
        .collect()
}

/// Synthesize a minimal ToolDefinition from an AgentTool.
///
/// This keeps AgentSession's internal registry definition-first even when a caller
/// provides plain AgentTool overrides that do not include prompt metadata or renderers.
pub fn create_tool_definition_from_agent_tool(
    tool: &AgentTool<serde_json::Value, serde_json::Value>,
) -> ToolDefinition {
    ToolDefinition {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: Some(tool.parameters_schema.clone()),
        prompt_guidelines: None,
        execution_mode: tool
            .execution_mode
            .map(|m| match m {
                pi_agent_core::pi_ai_types::ToolExecutionMode::Sequential => "sequential".into(),
                pi_agent_core::pi_ai_types::ToolExecutionMode::Parallel => "parallel".into(),
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_tool_definition() {
        let def = ToolDefinition {
            name: "test_tool".into(),
            description: "A test tool".into(),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                }
            })),
            prompt_guidelines: None,
            execution_mode: None,
        };

        let tool = wrap_tool_definition::<()>(def);
        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.description, "A test tool");
    }

    #[test]
    fn test_wrap_multiple_definitions() {
        let defs = vec![
            ToolDefinition {
                name: "tool1".into(),
                description: "First tool".into(),
                parameters: None,
                prompt_guidelines: None,
                execution_mode: None,
            },
            ToolDefinition {
                name: "tool2".into(),
                description: "Second tool".into(),
                parameters: None,
                prompt_guidelines: None,
                execution_mode: None,
            },
        ];

        let tools = wrap_tool_definitions(&defs);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "tool1");
        assert_eq!(tools[1].name, "tool2");
    }

    #[test]
    fn test_create_definition_from_tool() {
        use std::sync::Arc;
        let tool = AgentTool {
            name: "my_tool".into(),
            description: "My custom tool".into(),
            label: String::new(),
            parameters_schema: serde_json::json!({"type": "object"}),
            execution_mode: None,
            prepare_arguments: None,
            execute: Arc::new(|_id, _args, _signal, _on_update| {
                Box::pin(async move {
                    Err("not implemented".into())
                })
            }),
        };

        let def = create_tool_definition_from_agent_tool(&tool);
        assert_eq!(def.name, "my_tool");
        assert_eq!(def.description, "My custom tool");
        assert!(def.parameters.is_some());
    }

    #[test]
    fn test_wrap_definition_without_params() {
        let def = ToolDefinition {
            name: "simple_tool".into(),
            description: "A tool with no params".into(),
            parameters: None,
            prompt_guidelines: None,
            execution_mode: None,
        };

        let tool = wrap_tool_definition::<()>(def);
        assert_eq!(tool.name, "simple_tool");
        assert_eq!(tool.parameters_schema, serde_json::Value::Null);
    }
}
