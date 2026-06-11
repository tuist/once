//! `once mcp` — expose Once's graph queries over the Model Context
//! Protocol so a coding agent can call `query_targets`,
//! `query_capabilities`, and `query_schema` as MCP tools.
//!
//! Transport is newline-delimited JSON over stdio: every request is
//! one line of JSON-RPC 2.0 in, every response is one line out. The
//! handshake follows MCP 2024-11-05: the client sends `initialize`,
//! we reply with our server info and the `tools` capability, the
//! client sends a `notifications/initialized` and then `tools/list`
//! to discover what we can do, then `tools/call` for each tool.
//!
//! Action invocation (call a graph action, get a digest back, query
//! the cached outputs / logs / provider record by that digest later)
//! is intentional follow-up work — that surface is read-write and
//! needs design choices about cross-workspace cache lookups that
//! aren't worth bundling with the initial inspection-only ramp.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP protocol version we negotiate.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server until stdin closes.
pub async fn serve(workspace: PathBuf) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    let server = Server::new(workspace);

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match server.dispatch_line(&line) {
            DispatchOutcome::Reply(response) => {
                let bytes = serde_json::to_vec(&response).context("encoding MCP response")?;
                stdout.write_all(&bytes).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
            DispatchOutcome::Notification => {
                // Notifications get no reply per JSON-RPC 2.0.
            }
        }
    }
    Ok(())
}

/// State the dispatcher needs to answer requests.
struct Server {
    workspace: PathBuf,
}

#[derive(Debug)]
enum DispatchOutcome {
    Reply(JsonRpcResponse),
    Notification,
}

impl Server {
    fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    fn dispatch_line(&self, line: &str) -> DispatchOutcome {
        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(error) => {
                return DispatchOutcome::Reply(JsonRpcResponse::error(
                    Value::Null,
                    PARSE_ERROR,
                    format!("parse error: {error}"),
                ));
            }
        };
        let id = request.id.clone();
        // JSON-RPC notifications carry no id; we still walk into the
        // handler so methods like `notifications/initialized` can
        // run, but the reply is suppressed.
        let is_notification = id.is_none();
        let response = self.dispatch(request);
        if is_notification {
            DispatchOutcome::Notification
        } else {
            DispatchOutcome::Reply(response.with_id_fallback(id))
        }
    }

    fn dispatch(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        let id = request.id.clone().unwrap_or(Value::Null);
        match request.method.as_str() {
            "initialize" => JsonRpcResponse::ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "once-mcp", "version": env!("CARGO_PKG_VERSION") },
                }),
            ),
            "notifications/initialized" => JsonRpcResponse::ok(id, Value::Null),
            "tools/list" => JsonRpcResponse::ok(id, json!({ "tools": tool_definitions() })),
            "tools/call" => self.handle_tool_call(id, request.params),
            other => JsonRpcResponse::error(
                id,
                METHOD_NOT_FOUND,
                format!("unknown MCP method `{other}`"),
            ),
        }
    }

    fn handle_tool_call(&self, id: Value, params: Option<Value>) -> JsonRpcResponse {
        let Some(params) = params else {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "tools/call requires params");
        };
        let call: ToolCallParams = match serde_json::from_value(params) {
            Ok(call) => call,
            Err(error) => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    format!("malformed tools/call params: {error}"),
                )
            }
        };
        let result = match call.name.as_str() {
            "once_query_targets" => self.tool_query_targets(&call.arguments),
            "once_query_capabilities" => self.tool_query_capabilities(&call.arguments),
            // Schema queries don't need a workspace because they
            // read from the compiled-in rule prelude.
            "once_query_schema" => tool_query_schema(&call.arguments),
            other => Err(anyhow::anyhow!("unknown tool `{other}`")),
        };
        match result {
            Ok(value) => JsonRpcResponse::ok(id, json!({ "content": [text_content(&value)] })),
            Err(error) => tool_error(id, &error.to_string()),
        }
    }

    fn tool_query_targets(&self, args: &Value) -> Result<Value> {
        let kind = args.get("kind").and_then(Value::as_str).map(str::to_string);
        let graph =
            once_frontend::load_graph_workspace(&self.workspace).context("loading graph")?;
        let records: Vec<TargetView> = graph
            .into_iter()
            .filter(|target| kind.as_deref().is_none_or(|k| target.kind == k))
            .map(TargetView::from)
            .collect();
        Ok(serde_json::to_value(records)?)
    }

    fn tool_query_capabilities(&self, args: &Value) -> Result<Value> {
        let target_id = args
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?;
        let graph =
            once_frontend::load_graph_workspace(&self.workspace).context("loading graph")?;
        let target = graph
            .into_iter()
            .find(|target| target.label.id == target_id)
            .with_context(|| format!("no target matches `{target_id}`"))?;
        Ok(serde_json::to_value(CapabilityView::from(target))?)
    }
}

