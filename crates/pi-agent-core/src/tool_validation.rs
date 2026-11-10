//! Tool argument coercion and validation (port of TS `validateToolArguments`).
//!
//! v0.87.1 runs `Compile(tool.parameters).parse(args)`: common model mistakes
//! are coerced first (`"42"` -> `42`, `"true"` -> `true`, `null` for an
//! optional argument -> omitted) and the result is then validated against the
//! JSON Schema. A failure becomes an error tool result instead of reaching the
//! tool implementation.
//!
//! This module implements the keywords the built-in and extension tools use:
//! `type` (including unions), `enum`, `const`, `properties`/`required`,
//! `additionalProperties`, `items` (single schema and tuple),
//! `anyOf`/`oneOf`/`allOf`. `format`/`pattern`/`minimum`/`maximum` are not
//! enforced yet; that gap is recorded in `DEVIATIONS.md`.

use serde_json::{Map, Value};

/// Coerce and validate `args` against `schema`.
///
/// Returns the coerced object on success, or a human-readable validation
/// message (the caller wraps it in the TS-shaped
/// `Validation failed for tool ...` error result).
pub fn validate_tool_arguments(schema: &Value, args: &Value) -> Result<Value, String> {
    if schema.is_null() {
        return Ok(args.clone());
    }
    let mut value = args.clone();
    normalize_optional_nulls(&mut value, schema);
    // Match the existing behavior: keys not declared in `properties` are
    // dropped before validation (providers often echo extra fields).
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        if let Some(obj) = value.as_object() {
            value = Value::Object(
                obj.iter()
                    .filter(|(key, _)| properties.contains_key(*key))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            );
        }
    }
    coerce_and_validate(&value, schema, "")
}

fn path_or_root(path: &str) -> &str {
    if path.is_empty() {
        "root"
    } else {
        path
    }
}

fn schema_types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(ty)) => vec![ty.clone()],
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(|ty| ty.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn matches_json_type(value: &Value, ty: &str) -> bool {
    match ty {
        "number" => value.is_number(),
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn coerce_primitive(value: &Value, ty: &str) -> Option<Value> {
    match ty {
        "number" => match value {
            Value::Null => Some(Value::from(0)),
            Value::String(s) if !s.trim().is_empty() => s
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .map(Value::from),
            Value::Bool(b) => Some(Value::from(if *b { 1 } else { 0 })),
            Value::Number(n) => Some(Value::Number(n.clone())),
            _ => None,
        },
        "integer" => match value {
            Value::Null => Some(Value::from(0)),
            Value::String(s) if !s.trim().is_empty() => {
                s.parse::<i64>().ok().map(Value::from)
            }
            Value::Bool(b) => Some(Value::from(if *b { 1 } else { 0 })),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(Value::from(i))
                } else if let Some(u) = n.as_u64() {
                    Some(Value::from(u))
                } else {
                    n.as_f64()
                        .filter(|f| f.fract() == 0.0)
                        .map(Value::from)
                }
            }
            _ => None,
        },
        "boolean" => match value {
            Value::Null => Some(Value::from(false)),
            Value::String(s) if s == "true" => Some(Value::from(true)),
            Value::String(s) if s == "false" => Some(Value::from(false)),
            Value::Number(n) if n.as_i64() == Some(1) => Some(Value::from(true)),
            Value::Number(n) if n.as_i64() == Some(0) => Some(Value::from(false)),
            Value::Bool(b) => Some(Value::from(*b)),
            _ => None,
        },
        "string" => match value {
            Value::Null => Some(Value::from("")),
            Value::Number(n) => Some(Value::from(n.to_string())),
            Value::Bool(b) => Some(Value::from(b.to_string())),
            Value::String(s) => Some(Value::from(s.clone())),
            _ => None,
        },
        "null" => match value {
            Value::Null => Some(Value::Null),
            Value::String(s) if s.is_empty() => Some(Value::Null),
            Value::Number(n) if n.as_i64() == Some(0) => Some(Value::Null),
            Value::Bool(false) => Some(Value::Null),
            _ => None,
        },
        "array" => value.is_array().then(|| value.clone()),
        "object" => value.is_object().then(|| value.clone()),
        _ => Some(value.clone()),
    }
}

fn coerce_and_validate(value: &Value, schema: &Value, path: &str) -> Result<Value, String> {
    let mut current = value.clone();

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for nested in all_of {
            current = coerce_and_validate(&current, nested, path)?;
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        current = coerce_union(&current, any_of, path, "anyOf")?;
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        current = coerce_union(&current, one_of, path, "oneOf")?;
    }

    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|candidate| candidate == &current) {
            return Err(format!(
                "{}: must be one of {}",
                path_or_root(path),
                serde_json::to_string(enum_values).unwrap_or_default()
            ));
        }
    }
    if let Some(const_value) = schema.get("const") {
        if const_value != &current {
            return Err(format!(
                "{}: must be {}",
                path_or_root(path),
                serde_json::to_string(const_value).unwrap_or_default()
            ));
        }
    }

    let types = schema_types(schema);
    if types.is_empty() {
        if schema.get("properties").is_some() {
            return validate_object(&current, schema, path);
        }
        return Ok(current);
    }

    // The value already matches one of the allowed types: validate it as-is.
    if types.iter().any(|ty| matches_json_type(&current, ty)) {
        return validate_typed(&current, schema, &types, path);
    }

    // Otherwise try the same primitive coercions TypeBox applies.
    let mut errors = Vec::new();
    for ty in &types {
        if let Some(coerced) = coerce_primitive(&current, ty) {
            if matches_json_type(&coerced, ty) {
                match validate_typed(&coerced, schema, &types, path) {
                    Ok(value) => return Ok(value),
                    Err(error) => errors.push(error),
                }
            }
        } else {
            errors.push(format!("{}: expected {}", path_or_root(path), ty));
        }
    }
    Err(errors
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("{}: invalid value", path_or_root(path))))
}

