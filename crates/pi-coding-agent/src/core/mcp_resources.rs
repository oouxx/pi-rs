//! MCP resource tools: `list_mcp_resources`, `list_mcp_resource_templates`,
//! and `read_mcp_resource`.
//!
//! Port of `packages/coding-agent/src/extensions/mcp/resources.ts`.

use std::sync::Arc;

use futures::future::join_all;
use pi_mcp::{
    CallToolResult, ContentBlock, ListResourceTemplatesResult, ListResourcesResult, Resource,
    ResourceContents, ResourceTemplate,
};
use serde_json::{json, Value};

use crate::core::mcp_runtime::McpServerConnection;
use crate::core::mcp_servers::{
    McpExposure, LIST_MCP_RESOURCE_TEMPLATES_TOOL, LIST_MCP_RESOURCES_TOOL, READ_MCP_RESOURCE_TOOL,
};
use crate::core::mcp_tools::{limit_content, to_tool_exposure};

fn is_mcp_app_resource(uri: Option<&str>, mime_type: Option<&str>) -> bool {
    uri.is_some_and(|uri| uri.starts_with("ui://"))
        || mime_type.is_some_and(|mime_type| {
            let lower = mime_type.to_ascii_lowercase();
            lower.contains("profile=mcp-app") || lower.contains("profile=\"mcp-app\"")
        })
}

fn listed(server: &str, value: Value) -> Value {
    let mut object = value.as_object().cloned().unwrap_or_default();
    object.remove("_meta");
    object.remove("icons");
    let mut result = serde_json::Map::new();
    result.insert("server".to_string(), Value::String(server.to_string()));
    result.extend(object);
    Value::Object(result)
}

fn optional_string(params: &Value, key: &str) -> Result<Option<String>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            Ok(if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            })
        }
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn text_output(text: String, details: Value) -> pi_extension_api::ToolCallOutput {
    let (content, full_output) = limit_content(vec![json!({"type": "text", "text": text})]);
    let mut details = details;
    if let Some(path) = full_output {
        details["fullOutputPath"] = Value::String(path);
    }
    pi_extension_api::ToolCallOutput {
        content,
        details: Some(details),
        is_error: false,
        terminate: None,
    }
}

fn resource_to_value(resource: &Resource) -> Value {
    let mut value = serde_json::to_value(resource).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("_meta");
        object.remove("icons");
    }
    value
}

fn template_to_value(template: &ResourceTemplate) -> Value {
    let mut value = serde_json::to_value(template).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.remove("_meta");
        object.remove("icons");
    }
    value
}

