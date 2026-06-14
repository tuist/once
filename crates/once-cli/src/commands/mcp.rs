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
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP protocol version we negotiate.
const PROTOCOL_VERSION: &str = "2024-11-05";
const RUN_TARGET_TIMEOUT_SECS: u64 = 3 * 60;

/// Run the MCP server until stdin closes.
pub async fn serve(workspace: PathBuf, allow_run: bool) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();
    let server = Server::new(workspace, allow_run);

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
    allow_run: bool,
}

#[derive(Debug)]
enum DispatchOutcome {
    Reply(JsonRpcResponse),
    Notification,
}

impl Server {
    fn new(workspace: PathBuf, allow_run: bool) -> Self {
        Self {
            workspace,
            allow_run,
        }
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
            "tools/list" => {
                JsonRpcResponse::ok(id, json!({ "tools": tool_definitions(self.allow_run) }))
            }
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
            "once_get_target" => self.tool_get_target(&call.arguments),
            "once_run_target" if self.allow_run => self.tool_run_target(&call.arguments),
            "once_run_target" => Err(anyhow::anyhow!(
                "tool `once_run_target` requires starting `once mcp --allow-run`"
            )),
            "once_apply_edit" => self.tool_apply_edit(&call.arguments),
            // Rule registry queries don't need a workspace because they
            // read from the compiled-in rule prelude.
            "once_query_schema" => tool_query_schema(&call.arguments),
            "once_list_rules" => tool_list_rules(),
            "once_validate_target" => tool_validate_target(&call.arguments),
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

    fn tool_get_target(&self, args: &Value) -> Result<Value> {
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
        Ok(serde_json::to_value(target)?)
    }

    fn tool_apply_edit(&self, args: &Value) -> Result<Value> {
        let package = args
            .get("package")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `package` argument"))?;
        let raw_ops = args
            .get("operations")
            .ok_or_else(|| anyhow::anyhow!("missing `operations` argument"))?;
        let operations: Vec<once_frontend::EditOperation> = serde_json::from_value(raw_ops.clone())
            .map_err(|err| anyhow::anyhow!("invalid `operations`: {err}"))?;
        apply_edit_to_package(&self.workspace, package, &operations)
    }

    fn tool_run_target(&self, args: &Value) -> Result<Value> {
        let args: RunTargetArgs = serde_json::from_value(tool_args(args))?;
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .arg("-C")
            .arg(&self.workspace)
            .arg("--format")
            .arg("json")
            .arg("run");
        let output = output_with_timeout(
            command.arg(args.target),
            Duration::from_secs(RUN_TARGET_TIMEOUT_SECS),
        )?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("once run failed: {}", stderr.trim());
        }
        let stdout = String::from_utf8(output.stdout)?;
        Ok(serde_json::from_str(stdout.trim())?)
    }
}