fn tool_query_schema(args: &Value) -> Result<Value> {
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `kind` argument"))?;
    let schema = once_frontend::built_in_rule_schemas_result()?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no built-in rule schema matches `{kind}`"))?;
    Ok(serde_json::to_value(schema)?)
}

fn tool_error(id: Value, message: &str) -> JsonRpcResponse {
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

fn text_content(value: &Value) -> Value {
    text_content_str(&serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
}

fn text_content_str(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn tool_definitions() -> Vec<Value> {
    // The runtime `tools/list` reply is the wire projection of the
    // shared catalog; the doc generator walks the same catalog so
    // the reference page can't drift from the server's advertised
    // surface.
    tool_catalog()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect()
}

/// A single MCP tool, structured so the same record drives the
/// runtime `tools/list` reply and the generated reference page. The
/// fields cover both surfaces: `description` is the one-liner the
/// agent host sees in `tools/list`, `long_description` is the
/// markdown body rendered below the input schema on the reference,
/// and `example_return` is a JSON snippet shown as a worked example.
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub long_description: &'static str,
    pub input_schema: Value,
    pub example_return: &'static str,
}

/// All tools `once mcp` exposes, in the same order they appear in
/// `tools/list` and on the reference page.
pub fn tool_catalog() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "once_query_targets",
            description: "List every declared target in the workspace, optionally filtered by rule kind.",
            long_description: "Returns the same record shape as `once query targets --format json`: one entry per declared target with its canonical id, package, name, rule kind, dep edges, and the capabilities it exposes. The optional `kind` argument narrows results to a single rule.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "description": "Restrict results to targets of this rule kind (e.g. `apple_library`)."
                    }
                }
            }),
            example_return: "[\n  { \"id\": \"apps/ios/AppCore\", \"package\": \"apps/ios\", \"name\": \"AppCore\",\n    \"kind\": \"apple_library\", \"deps\": [], \"capabilities\": [\"build\"] },\n  { \"id\": \"apps/ios/Greeter\", \"package\": \"apps/ios\", \"name\": \"Greeter\",\n    \"kind\": \"apple_library\", \"deps\": [\"apps/ios/AppCore\"],\n    \"capabilities\": [\"build\"] }\n]",
        },
        ToolDefinition {
            name: "once_query_capabilities",
            description: "Return the capabilities (`build`, `run`, `test`) a target exposes, with their output groups and required inputs.",
            long_description: "Returns the same record `once query capabilities <target> --format json` emits: the target's id and kind plus one entry per capability with its output groups (what running the capability produces) and required outputs (what it depends on having built).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Canonical target id, e.g. `apps/ios/App`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"id\": \"apps/ios/App\",\n  \"kind\": \"apple_application\",\n  \"capabilities\": [\n    { \"name\": \"build\", \"output_groups\": [\"bundle\", \"dsyms\"],\n      \"requires_outputs\": [] },\n    { \"name\": \"run\", \"output_groups\": [\"default\"],\n      \"requires_outputs\": [\"bundle\"] }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_query_schema",
            description: "Return the typed contract for a rule kind: attributes, dep edges, providers, and capabilities.",
            long_description: "Returns the rule schema (the typed contract a target of that kind must match) as `once query schema <kind> --format json` would. The record carries the rule's documentation, attribute list (with types, required flag, and whether the attribute is configurable), expected dep providers, emitted providers, and exposed capabilities.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "description": "Rule kind to introspect, e.g. `apple_library`."
                    }
                },
                "required": ["kind"]
            }),
            example_return: "{\n  \"kind\": \"apple_library\",\n  \"docs\": \"Mixed Swift, Objective-C, C, and C++ static library...\",\n  \"attrs\": [\n    { \"name\": \"platform\", \"ty\": \"string\", \"required\": true, \"configurable\": true },\n    { \"name\": \"sdk_frameworks\", \"ty\": \"list<string>\", \"required\": false, \"configurable\": true }\n  ],\n  \"capabilities\": [ { \"name\": \"build\", \"output_groups\": [\"archive\"], \"requires_outputs\": [] } ],\n  \"providers\": [\"SwiftInfo\", \"CcInfo\"]\n}",
        },
    ]
}

#[derive(Debug, Serialize)]
struct TargetView {
    id: String,
    package: String,
    name: String,
    kind: String,
    deps: Vec<String>,
    capabilities: Vec<String>,
}

