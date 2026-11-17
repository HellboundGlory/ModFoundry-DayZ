//! Minimal JSON-RPC 2.0 for the MCP stdio transport.
//!
//! MCP stdio framing is one JSON object per newline-delimited line; JSON-RPC
//! *batching* was removed from the MCP spec (2025-06-18), so no arrays are
//! produced or accepted.

use serde::Serialize;
use serde_json::Value;

pub const VERSION: &str = "2.0";

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// An unknown (or unreachable) resource URI. The MCP specification documents
/// this in prose only — there is no constant for it in the schema.
pub const RESOURCE_NOT_FOUND: i64 = -32002;

/// A successfully decoded inbound message.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// Carries an `id`; a response is expected.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// No `id`; the server never replies.
    Notification,
}

/// Result of decoding one line: a message to dispatch, or an error response
/// the transport must send (for malformed JSON there is no usable `id`).
#[derive(Debug, Clone)]
pub enum Decode {
    Message(Incoming),
    /// A response to emit (always an error) without further dispatch.
    Reject(Response),
}

/// Parse one MCP stdio line into a message or a rejection response.
pub fn decode(text: &str) -> Decode {
    let trimmed = text.trim();
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            return Decode::Reject(Response::error(Value::Null, PARSE_ERROR, "parse error"));
        }
    };
    let Some(obj) = value.as_object() else {
        return Decode::Reject(Response::error(
            Value::Null,
            INVALID_REQUEST,
            "expected an object",
        ));
    };
    let jsonrpc = obj.get("jsonrpc").and_then(Value::as_str);
    let method = obj.get("method").and_then(Value::as_str);
    if jsonrpc != Some(VERSION) || method.is_none() {
        let id = obj.get("id").cloned().unwrap_or(Value::Null);
        return Decode::Reject(Response::error(id, INVALID_REQUEST, "invalid request"));
    }
    let id = obj.get("id").cloned();
    let message = match id {
        Some(id) => Incoming::Request {
            id,
            method: method.unwrap().to_string(),
            params: obj.get("params").cloned().unwrap_or(Value::Null),
        },
        None => Incoming::Notification,
    };
    Decode::Message(message)
}

/// A JSON-RPC 2.0 response object.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl Response {
    pub fn result(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self::error_with_data(id, code, message, None)
    }

    /// An error carrying the MCP `data` field — the spec attaches the offending
    /// URI to a resource-not-found.
    pub fn error_with_data(
        id: Value,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self {
            jsonrpc: VERSION,
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }

    /// The spec's resource-not-found shape: `-32002` with the URI in `data`. A
    /// body that could not produce the resource keeps its own reason there too:
    /// the code is the spec's `Resource not found`, but an agent should still
    /// learn *why* (the file is gone, the path is not a file).
    pub fn resource_not_found(id: Value, uri: &str, reason: Option<String>) -> Self {
        let mut data = serde_json::json!({ "uri": uri });
        if let Some(reason) = reason {
            data["reason"] = serde_json::json!(reason);
        }
        Self::error_with_data(id, RESOURCE_NOT_FOUND, "Resource not found", Some(data))
    }
}