/// Create the three resource tools for all connections with resources.
pub fn create_mcp_resource_tool_definitions(
    connections: Arc<Vec<McpServerConnection>>,
    exposure: McpExposure,
) -> Vec<pi_extension_api::ToolDefinition> {
    let read_only = pi_extension_api::ToolAnnotations {
        read_only_hint: Some(true),
        ..Default::default()
    };
    let list_parameters = json!({
        "type": "object",
        "properties": {
            "server": {"type": "string", "description": "MCP server name. Omit to list every server with resources."},
            "cursor": {"type": "string", "description": "Opaque cursor from a previous call with the same server; omit for the first page."}
        },
        "additionalProperties": false
    });
    let read_parameters = json!({
        "type": "object",
        "properties": {
            "server": {"type": "string", "description": "MCP server name exactly as configured. Must match the 'server' field returned by list_mcp_resources."},
            "uri": {"type": "string", "description": "Resource URI to read. Must be one of the URIs returned by list_mcp_resources."}
        },
        "required": ["server", "uri"],
        "additionalProperties": false
    });

    let list_resources = {
        let connections = Arc::clone(&connections);
        let execute: pi_extension_api::ToolExecuteFn = Arc::new(move |_id, params, _signal| {
            let connections = Arc::clone(&connections);
            Box::pin(async move {
                let server_name = optional_string(&params, "server")
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                let cursor = optional_string(&params, "cursor")
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                let payload = list_resources_payload(&connections, server_name.as_deref(), cursor)
                    .await
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                Ok(text_output(
                    serde_json::to_string(&payload).unwrap_or_default(),
                    json!({"server": server_name.unwrap_or_default(), "tool": LIST_MCP_RESOURCES_TOOL}),
                ))
            })
        });
        pi_extension_api::ToolDefinition {
            name: LIST_MCP_RESOURCES_TOOL.to_string(),
            label: Some(LIST_MCP_RESOURCES_TOOL.to_string()),
            description: "Lists resources provided by MCP servers. Resources allow servers to share data that provides context to language models, such as files, database schemas, or application-specific information. Prefer resources over web search when possible.".to_string(),
            parameters: Some(list_parameters.clone()),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "resources": {"type": "array"},
                    "nextCursor": {"type": "string"},
                    "errors": {"type": "array"}
                },
                "required": ["resources"]
            })),
            exposure: to_tool_exposure(exposure),
            annotations: Some(read_only.clone()),
            execute: Some(execute),
            ..Default::default()
        }
    };

    let list_templates = {
        let connections = Arc::clone(&connections);
        let execute: pi_extension_api::ToolExecuteFn = Arc::new(move |_id, params, _signal| {
            let connections = Arc::clone(&connections);
            Box::pin(async move {
                let server_name = optional_string(&params, "server")
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                let cursor = optional_string(&params, "cursor")
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                let payload =
                    list_templates_payload(&connections, server_name.as_deref(), cursor)
                        .await
                        .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                Ok(text_output(
                    serde_json::to_string(&payload).unwrap_or_default(),
                    json!({"server": server_name.unwrap_or_default(), "tool": LIST_MCP_RESOURCE_TEMPLATES_TOOL}),
                ))
            })
        });
        pi_extension_api::ToolDefinition {
            name: LIST_MCP_RESOURCE_TEMPLATES_TOOL.to_string(),
            label: Some(LIST_MCP_RESOURCE_TEMPLATES_TOOL.to_string()),
            description: "Lists resource templates provided by MCP servers. Parameterized resource templates allow servers to share data that takes parameters and provides context to language models, such as files, database schemas, or application-specific information. Prefer resource templates over web search when possible.".to_string(),
            parameters: Some(list_parameters),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "resourceTemplates": {"type": "array"},
                    "nextCursor": {"type": "string"},
                    "errors": {"type": "array"}
                },
                "required": ["resourceTemplates"]
            })),
            exposure: to_tool_exposure(exposure),
            annotations: Some(read_only),
            execute: Some(execute),
            ..Default::default()
        }
    };

    let read_resource = {
        let connections = Arc::clone(&connections);
        let execute: pi_extension_api::ToolExecuteFn = Arc::new(move |_id, params, _signal| {
            let connections = Arc::clone(&connections);
            Box::pin(async move {
                let server_name = optional_string(&params, "server")
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?
                    .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                        "server must be provided".to_string().into()
                    })?;
                let uri = optional_string(&params, "uri")
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?
                    .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                        "uri must be provided".to_string().into()
                    })?;
                let server = find_server(&connections, &server_name)
                    .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("MCP server \"{server_name}\" has no resources").into()
                    })?;
                let result = server
                    .read_resource(&uri)
                    .await
                    .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })?;
                let mut content = Vec::new();
                for contents in &result.contents {
                    if result.contents.len() > 1 {
                        content.push(json!({"type": "text", "text": format!("{}:", contents.uri())}));
                    }
                    let block = ContentBlock::Resource {
                        resource: contents.clone(),
                        annotations: None,
                        meta: None,
                    };
                    for llm in pi_mcp::to_llm_content(&CallToolResult {
                        content: vec![block],
                        structured_content: None,
                        is_error: false,
                        meta: None,
                    }) {
                        content.push(match llm {
                            pi_mcp::LlmContent::Text { text } => {
                                json!({"type": "text", "text": text})
                            }
                            pi_mcp::LlmContent::Image { data, mime_type } => {
                                json!({"type": "image", "data": data, "mimeType": mime_type})
                            }
                        });
                    }
                }
                if content.is_empty() {
                    content.push(json!({"type": "text", "text": format!("Resource {uri} is empty.")}));
                }
                let (content, full_output) = limit_content(content);
                let mut details = json!({"server": server_name, "tool": READ_MCP_RESOURCE_TOOL});
                if let Some(path) = full_output {
                    details["fullOutputPath"] = Value::String(path);
                }
                details["structuredContent"] = json!({
                    "server": server_name,
                    "uri": uri,
                    "contents": result.contents.iter().map(resource_contents_value).collect::<Vec<_>>()
                });
                Ok(pi_extension_api::ToolCallOutput {
                    content,
                    details: Some(details),
                    is_error: false,
                    terminate: None,
                })
            })
        });
        pi_extension_api::ToolDefinition {
            name: READ_MCP_RESOURCE_TOOL.to_string(),
            label: Some(READ_MCP_RESOURCE_TOOL.to_string()),
            description: "Read a specific resource from an MCP server given the server name and resource URI.".to_string(),
            parameters: Some(read_parameters),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "server": {"type": "string"},
                    "uri": {"type": "string"},
                    "contents": {"type": "array"}
                },
                "required": ["server", "uri", "contents"]
            })),
            exposure: to_tool_exposure(exposure),
            annotations: None,
            execute: Some(execute),
            ..Default::default()
        }
    };

    vec![list_resources, list_templates, read_resource]
}