impl From<once_frontend::GraphTarget> for TargetView {
    fn from(target: once_frontend::GraphTarget) -> Self {
        Self {
            id: target.label.id,
            package: target.label.package,
            name: target.label.name,
            kind: target.kind,
            deps: target.deps,
            capabilities: target
                .capabilities
                .into_iter()
                .map(|capability| capability.name)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct CapabilityView {
    id: String,
    kind: String,
    capabilities: Vec<CapabilityDetail>,
}

#[derive(Debug, Serialize)]
struct CapabilityDetail {
    name: String,
    output_groups: Vec<String>,
    requires_outputs: Vec<String>,
}

impl From<once_frontend::GraphTarget> for CapabilityView {
    fn from(target: once_frontend::GraphTarget) -> Self {
        Self {
            id: target.label.id,
            kind: target.kind,
            capabilities: target
                .capabilities
                .into_iter()
                .map(|capability| CapabilityDetail {
                    name: capability.name,
                    output_groups: capability.output_groups,
                    requires_outputs: capability.requires_outputs,
                })
                .collect(),
        }
    }
}

// ---------- JSON-RPC envelope ----------

const PARSE_ERROR: i32 = -32700;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

impl JsonRpcResponse {
    fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
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

    fn with_id_fallback(mut self, id: Option<Value>) -> Self {
        if let Some(id) = id {
            self.id = id;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn server(workspace: PathBuf) -> Server {
        Server::new(workspace)
    }

    fn request(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            id: Some(Value::from(1)),
            method: method.to_string(),
            params: Some(params),
        }
    }

    #[test]
    fn initialize_returns_protocol_version_and_tool_capability() {
        let tmp = TempDir::new().unwrap();
        let response = server(tmp.path().to_path_buf()).dispatch(request("initialize", json!({})));
        let result = response.result.expect("result");
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        // The presence of an empty `tools` capability object signals
        // we serve the tools API; clients gate `tools/list` on it.
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], "once-mcp");
    }

    #[test]
    fn tools_list_advertises_the_three_query_tools() {
        let tmp = TempDir::new().unwrap();
        let response = server(tmp.path().to_path_buf()).dispatch(request("tools/list", json!({})));
        let names: Vec<String> = response.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec![
                "once_query_targets".to_string(),
                "once_query_capabilities".to_string(),
                "once_query_schema".to_string(),
            ]
        );
    }

    #[test]
    fn unknown_method_returns_method_not_found_error() {
        let tmp = TempDir::new().unwrap();
        let response =
            server(tmp.path().to_path_buf()).dispatch(request("does/not/exist", json!({})));
        let error = response.error.expect("error");
        assert_eq!(error.code, METHOD_NOT_FOUND);
        assert!(error.message.contains("does/not/exist"));
    }

    #[test]
    fn tools_call_missing_required_argument_lands_in_is_error_payload() {
        let tmp = TempDir::new().unwrap();
        // `once_query_capabilities` requires `target`; omit it.
        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({ "name": "once_query_capabilities", "arguments": {} }),
        ));
        let result = response.result.expect("result");
        // Tool-level failures surface inside the result envelope so
        // the agent sees an error message instead of a protocol
        // error; the JSON-RPC layer stays successful.
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("missing `target`"));
    }

    #[test]
    fn parse_error_reports_back_with_null_id() {
        let outcome = server(PathBuf::new()).dispatch_line("not json {");
        match outcome {
            DispatchOutcome::Reply(response) => {
                let error = response.error.expect("error");
                assert_eq!(error.code, PARSE_ERROR);
                assert_eq!(response.id, Value::Null);
            }
            DispatchOutcome::Notification => panic!("expected reply, got notification"),
        }
    }

    #[test]
    fn notifications_initialized_does_not_emit_a_reply() {
        // A JSON-RPC notification carries no id; the dispatcher
        // suppresses the reply, otherwise the client would see an
        // unsolicited response with a null id.
        let line = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        })
        .to_string();
        match server(PathBuf::new()).dispatch_line(&line) {
            DispatchOutcome::Notification => {}
            DispatchOutcome::Reply(reply) => {
                panic!("expected notification (no reply), got {reply:?}")
            }
        }
    }

    #[test]
    fn query_schema_returns_the_apple_library_contract() {
        let tmp = TempDir::new().unwrap();
        let _ = server(tmp.path().to_path_buf());
        let value = tool_query_schema(&json!({ "kind": "apple_library" })).unwrap();
        assert_eq!(value["kind"], "apple_library");
        let attr_names: Vec<&str> = value["attrs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|attr| attr["name"].as_str().unwrap())
            .collect();
        assert!(attr_names.contains(&"platform"));
        assert!(attr_names.contains(&"sdk_frameworks"));
    }
}
