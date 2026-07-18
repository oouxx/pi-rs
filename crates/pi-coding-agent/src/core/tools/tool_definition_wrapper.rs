use std::sync::Arc;

use pi_agent_core::types::AgentTool;
use serde::de::DeserializeOwned;

use crate::core::extensions::ToolDefinition;

/// Wrap a ToolDefinition into an AgentTool for the core runtime.
///
/// When `definition.execute` is set, the AgentTool uses it as the real
/// execute implementation. Otherwise a stub error is returned.
pub fn wrap_tool_definition<TDetails>(
    definition: ToolDefinition,
) -> AgentTool<serde_json::Value, TDetails>
where
    TDetails: Clone + Send + Sync + 'static + serde::de::DeserializeOwned,
{
    let params = definition
        .parameters
        .clone()
        .unwrap_or(serde_json::Value::Null);
    let exec_mode = definition
        .execution_mode
        .as_deref()
        .and_then(|m| match m {
            "sequential" => Some(pi_agent_core::pi_ai_types::ToolExecutionMode::Sequential),
            _ => None,
        });

    let execute: Arc<
        dyn Fn(
                String,
                serde_json::Value,
                Option<tokio::sync::watch::Receiver<bool>>,
                Option<
                    Arc<
                        dyn Fn(
                                pi_agent_core::types::AgentToolResult<TDetails>,
                            ) + Send
                            + Sync,
                    >,
                >,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<
                                pi_agent_core::types::AgentToolResult<TDetails>,
                                Box<dyn std::error::Error + Send + Sync>,
                            >,
                        > + Send,
                >,
            > + Send
            + Sync,
    > = if let Some(ref tool_exec) = definition.execute {
        let exec = tool_exec.clone();
        Arc::new(move |id, params, signal, _on_update| {
            let exec = exec.clone();
            Box::pin(async move {
                let output = exec(id.clone(), params, signal).await?;
                let content: Vec<pi_agent_core::pi_ai_types::ContentBlock> = output
                    .content
                    .into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect();
                let details: TDetails = output
                    .details
                    .map(|v| serde_json::from_value(v).unwrap_or_else(|_| {
                        serde_json::from_value(serde_json::Value::Null).unwrap()
                    }))
                    .unwrap_or_else(|| {
                        serde_json::from_value(serde_json::Value::Null).unwrap()
                    });
                Ok(pi_agent_core::types::AgentToolResult {
                    content,
                    details,
                    terminate: None,
                })
            })
        })
    } else {
        Arc::new(|_id, _args, _signal, _on_update| {
            Box::pin(async move {
                Err("Tool not implemented via definition wrapper (execute not wired)".into())
            })
        })
    };

    AgentTool {
        name: definition.name,
        description: definition.description,
        label: definition.label.unwrap_or_default(),
        parameters_schema: params,
        execution_mode: exec_mode,
        prepare_arguments: None,
        execute,
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
        label: Some(tool.label.clone()),
        description: tool.description.clone(),
        prompt_snippet: None,
        prompt_guidelines: None,
        parameters: Some(tool.parameters_schema.clone()),
        render_shell: None,
        execution_mode: tool.execution_mode.map(|m| match m {
            pi_agent_core::pi_ai_types::ToolExecutionMode::Sequential => "sequential".into(),
            pi_agent_core::pi_ai_types::ToolExecutionMode::Parallel => "parallel".into(),
        }),
        execute: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_tool_definition() {
        let def = ToolDefinition {
            name: "test_tool".into(),
            label: Some("Test Tool".into()),
            description: "A test tool".into(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                }
            })),
            render_shell: None,
            execution_mode: None,
            execute: None,
        };

        let tool = wrap_tool_definition::<()>(def);
        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.description, "A test tool");
        assert_eq!(tool.label, "Test Tool");
    }

    #[test]
    fn test_wrap_multiple_definitions() {
        let defs = vec![
            ToolDefinition {
                name: "tool1".into(),
                label: None,
                description: "First tool".into(),
                prompt_snippet: None,
                prompt_guidelines: None,
                parameters: None,
                render_shell: None,
                execution_mode: None,
                execute: None,
            },
            ToolDefinition {
                name: "tool2".into(),
                label: None,
                description: "Second tool".into(),
                prompt_snippet: None,
                prompt_guidelines: None,
                parameters: None,
                render_shell: None,
                execution_mode: None,
                execute: None,
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
            label: "My Tool".into(),
            parameters_schema: serde_json::json!({"type": "object"}),
            execution_mode: None,
            prepare_arguments: None,
            execute: Arc::new(|_id, _args, _signal, _on_update| {
                Box::pin(async move { Err("not implemented".into()) })
            }),
        };

        let def = create_tool_definition_from_agent_tool(&tool);
        assert_eq!(def.name, "my_tool");
        assert_eq!(def.description, "My custom tool");
        assert_eq!(def.label, Some("My Tool".into()));
        assert!(def.parameters.is_some());
    }

    #[test]
    fn test_wrap_definition_without_params() {
        let def = ToolDefinition {
            name: "simple_tool".into(),
            label: None,
            description: "A tool with no params".into(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters: None,
            render_shell: None,
            execution_mode: None,
            execute: None,
        };

        let tool = wrap_tool_definition::<()>(def);
        assert_eq!(tool.name, "simple_tool");
        assert_eq!(tool.parameters_schema, serde_json::Value::Null);
    }
}
