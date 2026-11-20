//! JSON-RPC envelope types and MCP tool result helpers.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(super) fn tool_error(id: Value, message: &str) -> JsonRpcResponse {
    // MCP tool errors are still successful JSON-RPC responses; the
    // tool result carries an `isError: true` flag and the message
    // surfaces to the agent.
    JsonRpcResponse::ok(
        id,
        json!({
            "isError": true,
            "content": [text_content_str(message)],
        }),
    )
}

pub(super) fn tool_error_from_error(id: Value, error: &anyhow::Error) -> JsonRpcResponse {
    let diagnostic = struo_frontend::analysis::AnalysisFailure::find(error)
        .map(|failure| failure.diagnostic.clone());
    let Some(diagnostic) = diagnostic else {
        return tool_error(id, &error.to_string());
    };
    let code = diagnostic.code.clone();
    JsonRpcResponse::ok(
        id,
        json!({
            "isError": true,
            "content": [text_content_str(&diagnostic.message)],
            "structuredContent": {
                "error": {
                    "code": code,
                    "message": diagnostic.message,
                    "diagnostics": [diagnostic],
                }
            },
        }),
    )
}

pub(super) fn text_content(value: &Value) -> Value {
    text_content_str(&serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
}

pub(super) fn text_content_str(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

// ---------- JSON-RPC envelope ----------

pub(super) const PARSE_ERROR: i32 = -32700;
pub(super) const METHOD_NOT_FOUND: i32 = -32601;
pub(super) const INVALID_PARAMS: i32 = -32602;

#[derive(Debug, Deserialize)]
pub(super) struct JsonRpcRequest {
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) jsonrpc: Option<String>,
    #[serde(default)]
    pub(super) id: Option<Value>,
    pub(super) method: String,
    #[serde(default)]
    pub(super) params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub(super) struct JsonRpcResponse {
    pub(super) jsonrpc: &'static str,
    pub(super) id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub(super) struct JsonRpcError {
    pub(super) code: i32,
    pub(super) message: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ToolCallParams {
    pub(super) name: String,
    #[serde(default)]
    pub(super) arguments: Value,
}

impl JsonRpcResponse {
    pub(super) fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub(super) fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }

    pub(super) fn with_id_fallback(mut self, id: Option<Value>) -> Self {
        if let Some(id) = id {
            self.id = id;
        }
        self
    }
}