fn output_with_timeout(
    command: &mut std::process::Command,
    timeout: Duration,
) -> Result<std::process::Output> {
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("once run timed out after {} seconds", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn tool_args(args: &Value) -> Value {
    if args.is_null() {
        json!({})
    } else {
        args.clone()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunTargetArgs {
    target: String,
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

fn tool_list_rules() -> Result<Value> {
    let schemas = once_frontend::built_in_rule_schemas_result()?;
    let summaries: Vec<RuleSummary> = schemas.into_iter().map(RuleSummary::from).collect();
    Ok(serde_json::to_value(summaries)?)
}

fn tool_validate_target(args: &Value) -> Result<Value> {
    let raw_target = args
        .get("target")
        .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?
        .clone();
    let spec: once_frontend::TargetSpec = serde_json::from_value(raw_target)
        .map_err(|err| anyhow::anyhow!("invalid `target`: {err}"))?;
    let schemas = once_frontend::built_in_rule_schemas_result()?;
    let diagnostics = once_frontend::validate_target(&spec, &schemas);
    if diagnostics.is_empty() {
        Ok(json!({ "valid": true }))
    } else {
        Ok(json!({ "valid": false, "diagnostics": diagnostics }))
    }
}

fn apply_edit_to_package(
    workspace: &std::path::Path,
    package: &str,
    operations: &[once_frontend::EditOperation],
) -> Result<Value> {
    let package_dir = crate::commands::edit::resolve_package_dir(workspace, package)?;
    let manifest_path = package_dir.join(once_frontend::TOML_BUILD_FILE_NAME);
    let existing = match std::fs::read_to_string(&manifest_path) {
        Ok(src) => src,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => {
            return Err(anyhow::anyhow!(
                "reading `{}`: {err}",
                manifest_path.display()
            ));
        }
    };
    match once_frontend::apply_operations(&existing, operations) {
        Ok(new_src) => {
            std::fs::create_dir_all(&package_dir).with_context(|| {
                format!("creating package directory `{}`", package_dir.display())
            })?;
            std::fs::write(&manifest_path, &new_src)
                .with_context(|| format!("writing `{}`", manifest_path.display()))?;
            Ok(json!({
                "applied": true,
                "path": manifest_path.to_string_lossy(),
            }))
        }
        Err(diagnostics) => Ok(json!({ "applied": false, "diagnostics": diagnostics })),
    }
}

#[derive(Debug, Serialize)]
struct RuleSummary {
    kind: String,
    docs: String,
    examples: Vec<RuleExampleSummary>,
}

#[derive(Debug, Serialize)]
struct RuleExampleSummary {
    slug: String,
    name: String,
    use_when: String,
}

impl From<once_frontend::RuleSchema> for RuleSummary {
    fn from(schema: once_frontend::RuleSchema) -> Self {
        Self {
            kind: schema.kind,
            docs: schema.docs,
            examples: schema
                .examples
                .into_iter()
                .map(|example| RuleExampleSummary {
                    slug: example.slug,
                    name: example.name,
                    use_when: example.use_when,
                })
                .collect(),
        }
    }
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

fn tool_definitions(allow_run: bool) -> Vec<Value> {
    // The runtime `tools/list` reply is the wire projection of the
    // shared catalog; the doc generator walks the same catalog so
    // the reference page can't drift from the server's advertised
    // surface.
    tool_catalog()
        .into_iter()
        .filter(|tool| allow_run || tool.name != "once_run_target")
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
#[allow(clippy::too_many_lines)]
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
            example_return: "{\n  \"id\": \"apps/ios/App\",\n  \"kind\": \"apple_application\",\n  \"capabilities\": [\n    { \"name\": \"build\", \"output_groups\": [\"default\", \"bundle\", \"dsyms\"],\n      \"requires_outputs\": [] }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_query_schema",
            description: "Return the typed contract for a rule kind: attributes, dep edges, providers, capabilities, and runnable starter examples.",
            long_description: "Returns the rule schema (the typed contract a target of that kind must match) as `once query schema <kind> --format json` would. The record carries the rule's documentation, attribute list (with types, required flag, and whether the attribute is configurable), expected dep providers, emitted providers, exposed capabilities, and a list of runnable starter examples. Each example bundles a slug, a one-line `use_when` hint, and the full file tree (`once.toml` plus source files) a caller would copy to get a working target.",
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
            example_return: "{\n  \"kind\": \"apple_library\",\n  \"docs\": \"Mixed Swift, Objective-C, C, and C++ static library...\",\n  \"attrs\": [\n    { \"name\": \"platform\", \"ty\": \"string\", \"required\": true, \"configurable\": true }\n  ],\n  \"capabilities\": [ { \"name\": \"build\", \"output_groups\": [\"archive\"], \"requires_outputs\": [] } ],\n  \"providers\": [\"apple_linkable\", \"apple_module\"],\n  \"examples\": [\n    {\n      \"slug\": \"apple-library-minimal\",\n      \"name\": \"Minimal Apple library\",\n      \"use_when\": \"...\",\n      \"files\": [\n        { \"path\": \"apps/Hello/once.toml\", \"contents\": \"[[target]]\\nname = \\\"Hello\\\"\\nkind = \\\"apple_library\\\"\\n...\" }\n      ]\n    }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_list_rules",
            description: "List every rule kind the registry knows about, with its one-line docs and example slugs.",
            long_description: "Lightweight discovery entry point. Returns one entry per rule kind containing the rule's documentation and the slugs of its bundled starter examples. Use this to discover what kinds of targets are buildable in the workspace before calling `once_query_schema` for the full contract of a chosen rule.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            example_return: "[\n  {\n    \"kind\": \"apple_library\",\n    \"docs\": \"Mixed Swift, Objective-C, C, and C++ static library...\",\n    \"examples\": [\n      { \"slug\": \"apple-library-minimal\", \"name\": \"Minimal Apple library\", \"use_when\": \"...\" }\n    ]\n  }\n]",
        },
        ToolDefinition {
            name: "once_get_target",
            description: "Return the resolved view of a single target: rule kind, srcs, deps, typed attrs, capabilities, providers.",
            long_description: "Returns the same `GraphTarget` record `once_query_targets` emits, scoped to one target id. Includes the target's typed attribute values (with the types declared by its rule schema), the capabilities it exposes, the providers it emits, and any diagnostics emitted while loading the manifest. Use this before editing a target to learn its current shape.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Canonical target id, e.g. `apps/Hello/Hello`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"label\": { \"package\": \"apps/Hello\", \"name\": \"Hello\", \"id\": \"apps/Hello/Hello\" },\n  \"kind\": \"apple_library\",\n  \"srcs\": [\"Sources/**/*.swift\"],\n  \"deps\": [],\n  \"attrs\": { \"platform\": \"ios\", \"minimum_os\": \"17.0\" },\n  \"capabilities\": [ { \"name\": \"build\", \"output_groups\": [\"default\", \"binary\"], \"requires_outputs\": [] } ],\n  \"providers\": [\"apple_linkable\", \"apple_module\"]\n}",
        },
        ToolDefinition {
            name: "once_run_target",
            description: "Run a target through the same action path as `once run`.",
            long_description: "Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Executes `once run --format json` for a target and returns the structured run record. The tool has the same side effects as the CLI: it may build dependencies, write `.once/out` outputs, install software, or launch a process.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Canonical target id to run."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"target\": \"tools/demo/LaunchApp\",\n  \"kind\": \"script\",\n  \"capability\": \"run\",\n  \"status\": \"completed\",\n  \"cache\": \"miss\",\n  \"outputs\": [\".once/out/tools/demo/LaunchApp/run\"]\n}",
        },
        ToolDefinition {
            name: "once_validate_target",
            description: "Validate a proposed `[[target]]` table against its rule schema. Returns structured diagnostics instead of prose.",
            long_description: "Schema-only validation: checks that the target declares a known rule kind, every required attribute is present, every declared attribute is known to the rule and matches the rule's declared type, and the target name is well-formed. The check is local; it does not resolve dep references or read other manifests. Returns `{ valid: true }` on success or `{ valid: false, diagnostics: [...] }` where each diagnostic carries a stable `code`, the offending `target` id, the offending `attribute` when applicable, and `repairs` an agent can apply.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "object",
                        "description": "Raw `[[target]]` table shape with `name`, `kind`, optional `deps`, `srcs`, and `attrs`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"valid\": false,\n  \"diagnostics\": [\n    {\n      \"code\": \"missing_required_attr\",\n      \"message\": \"rule `apple_library` requires attribute `platform`\",\n      \"target\": \"Hello\",\n      \"attribute\": \"platform\"\n    }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_apply_edit",
            description: "Apply a batch of `create` / `update` / `delete` operations to one `once.toml` atomically.",
            long_description: "Reads the manifest at `<workspace>/<package>/once.toml` (creating it if missing), applies the batch of operations against the in-memory document, and writes the result back only if every operation succeeds. Returns `{ applied: true, path: <manifest path> }` on success or `{ applied: false, diagnostics: [...] }` with the structured diagnostic shape used by `once_validate_target`. The original file is never partially modified.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {
                        "type": "string",
                        "description": "Package directory relative to the workspace root, e.g. `apps/Hello`. Use `\"\"` for the root manifest."
                    },
                    "operations": {
                        "type": "array",
                        "description": "Ordered list of operations. Each is `{ op: \"create\", target: {...} }`, `{ op: \"update\", target_name: \"...\", set: {...} }`, or `{ op: \"delete\", target_name: \"...\" }`.",
                        "items": { "type": "object" }
                    }
                },
                "required": ["package", "operations"]
            }),
            example_return: "{\n  \"applied\": true,\n  \"path\": \"apps/Hello/once.toml\"\n}",
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
        Server::new(workspace, false)
    }

    fn run_server(workspace: PathBuf) -> Server {
        Server::new(workspace, true)
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
    fn tools_list_omits_run_tool_by_default() {
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
                "once_list_rules".to_string(),
                "once_get_target".to_string(),
                "once_validate_target".to_string(),
                "once_apply_edit".to_string(),
            ]
        );
    }

    #[test]
    fn tools_list_advertises_run_tool_when_allowed() {
        let tmp = TempDir::new().unwrap();
        let response =
            run_server(tmp.path().to_path_buf()).dispatch(request("tools/list", json!({})));
        let names: Vec<String> = response.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"once_run_target".to_string()));
    }

    #[test]
    fn run_tool_requires_allow_run() {
        let tmp = TempDir::new().unwrap();
        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({ "name": "once_run_target", "arguments": { "target": "App" } }),
        ));
        let result = response.result.expect("result");

        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("--allow-run"));
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

    #[test]
    fn query_schema_inlines_example_files() {
        let value = tool_query_schema(&json!({ "kind": "apple_library" })).unwrap();
        let examples = value["examples"].as_array().expect("examples is an array");
        assert!(
            !examples.is_empty(),
            "apple_library should advertise at least one example"
        );
        let minimal = examples
            .iter()
            .find(|e| e["slug"] == "apple-library-minimal")
            .expect("apple-library-minimal example present");
        assert!(!minimal["name"].as_str().unwrap().is_empty());
        assert!(!minimal["use_when"].as_str().unwrap().is_empty());
        let files = minimal["files"].as_array().expect("files is an array");
        assert!(files
            .iter()
            .any(|f| f["path"] == "apps/Hello/once.toml"
                && !f["contents"].as_str().unwrap().is_empty()));
    }

    #[test]
    fn list_rules_includes_every_known_rule() {
        let value = tool_list_rules().unwrap();
        let kinds: Vec<&str> = value
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"apple_library"));
        assert!(kinds.contains(&"apple_application"));
        // The list summarizes; full schema lives behind once_query_schema.
        assert!(value.as_array().unwrap()[0].get("attrs").is_none());
    }

    #[test]
    fn validate_target_returns_valid_for_minimal_apple_library() {
        let value = tool_validate_target(&json!({
            "target": {
                "name": "Hello",
                "kind": "apple_library",
                "attrs": { "platform": "ios" }
            }
        }))
        .unwrap();
        assert_eq!(value["valid"], true);
    }

    #[test]
    fn validate_target_returns_structured_diagnostics_on_failure() {
        let value = tool_validate_target(&json!({
            "target": {
                "name": "Hello",
                "kind": "apple_library"
            }
        }))
        .unwrap();
        assert_eq!(value["valid"], false);
        let diagnostics = value["diagnostics"].as_array().unwrap();
        let missing = diagnostics
            .iter()
            .find(|d| d["code"] == "missing_required_attr")
            .expect("missing_required_attr diagnostic");
        assert_eq!(missing["target"], "Hello");
        assert_eq!(missing["attribute"], "platform");
    }

    #[test]
    fn apply_edit_creates_manifest_when_missing() {
        let tmp = TempDir::new().unwrap();
        let result = apply_edit_to_package(
            tmp.path(),
            "apps/Hello",
            &[once_frontend::EditOperation::Create {
                target: once_frontend::TargetSpec {
                    name: "Hello".to_string(),
                    kind: "apple_library".to_string(),
                    attrs: serde_json::Map::from_iter([("platform".to_string(), json!("ios"))]),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        assert_eq!(result["applied"], true);
        let written = std::fs::read_to_string(tmp.path().join("apps/Hello/once.toml")).unwrap();
        assert!(written.contains("name = \"Hello\""));
    }

    #[test]
    fn apply_edit_returns_diagnostics_on_failure_without_writing() {
        let tmp = TempDir::new().unwrap();
        let result = apply_edit_to_package(
            tmp.path(),
            "apps/Hello",
            &[once_frontend::EditOperation::Delete {
                target_name: "Missing".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(result["applied"], false);
        assert_eq!(result["diagnostics"][0]["code"], "target_not_found");
        // Nothing should have been written when the batch failed.
        assert!(!tmp.path().join("apps/Hello/once.toml").exists());
    }

    #[test]
    fn get_target_returns_resolved_target_record() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("apps/Hello");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("once.toml"),
            "[[target]]\nname = \"Hello\"\nkind = \"apple_library\"\n[target.attrs]\nplatform = \"ios\"\n",
        )
        .unwrap();
        let value = server(tmp.path().to_path_buf())
            .tool_get_target(&json!({ "target": "apps/Hello/Hello" }))
            .unwrap();
        assert_eq!(value["label"]["id"], "apps/Hello/Hello");
        assert_eq!(value["kind"], "apple_library");
        assert_eq!(value["attrs"]["platform"], "ios");
    }

    #[test]
    fn apply_edit_then_get_target_round_trips_through_disk() {
        let tmp = TempDir::new().unwrap();
        let server = server(tmp.path().to_path_buf());
        // 1) Create a target via apply_edit.
        let create = server
            .tool_apply_edit(&json!({
                "package": "apps/Hello",
                "operations": [
                    {
                        "op": "create",
                        "target": {
                            "name": "Hello",
                            "kind": "apple_library",
                            "attrs": { "platform": "ios" }
                        }
                    }
                ]
            }))
            .unwrap();
        assert_eq!(create["applied"], true);
        // 2) Read it back via get_target.
        let target = server
            .tool_get_target(&json!({ "target": "apps/Hello/Hello" }))
            .unwrap();
        assert_eq!(target["label"]["id"], "apps/Hello/Hello");
        assert_eq!(target["kind"], "apple_library");
    }
}
