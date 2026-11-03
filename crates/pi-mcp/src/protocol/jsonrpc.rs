//! JSON-RPC 2.0 message types and validation.
//!
//! Port of `packages/mcp/src/protocol/jsonrpc.ts`. Messages are validated the
//! same way the TypeScript client validates them: `method` makes a request or
//! notification, otherwise an `id` with `result`/`error` makes a response.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{McpError, Result};

/// JSON-RPC request id: a string or an integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    /// Numeric id.
    Number(i64),
    /// String id.
    String(String),
}

impl fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(value) => write!(f, "{value}"),
            Self::String(value) => write!(f, "{value}"),
        }
    }
}

impl From<i64> for JsonRpcId {
    fn from(value: i64) -> Self {
        Self::Number(value)
    }
}

impl From<String> for JsonRpcId {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for JsonRpcId {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    /// Error code.
    pub code: i64,
    /// Error message.
    pub message: String,
    /// Optional structured data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A JSON-RPC request.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcRequest {
    /// Request id.
    pub id: JsonRpcId,
    /// Method name.
    pub method: String,
    /// Optional parameters.
    pub params: Option<Value>,
}

/// A JSON-RPC notification (no id, no response).
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcNotification {
    /// Method name.
    pub method: String,
    /// Optional parameters.
    pub params: Option<Value>,
}

/// A JSON-RPC response.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonRpcResponse {
    /// Successful response.
    Success {
        /// Request id.
        id: JsonRpcId,
        /// Result value.
        result: Value,
    },
    /// Error response.
    Error {
        /// Request id.
        id: JsonRpcId,
        /// Error object.
        error: JsonRpcErrorObject,
    },
}

impl JsonRpcResponse {
    /// The id this response answers.
    pub fn id(&self) -> &JsonRpcId {
        match self {
            Self::Success { id, .. } | Self::Error { id, .. } => id,
        }
    }
}

/// Any JSON-RPC message.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonRpcMessage {
    /// A request.
    Request(JsonRpcRequest),
    /// A notification.
    Notification(JsonRpcNotification),
    /// A response.
    Response(JsonRpcResponse),
}

impl JsonRpcMessage {
    /// Build a request.
    pub fn request(id: impl Into<JsonRpcId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self::Request(JsonRpcRequest {
            id: id.into(),
            method: method.into(),
            params,
        })
    }

    /// Build a notification.
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self::Notification(JsonRpcNotification {
            method: method.into(),
            params,
        })
    }

    /// Build a successful response.
    pub fn success(id: impl Into<JsonRpcId>, result: Value) -> Self {
        Self::Response(JsonRpcResponse::Success {
            id: id.into(),
            result,
        })
    }

    /// Build an error response.
    pub fn error(id: impl Into<JsonRpcId>, error: JsonRpcErrorObject) -> Self {
        Self::Response(JsonRpcResponse::Error {
            id: id.into(),
            error,
        })
    }

    /// Whether this message expects a response.
    pub fn is_request(&self) -> bool {
        matches!(self, Self::Request(_))
    }

    /// The method name for requests and notifications.
    pub fn method(&self) -> Option<&str> {
        match self {
            Self::Request(request) => Some(&request.method),
            Self::Notification(notification) => Some(&notification.method),
            Self::Response(_) => None,
        }
    }

    /// Serialize to a JSON value exactly as it goes on the wire.
    pub fn to_json_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        match self {
            Self::Request(request) => {
                object.insert("id".to_string(), serde_json::to_value(&request.id).unwrap_or(Value::Null));
                object.insert("method".to_string(), Value::String(request.method.clone()));
                if let Some(params) = &request.params {
                    object.insert("params".to_string(), params.clone());
                }
            }
            Self::Notification(notification) => {
                object.insert(
                    "method".to_string(),
                    Value::String(notification.method.clone()),
                );
                if let Some(params) = &notification.params {
                    object.insert("params".to_string(), params.clone());
                }
            }
            Self::Response(JsonRpcResponse::Success { id, result }) => {
                object.insert("id".to_string(), serde_json::to_value(id).unwrap_or(Value::Null));
                object.insert("result".to_string(), result.clone());
            }
            Self::Response(JsonRpcResponse::Error { id, error }) => {
                object.insert("id".to_string(), serde_json::to_value(id).unwrap_or(Value::Null));
                object.insert(
                    "error".to_string(),
                    serde_json::to_value(error).unwrap_or(Value::Null),
                );
            }
        }
        Value::Object(object)
    }

    /// Serialize to a compact JSON string.
    pub fn to_json_string(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.to_json_value())?)
    }
}

/// Standard JSON-RPC error codes.
#[derive(Debug, Clone, Copy)]
pub struct JsonRpcErrorCodes {
    /// Malformed JSON.
    pub parse_error: i64,
    /// Invalid request object.
    pub invalid_request: i64,
    /// Unknown method.
    pub method_not_found: i64,
    /// Invalid method parameters.
    pub invalid_params: i64,
    /// Internal error.
    pub internal_error: i64,
}

