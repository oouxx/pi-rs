//! String enum helper for JSON Schema, matching Typebox's StringEnum pattern.
//!
//! Produces `{ type: "string", enum: [...] }` schemas compatible with providers
//! (like Google's API) that don't support anyOf/const patterns.
//!
//! Ported from `packages/ai/src/utils/typebox-helpers.ts`.

/// Create a JSON Schema `{ type: "string", enum: [...] }` value.
///
/// This replicates Typebox's `Type.Unsafe<StringEnum>` without requiring
/// the Typebox crate itself.
///
/// # Example
/// ```
/// use pi_ai::utils::typebox_helpers::string_enum;
/// let schema = string_enum(&["add", "subtract", "multiply", "divide"],
///     Some("The operation to perform"), None);
/// assert_eq!(schema["type"], "string");
/// assert_eq!(schema["enum"].as_array().unwrap().len(), 4);
/// ```
pub fn string_enum<T: AsRef<str>>(
    values: &[T],
    description: Option<&str>,
    default_value: Option<&str>,
) -> serde_json::Value {
    let enum_values: Vec<serde_json::Value> =
        values.iter().map(|v| serde_json::Value::String(v.as_ref().to_string())).collect();

    let mut map = serde_json::Map::new();
    map.insert("type".into(), serde_json::Value::String("string".into()));
    map.insert("enum".into(), serde_json::Value::Array(enum_values));

    if let Some(desc) = description {
        map.insert("description".into(), serde_json::Value::String(desc.into()));
    }
    if let Some(def) = default_value {
        map.insert("default".into(), serde_json::Value::String(def.into()));
    }

    serde_json::Value::Object(map)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_enum_basic() {
        let schema = string_enum(&["add", "subtract"], None, None);
        assert_eq!(schema["type"], "string");
        let items = schema["enum"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], "add");
        assert_eq!(items[1], "subtract");
    }

    #[test]
    fn test_string_enum_with_description() {
        let schema = string_enum(&["a", "b"], Some("The operation"), None);
        assert_eq!(schema["description"], "The operation");
    }

    #[test]
    fn test_string_enum_with_default() {
        let schema = string_enum(&["x", "y", "z"], None, Some("y"));
        assert_eq!(schema["default"], "y");
    }

    #[test]
    fn test_string_enum_all_options() {
        let schema = string_enum(&["foo"], Some("A foo option"), Some("foo"));
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["description"], "A foo option");
        assert_eq!(schema["default"], "foo");
    }

    #[test]
    fn test_string_enum_empty_values() {
        let schema = string_enum::<&str>(&[], None, None);
        assert_eq!(schema["type"], "string");
        assert!(schema["enum"].as_array().unwrap().is_empty());
    }
}
