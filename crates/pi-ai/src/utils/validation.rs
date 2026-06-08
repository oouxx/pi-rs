use serde_json::{Map, Value};

use crate::types::{Tool, ToolCall};

/// Find a tool by name and validate the tool call arguments against its schema.
pub fn validate_tool_call(tools: &[Tool], tool_call: &ToolCall) -> Result<Value, String> {
    let tool = tools
        .iter()
        .find(|t| t.name == tool_call.name)
        .ok_or_else(|| format!("Tool \"{}\" not found", tool_call.name))?;
    validate_tool_arguments(tool, tool_call)
}

/// Validate tool call arguments against the tool's JSON Schema.
/// Returns coerced arguments on success, or an error with details.
pub fn validate_tool_arguments(tool: &Tool, tool_call: &ToolCall) -> Result<Value, String> {
    let args = tool_call.arguments.clone();
    let schema = &tool.parameters;

    // JSON Schema must be an object with "type": "object" and "properties"
    let schema_obj = match schema {
        Value::Object(m) => m,
        _ => return Err(format!("Invalid schema for tool \"{}\"", tool.name)),
    };

    // If schema has no validation constraints, accept as-is
    let schema_type = schema_obj.get("type").and_then(Value::as_str);
    if schema_type != Some("object") {
        return Ok(args);
    }

    let properties = match schema_obj.get("properties") {
        Some(Value::Object(p)) => p,
        _ => return Ok(args),
    };

    let required = match schema_obj.get("required") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    let mut coerced_args = match args {
        Value::Object(m) => m,
        _ => {
            return Err(format!(
                "Expected object arguments for tool \"{}\", got {:?}",
                tool.name,
                args_type_name(&args)
            ));
        }
    };

    // Check required fields and coerce values by schema
    for required_field in &required {
        if !coerced_args.contains_key(required_field) {
            return Err(format!(
                "  - {}: is required\n\nReceived arguments:\n{}",
                required_field,
                serde_json::to_string_pretty(&tool_call.arguments).unwrap_or_default()
            ));
        }
    }

    let mut errors: Vec<String> = Vec::new();

    for (key, field_schema) in properties.iter() {
        if let Some(field_schema_obj) = field_schema.as_object() {
            if let Some(field_type) = field_schema_obj.get("type").and_then(Value::as_str) {
                if let Some(value) = coerced_args.get(key) {
                    match coerce_and_validate(value, field_schema_obj, field_type) {
                        Ok(coerced) => {
                            coerced_args.insert(key.clone(), coerced);
                        }
                        Err(e) => {
                            errors.push(format!("  - {}: {}", key, e));
                        }
                    }
                } else if required.contains(key) {
                    errors.push(format!("  - {}: is required", key));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(Value::Object(coerced_args))
    } else {
        Err(format!(
            "Validation failed for tool \"{}\":\n{}{}",
            tool.name,
            errors.join("\n"),
            format!(
                "\n\nReceived arguments:\n{}",
                serde_json::to_string_pretty(&tool_call.arguments).unwrap_or_default()
            )
        ))
    }
}

/// Try to coerce a value to match the expected JSON Schema type, and validate constraints.
fn coerce_and_validate(
    value: &Value,
    schema: &Map<String, Value>,
    expected_type: &str,
) -> Result<Value, String> {
    let coerced = coerce_primitive(value, expected_type);

    // Check minLength / maxLength for strings
    if expected_type == "string" {
        if let Some(s) = coerced.as_str() {
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
                if (s.len() as u64) < min {
                    return Err(format!("must be at least {} characters", min));
                }
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                if (s.len() as u64) > max {
                    return Err(format!("must be at most {} characters", max));
                }
            }
        }
    }

    // Check minimum / maximum for numbers
    if expected_type == "number" || expected_type == "integer" {
        if let Some(n) = coerced.as_f64() {
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64) {
                if n < min {
                    return Err(format!("must be >= {}", min));
                }
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64) {
                if n > max {
                    return Err(format!("must be <= {}", max));
                }
            }
        }
    }

    // Check enum values
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|ev| ev == &coerced) {
            let valid: Vec<String> = enum_values
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            return Err(format!("must be one of: {}", valid.join(", ")));
        }
    }

    // Recurse into nested objects
    if expected_type == "object" {
        if let Some(obj) = coerced.as_object() {
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                return validate_nested_object(obj, props, schema);
            }
        }
        return Ok(coerced);
    }

    Ok(coerced)
}

/// Validate a nested object value against a property schema.
fn validate_nested_object(
    obj: &Map<String, Value>,
    properties: &Map<String, Value>,
    schema: &Map<String, Value>,
) -> Result<Value, String> {
    let required = match schema.get("required") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };

    let mut coerced = obj.clone();

    for key in &required {
        if !coerced.contains_key(key) {
            return Err(format!("  - {}: is required", key));
        }
    }

    for (key, field_schema) in properties.iter() {
        if let Some(field_schema_obj) = field_schema.as_object() {
            if let Some(field_type) = field_schema_obj.get("type").and_then(Value::as_str) {
                if let Some(value) = coerced.get(key) {
                    match coerce_and_validate(value, field_schema_obj, field_type) {
                        Ok(c) => {
                            coerced.insert(key.clone(), c);
                        }
                        Err(e) => {
                            return Err(format!("{}.{}: {}", "", key, e));
                        }
                    }
                }
            }
        }
    }

    Ok(Value::Object(coerced))
}

