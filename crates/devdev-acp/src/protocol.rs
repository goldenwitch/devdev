//! JSON-RPC 2.0 message envelope types.

use serde::{Deserialize, Serialize};

/// A JSON-RPC 2.0 message — request, response, or notification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

/// A JSON-RPC 2.0 request (has `id` and `method`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl Request {
    pub fn new(
        id: impl Into<RequestId>,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

/// A JSON-RPC 2.0 response (has `id`, one of `result` or `error`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn success(id: impl Into<RequestId>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: impl Into<RequestId>, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: id.into(),
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC 2.0 notification (has `method` but no `id`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        }
    }
}

/// A request ID — either a number or a string per JSON-RPC 2.0.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum RequestId {
    Number(u64),
    String(String),
}

impl From<u64> for RequestId {
    fn from(n: u64) -> Self {
        Self::Number(n)
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<&str> for RequestId {
    fn from(s: &str) -> Self {
        Self::String(s.to_owned())
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Well-known JSON-RPC / ACP error codes.
pub mod error_codes {
    /// Standard JSON-RPC: parse error.
    pub const PARSE_ERROR: i32 = -32700;
    /// Standard JSON-RPC: invalid request.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Standard JSON-RPC: method not found.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Standard JSON-RPC: invalid params.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Standard JSON-RPC: internal error.
    pub const INTERNAL_ERROR: i32 = -32603;
    /// ACP: authentication required.
    pub const AUTH_REQUIRED: i32 = -32000;
}