/// The standard JSON-RPC error codes, matching the TS constants.
pub const JSON_RPC_ERROR_CODES: JsonRpcErrorCodes = JsonRpcErrorCodes {
    parse_error: -32700,
    invalid_request: -32600,
    method_not_found: -32601,
    invalid_params: -32602,
    internal_error: -32603,
};

/// Whether `value` is a valid JSON-RPC id (string or integer).
pub fn is_json_rpc_id(value: &Value) -> bool {
    value.is_string() || value.as_i64().is_some()
}

fn parse_id(value: &Value) -> Option<JsonRpcId> {
    if let Some(string) = value.as_str() {
        return Some(JsonRpcId::String(string.to_string()));
    }
    value.as_i64().map(JsonRpcId::Number)
}

fn is_object(value: &Value) -> bool {
    value.is_object()
}

/// Parse an unknown JSON value into a validated JSON-RPC message.
///
/// Mirrors `parseJsonRpcMessage`: requests/notifications are recognized by
/// `method`, responses by `id` plus `result`/`error`.
pub fn parse_json_rpc_message(value: Value) -> Result<JsonRpcMessage> {
    if !is_object(&value) {
        return Err(McpError::invalid_request("Invalid JSON-RPC message"));
    }
    let object = match &value {
        Value::Object(object) => object,
        _ => unreachable!("checked object above"),
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(McpError::invalid_request("Invalid JSON-RPC message"));
    }

    if let Some(method) = object.get("method").and_then(Value::as_str) {
        if let Some(id_value) = object.get("id") {
            let Some(id) = parse_id(id_value) else {
                return Err(McpError::invalid_request("Invalid JSON-RPC message"));
            };
            return Ok(JsonRpcMessage::Request(JsonRpcRequest {
                id,
                method: method.to_string(),
                params: object.get("params").cloned(),
            }));
        }
        return Ok(JsonRpcMessage::Notification(JsonRpcNotification {
            method: method.to_string(),
            params: object.get("params").cloned(),
        }));
    }

    let id = object
        .get("id")
        .and_then(parse_id)
        .ok_or_else(|| McpError::invalid_request("Invalid JSON-RPC message"))?;

    if let Some(result) = object.get("result") {
        if object.contains_key("error") {
            return Err(McpError::invalid_request("Invalid JSON-RPC message"));
        }
        return Ok(JsonRpcMessage::Response(JsonRpcResponse::Success {
            id,
            result: result.clone(),
        }));
    }

    let Some(error) = object.get("error") else {
        return Err(McpError::invalid_request("Invalid JSON-RPC message"));
    };
    let error_object = error
        .as_object()
        .ok_or_else(|| McpError::invalid_request("Invalid JSON-RPC message"))?;
    let code = error_object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| McpError::invalid_request("Invalid JSON-RPC message"))?;
    let message = error_object
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::invalid_request("Invalid JSON-RPC message"))?;
    Ok(JsonRpcMessage::Response(JsonRpcResponse::Error {
        id,
        error: JsonRpcErrorObject {
            code,
            message: message.to_string(),
            data: error_object.get("data").cloned(),
        },
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_request() {
        let message = parse_json_rpc_message(json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {"cursor": "a"}
        }))
        .unwrap();
        match message {
            JsonRpcMessage::Request(request) => {
                assert_eq!(request.id, JsonRpcId::Number(7));
                assert_eq!(request.method, "tools/list");
                assert_eq!(request.params.unwrap()["cursor"], "a");
            }
            other => panic!("expected request, got {other:?}"),
        }
    }

    #[test]
    fn notification_has_no_id() {
        let message =
            parse_json_rpc_message(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
        assert!(matches!(message, JsonRpcMessage::Notification(_)));
    }

    #[test]
    fn response_success_and_error() {
        let success = parse_json_rpc_message(json!({"jsonrpc": "2.0", "id": "x", "result": {"ok": true}})).unwrap();
        assert!(matches!(success, JsonRpcMessage::Response(JsonRpcResponse::Success { .. })));

        let error = parse_json_rpc_message(json!({
            "jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "nope"}
        }))
        .unwrap();
        match error {
            JsonRpcMessage::Response(JsonRpcResponse::Error { error, .. }) => {
                assert_eq!(error.code, -32601);
            }
            other => panic!("expected error response, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_messages() {
        for value in [
            json!(null),
            json!({"jsonrpc": "1.0", "id": 1, "result": {}}),
            json!({"jsonrpc": "2.0", "id": true, "result": {}}),
            json!({"jsonrpc": "2.0", "id": 1}),
            json!({"jsonrpc": "2.0", "id": 1, "result": {}, "error": {}}),
        ] {
            assert!(parse_json_rpc_message(value).is_err());
        }
    }

    #[test]
    fn serializes_without_empty_params() {
        let message = JsonRpcMessage::notification("notifications/initialized", None);
        let value = message.to_json_value();
        assert!(value.get("params").is_none());
        assert!(value.get("id").is_none());
        assert_eq!(value["jsonrpc"], "2.0");
    }
}
