//! Mirror of TS `api/constrained-sampling.ts` `makeStrictJsonSchema` —
//! convert a tool parameter schema to the strict subset expected by
//! provider constrained sampling (TS 0.84.2): closed objects with every
//! property required and optional non-nullable properties widened to
//! `anyOf: [schema, {type: "null"}]`.
//!
//! The original tool definitions are preserved; the conversion applies only
//! to the schema sent to providers that advertise strict-mode support.

use serde_json::Value;

/// Unsupported constructs (TS `UNSUPPORTED_STRICT_SCHEMA_KEYS`).
const UNSUPPORTED_STRICT_SCHEMA_KEYS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

/// TS `isStructuredSchema`: object/array schemas (or schemas with
/// properties/items) — used to reject object/array unions.
fn is_structured_schema(schema: &Value) -> bool {
    if !is_json_object(schema) {
        return false;
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    types.iter().any(|t| *t == "object" || *t == "array")
        || schema.get("properties").is_some()
        || schema.get("items").is_some()
}

/// TS `schemaAllowsNull`: does this schema accept an explicit `null`?
pub fn schema_allows_null(schema: &Value) -> bool {
    if !is_json_object(schema) {
        return false;
    }
    match schema.get("type") {
        Some(Value::String(t)) if t == "null" => return true,
        Some(Value::Array(arr)) if arr.iter().any(|v| v == "null") => return true,
        _ => {}
    }
    if schema.get("const") == Some(&Value::Null) {
        return true;
    }
    if let Some(enum_) = schema.get("enum").and_then(|v| v.as_array()) {
        if enum_.iter().any(|v| v.is_null()) {
            return true;
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(|v| v.as_array()) {
        if any_of.iter().any(schema_allows_null) {
            return true;
        }
    }
    false
}

fn unsupported(key: &str) -> String {
    format!("{key} schemas are unsupported")
}

fn make_json_schema_node_strict(schema: &mut Value) -> Result<(), String> {
    if !is_json_object(schema) {
        return Err("boolean schemas are unsupported".to_string());
    }
    for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
        if schema.get(*key).is_some() {
            return Err(unsupported(key));
        }
    }

    if schema.get("anyOf").is_some() {
        let any_of_len = schema
            .get("anyOf")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "anyOf must contain at least one schema".to_string())?
            .len();
        if any_of_len == 0 {
            return Err("anyOf must contain at least one schema".to_string());
        }
        let variants = schema
            .get_mut("anyOf")
            .and_then(|v| v.as_array_mut())
            .ok_or_else(|| "anyOf must contain at least one schema".to_string())?;
        for variant in variants.iter_mut() {
            if is_structured_schema(variant) {
                return Err("object and array unions are unsupported".to_string());
            }
            make_json_schema_node_strict(variant)?;
        }
    }

    if schema.get("items").is_some() {
        if schema.get("items").and_then(|v| v.as_array()).is_some() {
            return Err("tuple schemas are unsupported".to_string());
        }
        make_json_schema_node_strict(
            schema
                .get_mut("items")
                .expect("items present (checked above)"),
        )?;
    }

    let is_object_schema = schema.get("type") == Some(&Value::String("object".into()));
    if schema.get("properties").is_some() && !is_object_schema {
        return Err("properties require type object".to_string());
    }
    if !is_object_schema {
        return Ok(());
    }
    if let Some(ap) = schema.get("additionalProperties") {
        if ap != &Value::Bool(false) {
            return Err("schema-valued or true additionalProperties is unsupported".to_string());
        }
    }
    if schema.get("properties").is_some() {
        let _props = schema
            .get("properties")
            .and_then(|v| v.as_object())
            .ok_or_else(|| "object properties must be a schema map".to_string())?;
    }
    if let Some(required) = schema.get("required") {
        if !required
            .as_array()
            .is_some_and(|arr| arr.iter().all(|v| v.is_string()))
        {
            return Err("object required must be a string array".to_string());
        }
    }

    let property_names: Vec<String> = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let required: std::collections::HashSet<String> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    for key in &property_names {
        let Some(prop) = schema
            .get_mut("properties")
            .and_then(|p| p.as_object_mut())
            .and_then(|m| m.get_mut(key))
        else {
            continue;
        };
        make_json_schema_node_strict(prop)?;
        if !required.contains(key) && !schema_allows_null(prop) {
            let widened = serde_json::json!({
                "anyOf": [prop.clone(), {"type": "null"}]
            });
            *prop = widened;
        }
    }
    // Every property becomes required (TS `schema.required = propertyNames`).
    if let Some(obj) = schema.as_object_mut() {
        obj.insert(
            "required".to_string(),
            Value::Array(property_names.into_iter().map(Value::String).collect()),
        );
        obj.insert("additionalProperties".to_string(), Value::Bool(false));
    }
    Ok(())
}

/// Convert a tool parameter schema to the strict subset (TS
/// `makeStrictJsonSchema`). Returns the converted schema (the input is
/// cloned; the original tool definition is untouched).
pub fn make_strict_json_schema(parameters: &Value) -> Result<Value, String> {
    let mut cloned = parameters.clone();
    if !is_json_object(&cloned) {
        return Err("root schema must have type object".to_string());
    }
    make_json_schema_node_strict(&mut cloned)?;
    if cloned.get("type") != Some(&Value::String("object".into())) {
        return Err("root schema must have type object".to_string());
    }
    Ok(cloned)
}

/// TS `getJsonSchemaToolParameters`: apply the strict conversion only when
/// strict sampling is active.
pub fn get_json_schema_tool_parameters(
    parameters: &Value,
    strict: Option<bool>,
) -> Result<Value, String> {
    if strict == Some(true) {
        make_strict_json_schema(parameters)
    } else {
        Ok(parameters.clone())
    }
}

fn is_json_object(v: &Value) -> bool {
    matches!(v, Value::Object(_))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn strict_converts_optional_to_nullable_required() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "required_field": {"type": "string"},
                "optional_field": {"type": "string"}
            },
            "required": ["required_field"]
        });
        let strict = make_strict_json_schema(&schema).unwrap();
        // Every property becomes required (order follows serde_json Map
        // iteration, i.e. sorted — order is not semantically meaningful).
        let mut required: Vec<&str> = strict["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        required.sort_unstable();
        assert_eq!(required, vec!["optional_field", "required_field"]);
        // Closed object.
        assert_eq!(strict["additionalProperties"], serde_json::Value::Bool(false));
        // Optional non-nullable property widened to anyOf [schema, null].
        assert_eq!(
            strict["properties"]["optional_field"],
            serde_json::json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
        );
        // Required property untouched.
        assert_eq!(strict["properties"]["required_field"], serde_json::json!({"type": "string"}));
    }

    #[test]
    fn strict_rejects_unsupported_keys() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "additionalProperties": true
        });
        assert!(make_strict_json_schema(&schema).is_err());
    }

    #[test]
    fn strict_allows_null_optional_unchanged() {
        // Nested optional object is widened to anyOf [converted, null]; the
        // inner optional property is itself widened (TS recursive semantics).
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]}
            }
        });
        let strict = make_strict_json_schema(&schema).unwrap();
        // `a` is optional and non-nullable → widened.
        assert_eq!(
            strict["properties"]["a"]["anyOf"][0]["type"],
            serde_json::json!("object")
        );
        // `x` was required inside `a` → stays a plain string.
        assert_eq!(
            strict["properties"]["a"]["anyOf"][0]["properties"]["x"],
            serde_json::json!({"type": "string"})
        );
        assert_eq!(
            strict["properties"]["a"]["anyOf"][1],
            serde_json::json!({"type": "null"})
        );
    }

    #[test]
    fn get_tool_parameters_respects_strict_flag() {
        let schema = serde_json::json!({"type": "object", "properties": {}});
        assert_eq!(
            get_json_schema_tool_parameters(&schema, None).unwrap(),
            schema
        );
        assert_eq!(
            get_json_schema_tool_parameters(&schema, Some(false)).unwrap(),
            schema
        );
        let strict = get_json_schema_tool_parameters(&schema, Some(true)).unwrap();
        assert_eq!(strict["additionalProperties"], serde_json::Value::Bool(false));
        assert_eq!(strict["required"], serde_json::json!([]));
    }
}