fn coerce_union(value: &Value, variants: &[Value], path: &str, keyword: &str) -> Result<Value, String> {
    // Prefer a variant that validates without changing the value, then fall
    // back to coercion (same order as TS `coerceWithUnionSchema`).
    for variant in variants {
        if let Ok(validated) = coerce_and_validate(value, variant, path) {
            if validated == *value {
                return Ok(validated);
            }
        }
    }
    for variant in variants {
        if let Ok(validated) = coerce_and_validate(value, variant, path) {
            return Ok(validated);
        }
    }
    Err(format!(
        "{}: does not match {}",
        path_or_root(path),
        keyword
    ))
}

fn validate_typed(value: &Value, schema: &Value, types: &[String], path: &str) -> Result<Value, String> {
    if types.iter().any(|ty| ty == "object") && value.is_object() {
        return validate_object(value, schema, path);
    }
    if types.iter().any(|ty| ty == "array") && value.is_array() {
        return validate_array(value, schema, path);
    }
    Ok(value.clone())
}

fn validate_object(value: &Value, schema: &Value, path: &str) -> Result<Value, String> {
    let Some(obj) = value.as_object() else {
        return Err(format!("{}: expected object", path_or_root(path)));
    };
    let properties = schema.get("properties").and_then(Value::as_object);

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                // TS `formatValidationPath` reports `basePath.requiredProperty`.
                let required_path = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                return Err(format!(
                    "{}: required property '{}' is missing",
                    required_path, key
                ));
            }
        }
    }

    let mut out = Map::new();
    for (key, value) in obj {
        let child_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if let Some(property_schema) = properties.and_then(|props| props.get(key)) {
            out.insert(
                key.clone(),
                coerce_and_validate(value, property_schema, &child_path)?,
            );
        } else if let Some(additional) = schema.get("additionalProperties") {
            if additional == &Value::Bool(false) {
                return Err(format!(
                    "{}: unexpected property '{}'",
                    path_or_root(path),
                    key
                ));
            }
            if additional.is_object() {
                out.insert(
                    key.clone(),
                    coerce_and_validate(value, additional, &child_path)?,
                );
            } else {
                out.insert(key.clone(), value.clone());
            }
        } else {
            out.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(out))
}

fn validate_array(value: &Value, schema: &Value, path: &str) -> Result<Value, String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{}: expected array", path_or_root(path)));
    };
    let mut out = Vec::with_capacity(items.len());
    match schema.get("items") {
        Some(Value::Array(item_schemas)) => {
            for (index, item) in items.iter().enumerate() {
                let child_path = format!("{path}[{index}]");
                if let Some(item_schema) = item_schemas.get(index) {
                    out.push(coerce_and_validate(item, item_schema, &child_path)?);
                } else {
                    out.push(item.clone());
                }
            }
        }
        Some(item_schema) if item_schema.is_object() => {
            for (index, item) in items.iter().enumerate() {
                let child_path = format!("{path}[{index}]");
                out.push(coerce_and_validate(item, item_schema, &child_path)?);
            }
        }
        _ => out.extend(items.iter().cloned()),
    }
    Ok(Value::Array(out))
}