/// Coerce a primitive value to match expected type.
fn coerce_primitive(value: &Value, expected_type: &str) -> Value {
    match expected_type {
        "number" => match value {
            Value::Null => Value::Number(0.into()),
            Value::String(s) => {
                if let Ok(n) = s.parse::<f64>() {
                    if n.fract() == 0.0 && n.is_finite() {
                        Value::Number(serde_json::Number::from_f64(n).unwrap_or(0.into()))
                    } else {
                        Value::Number(serde_json::Number::from_f64(n).unwrap_or(0.into()))
                    }
                } else {
                    value.clone()
                }
            }
            Value::Bool(b) => {
                if *b {
                    Value::Number(1.into())
                } else {
                    Value::Number(0.into())
                }
            }
            _ => value.clone(),
        },
        "integer" => match value {
            Value::Null => Value::Number(0.into()),
            Value::String(s) => {
                if let Ok(n) = s.parse::<i64>() {
                    Value::Number(n.into())
                } else {
                    value.clone()
                }
            }
            Value::Bool(b) => {
                if *b {
                    Value::Number(1.into())
                } else {
                    Value::Number(0.into())
                }
            }
            _ => value.clone(),
        },
        "boolean" => match value {
            Value::Null => Value::Bool(false),
            Value::String(s) => match s.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => value.clone(),
            },
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    match i {
                        1 => Value::Bool(true),
                        0 => Value::Bool(false),
                        _ => value.clone(),
                    }
                } else {
                    value.clone()
                }
            }
            _ => value.clone(),
        },
        "string" => match value {
            Value::Null => Value::String(String::new()),
            Value::Number(n) => Value::String(n.to_string()),
            Value::Bool(b) => Value::String(b.to_string()),
            _ => value.clone(),
        },
        "array" => match value {
            Value::Array(arr) => {
                // Recursive coercion for array items would go here
                // For simplicity, we pass arrays through
                Value::Array(arr.clone())
            }
            _ => value.clone(),
        },
        _ => value.clone(),
    }
}

fn args_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCall;

    fn make_tool(name: &str, schema: Value) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            parameters: schema,
        }
    }

    fn make_tool_call(name: &str, args: Value) -> ToolCall {
        ToolCall::new("call_1".to_string(), name.to_string(), args)
    }

    #[test]
    fn test_validate_tool_call_success() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path"]
        });
        let tools = vec![make_tool("write_file", schema)];
        let tc = make_tool_call(
            "write_file",
            serde_json::json!({"path": "/tmp/test.txt", "content": "hello"}),
        );
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tool_call_not_found() {
        let tools: Vec<Tool> = vec![];
        let tc = make_tool_call("nonexistent", serde_json::json!({}));
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_validate_tool_call_missing_required() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        });
        let tools = vec![make_tool("read_file", schema)];
        let tc = make_tool_call("read_file", serde_json::json!({}));
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("required"));
    }

    #[test]
    fn test_validate_tool_call_string_coercion_from_number() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });
        let tools = vec![make_tool("greet", schema)];
        let tc = make_tool_call("greet", serde_json::json!({"name": 42}));
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_ok());
        let args = result.unwrap();
        assert_eq!(args["name"], "42");
    }

    #[test]
    fn test_validate_tool_call_number_coercion_from_string() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "count": {"type": "number"}
            },
            "required": ["count"]
        });
        let tools = vec![make_tool("count", schema)];
        let tc = make_tool_call("count", serde_json::json!({"count": "42"}));
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tool_call_boolean_coercion() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "enabled": {"type": "boolean"}
            },
            "required": ["enabled"]
        });
        let tools = vec![make_tool("toggle", schema)];
        let tc = make_tool_call("toggle", serde_json::json!({"enabled": "true"}));
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_ok());
        let args = result.unwrap();
        assert_eq!(args["enabled"], true);
    }

    #[test]
    fn test_validate_tool_call_enum_constraint() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "unit": {
                    "type": "string",
                    "enum": ["celsius", "fahrenheit"]
                }
            },
            "required": ["unit"]
        });
        let tools = vec![make_tool("weather", schema)];
        let tc = make_tool_call("weather", serde_json::json!({"unit": "celsius"}));
        assert!(validate_tool_call(&tools, &tc).is_ok());

        let tc2 = make_tool_call("weather", serde_json::json!({"unit": "kelvin"}));
        assert!(validate_tool_call(&tools, &tc2).is_err());
    }

    #[test]
    fn test_validate_tool_call_nested_object() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "object",
                    "properties": {
                        "lat": {"type": "number"},
                        "lng": {"type": "number"}
                    },
                    "required": ["lat", "lng"]
                }
            },
            "required": ["location"]
        });
        let tools = vec![make_tool("geocode", schema)];
        let tc = make_tool_call(
            "geocode",
            serde_json::json!({"location": {"lat": 37.7749, "lng": -122.4194}}),
        );
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tool_call_min_length() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "code": {"type": "string", "minLength": 1}
            },
            "required": ["code"]
        });
        let tools = vec![make_tool("validate", schema)];
        let tc = make_tool_call("validate", serde_json::json!({"code": ""}));
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_tool_call_empty_schema_passes() {
        let schema = serde_json::json!({});
        let tools = vec![make_tool("noop", schema)];
        let tc = make_tool_call("noop", serde_json::json!({"anything": "goes"}));
        let result = validate_tool_call(&tools, &tc);
        assert!(result.is_ok());
    }
}