fn resource_contents_value(contents: &ResourceContents) -> Value {
    match contents {
        ResourceContents::Text(text) => json!({
            "uri": text.uri,
            "mimeType": text.mime_type,
            "text": text.text
        }),
        ResourceContents::Blob(blob) => json!({
            "uri": blob.uri,
            "mimeType": blob.mime_type,
            "blob": blob.blob
        }),
    }
}

fn find_server<'a>(
    connections: &'a [McpServerConnection],
    name: &str,
) -> Option<&'a McpServerConnection> {
    connections.iter().find(|connection| connection.name() == name)
}

async fn list_resources_payload(
    connections: &[McpServerConnection],
    server_name: Option<&str>,
    cursor: Option<String>,
) -> Result<Value, String> {
    if let Some(server_name) = server_name {
        let server = find_server(connections, server_name)
            .ok_or_else(|| format!("MCP server \"{server_name}\" has no resources"))?;
        let page = server.list_resources_page(cursor).await?;
        return Ok(resources_page_payload(server, page));
    }
    if cursor.is_some() {
        return Err("cursor can only be used when a server is specified".to_string());
    }
    let mut servers = connections.to_vec();
    servers.sort_by(|a, b| a.name().cmp(b.name()));
    let results = join_all(servers.iter().map(|server| server.list_resources())).await;
    let mut resources = Vec::new();
    let mut errors = Vec::new();
    for (server, result) in servers.iter().zip(results) {
        match result {
            Ok(items) => resources.extend(
                items
                    .iter()
                    .filter(|resource| {
                        !is_mcp_app_resource(Some(&resource.uri), resource.mime_type.as_deref())
                    })
                    .map(|resource| listed(server.name(), resource_to_value(resource))),
            ),
            Err(error) => errors.push(json!({"server": server.name(), "error": error})),
        }
    }
    let mut payload = json!({"resources": resources});
    if !errors.is_empty() {
        payload["errors"] = Value::Array(errors);
    }
    Ok(payload)
}

fn resources_page_payload(server: &McpServerConnection, page: ListResourcesResult) -> Value {
    let mut payload = json!({
        "server": server.name(),
        "resources": page.resources.iter()
            .filter(|resource| !is_mcp_app_resource(Some(&resource.uri), resource.mime_type.as_deref()))
            .map(|resource| listed(server.name(), resource_to_value(resource)))
            .collect::<Vec<_>>()
    });
    if let Some(cursor) = page.next_cursor {
        payload["nextCursor"] = Value::String(cursor);
    }
    payload
}

async fn list_templates_payload(
    connections: &[McpServerConnection],
    server_name: Option<&str>,
    cursor: Option<String>,
) -> Result<Value, String> {
    if let Some(server_name) = server_name {
        let server = find_server(connections, server_name)
            .ok_or_else(|| format!("MCP server \"{server_name}\" has no resources"))?;
        let page = server.list_resource_templates_page(cursor).await?;
        return Ok(templates_page_payload(server, page));
    }
    if cursor.is_some() {
        return Err("cursor can only be used when a server is specified".to_string());
    }
    let mut servers = connections.to_vec();
    servers.sort_by(|a, b| a.name().cmp(b.name()));
    let results = join_all(
        servers
            .iter()
            .map(|server| server.list_resource_templates()),
    )
    .await;
    let mut templates = Vec::new();
    let mut errors = Vec::new();
    for (server, result) in servers.iter().zip(results) {
        match result {
            Ok(items) => templates.extend(
                items
                    .iter()
                    .filter(|template| {
                        !is_mcp_app_resource(Some(&template.uri_template), template.mime_type.as_deref())
                    })
                    .map(|template| listed(server.name(), template_to_value(template))),
            ),
            Err(error) => errors.push(json!({"server": server.name(), "error": error})),
        }
    }
    let mut payload = json!({"resourceTemplates": templates});
    if !errors.is_empty() {
        payload["errors"] = Value::Array(errors);
    }
    Ok(payload)
}

fn templates_page_payload(server: &McpServerConnection, page: ListResourceTemplatesResult) -> Value {
    let mut payload = json!({
        "server": server.name(),
        "resourceTemplates": page.resource_templates.iter()
            .filter(|template| !is_mcp_app_resource(Some(&template.uri_template), template.mime_type.as_deref()))
            .map(|template| listed(server.name(), template_to_value(template)))
            .collect::<Vec<_>>()
    });
    if let Some(cursor) = page.next_cursor {
        payload["nextCursor"] = Value::String(cursor);
    }
    payload
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn filters_mcp_app_resources() {
        assert!(is_mcp_app_resource(Some("ui://app"), None));
        assert!(is_mcp_app_resource(None, Some("text/html; profile=mcp-app")));
        assert!(!is_mcp_app_resource(Some("file:///a"), Some("text/plain")));
    }
}