/// Whether a JSON schema accepts an explicit `null` value (TS
/// `schemaAllowsNull`): `type: "null"` / union containing null, `const:
/// null`, `enum` containing null, or an `anyOf` variant that allows null.
fn schema_allows_null(schema: &Value) -> bool {
    if let Some(ty) = schema.get("type") {
        if ty == "null" {
            return true;
        }
        if let Some(arr) = ty.as_array() {
            if arr.iter().any(|value| value == "null") {
                return true;
            }
        }
    }
    if schema.get("const") == Some(&Value::Null) {
        return true;
    }
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if enum_values.iter().any(Value::is_null) {
            return true;
        }
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        if any_of.iter().any(schema_allows_null) {
            return true;
        }
    }
    false
}

/// TS `normalizeOptionalNulls`: `null` values for optional non-nullable tool
/// arguments are treated as omitted — removed recursively (objects and array
/// items) before validation/execution.
fn normalize_optional_nulls(value: &mut Value, schema: &Value) {
    match value {
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for item in items.iter_mut() {
                    normalize_optional_nulls(item, item_schema);
                }
            }
        }
        Value::Object(obj) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            let required: std::collections::HashSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let keys: Vec<String> = obj.keys().cloned().collect();
            for key in keys {
                let Some(property_schema) = properties.get(&key) else {
                    continue;
                };
                if obj.get(&key) == Some(&Value::Null)
                    && !required.contains(key.as_str())
                    && !schema_allows_null(property_schema)
                {
                    obj.remove(&key);
                } else if let Some(child) = obj.get_mut(&key) {
                    normalize_optional_nulls(child, property_schema);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "limit": { "type": "integer" },
                "flag": { "type": "boolean" },
                "mode": { "type": "string", "enum": ["a", "b"] },
                "nested": {
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "required": ["name"]
                },
                "items": { "type": "array", "items": { "type": "integer" } }
            },
            "required": ["path"]
        })
    }

    #[test]
    fn rejects_missing_required_property() {
        let error = validate_tool_arguments(&schema(), &json!({ "limit": 1 })).unwrap_err();
        assert!(error.contains("required property 'path'"), "{error}");
    }

    #[test]
    fn coerces_primitives_like_typebox() {
        let validated = validate_tool_arguments(
            &schema(),
            &json!({ "path": "/tmp", "limit": "5", "flag": "true", "items": ["1", 2] }),
        )
        .unwrap();
        assert_eq!(validated["limit"], json!(5));
        assert_eq!(validated["flag"], json!(true));
        assert_eq!(validated["items"], json!([1, 2]));
    }

    #[test]
    fn rejects_invalid_enum_value() {
        let error =
            validate_tool_arguments(&schema(), &json!({ "path": "x", "mode": "c" })).unwrap_err();
        assert!(error.contains("must be one of"), "{error}");
    }

    #[test]
    fn validates_nested_objects_and_arrays() {
        let error = validate_tool_arguments(
            &schema(),
            &json!({ "path": "x", "nested": { "name": 3 } }),
        )
        .unwrap();
        assert_eq!(error["nested"]["name"], json!("3"));

        let error = validate_tool_arguments(&schema(), &json!({ "path": "x", "nested": {} }))
            .unwrap_err();
        assert!(error.contains("nested.name"), "{error}");
    }

    #[test]
    fn optional_null_is_treated_as_omitted() {
        let validated =
            validate_tool_arguments(&schema(), &json!({ "path": "x", "limit": null })).unwrap();
        assert!(!validated.as_object().unwrap().contains_key("limit"));
    }

    #[test]
    fn drops_undeclared_top_level_keys() {
        let validated =
            validate_tool_arguments(&schema(), &json!({ "path": "x", "extra": 1 })).unwrap();
        assert!(!validated.as_object().unwrap().contains_key("extra"));
    }

    #[test]
    fn type_union_accepts_null() {
        let schema = json!({
            "type": "object",
            "properties": { "value": { "type": ["string", "null"] } }
        });
        let validated = validate_tool_arguments(&schema, &json!({ "value": null })).unwrap();
        assert!(validated["value"].is_null());
    }
}
