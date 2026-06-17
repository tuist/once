//! `once mcp` exposes Once's graph queries over the Model Context
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
//! Test invocation is intentionally available as a first write-capable
//! tool because coding harnesses need a short discover, filter, run,
//! inspect loop after editing files.
//!
//! Target execution is opt-in because it writes outputs and may trigger rule
//! side effects. When enabled, tools can build, run, or start persisted runtime
//! sessions and return session ids agents can use to query status, read logs,
//! or stop the process later.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP protocol version we negotiate.
const PROTOCOL_VERSION: &str = "2024-11-05";

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
            "once_query_tests" => self.tool_query_tests(),
            "once_query_affected_tests" => self.tool_query_affected_tests(&call.arguments),
            "once_run_tests" => self.tool_run_tests(&call.arguments),
            "once_query_test_results" => self.tool_query_test_results(&call.arguments),
            "once_build_target" if self.allow_run => self.tool_build_target(&call.arguments),
            "once_run_target" if self.allow_run => self.tool_run_target(&call.arguments),
            "once_start_target" if self.allow_run => self.tool_start_target(&call.arguments),
            "once_runtime_status" if self.allow_run => self.tool_runtime_status(&call.arguments),
            "once_runtime_logs" if self.allow_run => self.tool_runtime_logs(&call.arguments),
            "once_stop_runtime" if self.allow_run => self.tool_stop_runtime(&call.arguments),
            "once_build_target"
            | "once_run_target"
            | "once_start_target"
            | "once_runtime_status"
            | "once_runtime_logs"
            | "once_stop_runtime" => Err(anyhow::anyhow!(
                "execution tools require starting `once mcp --allow-run`"
            )),
            "once_apply_edit" => self.tool_apply_edit(&call.arguments),
            "once_query_schema" => tool_query_schema(&self.workspace, &call.arguments),
            "once_list_rules" => tool_list_rules(&self.workspace),
            "once_validate_target" => tool_validate_target(&self.workspace, &call.arguments),
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

    fn tool_query_tests(&self) -> Result<Value> {
        crate::commands::query::tests_value(&self.workspace)
    }

    fn tool_query_affected_tests(&self, args: &Value) -> Result<Value> {
        let changed_paths = args
            .get("changed_paths")
            .and_then(Value::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        crate::commands::query::affected_tests_value(&self.workspace, &changed_paths)
    }

    fn tool_query_test_results(&self, args: &Value) -> Result<Value> {
        let target_id = args
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?;
        crate::commands::query::test_results_value(&self.workspace, target_id)
    }

    fn tool_run_tests(&self, args: &Value) -> Result<Value> {
        let targets = run_test_targets(&self.workspace, args)?;
        let mut runs = Vec::new();
        for target in targets {
            runs.push(run_test_target(&self.workspace, &target)?);
        }
        Ok(json!({ "runs": runs }))
    }

    fn tool_build_target(&self, args: &Value) -> Result<Value> {
        let args: TargetExecutionArgs = serde_json::from_value(tool_args(args))?;
        run_graph_target(&self.workspace, "build", &args.target)
    }

    fn tool_run_target(&self, args: &Value) -> Result<Value> {
        let args: TargetExecutionArgs = serde_json::from_value(tool_args(args))?;
        run_graph_target(&self.workspace, "run", &args.target)
    }

    fn tool_start_target(&self, args: &Value) -> Result<Value> {
        let args: StartTargetArgs = serde_json::from_value(tool_args(args))?;
        let target = once_frontend::normalize_cli_target(&self.workspace, &args.target)
            .context("resolving target argument")?;
        Ok(serde_json::to_value(
            crate::commands::runtime::start_session(&self.workspace, &target)?,
        )?)
    }

    fn tool_runtime_status(&self, args: &Value) -> Result<Value> {
        let args: RuntimeSessionArgs = serde_json::from_value(tool_args(args))?;
        Ok(serde_json::to_value(
            crate::commands::runtime::status_session(&self.workspace, &args.session_id)?,
        )?)
    }

    fn tool_runtime_logs(&self, args: &Value) -> Result<Value> {
        let args: RuntimeLogsArgs = serde_json::from_value(tool_args(args))?;
        Ok(serde_json::to_value(
            crate::commands::runtime::logs_session(
                &self.workspace,
                &args.session_id,
                args.source.as_deref(),
                args.cursor.as_deref(),
                args.limit,
            )?,
        )?)
    }

    fn tool_stop_runtime(&self, args: &Value) -> Result<Value> {
        let args: RuntimeSessionArgs = serde_json::from_value(tool_args(args))?;
        Ok(serde_json::to_value(
            crate::commands::runtime::stop_session(&self.workspace, &args.session_id)?,
        )?)
    }
}

fn run_test_targets(workspace: &std::path::Path, args: &Value) -> Result<Vec<String>> {
    let mut targets = Vec::new();
    if let Some(target) = args.get("target").and_then(Value::as_str) {
        targets.push(target.to_string());
    }
    if let Some(raw_targets) = args.get("targets").and_then(Value::as_array) {
        targets.extend(
            raw_targets
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    if !targets.is_empty() {
        targets.sort();
        targets.dedup();
        validate_test_targets(workspace, &targets)?;
        return Ok(targets);
    }

    let changed_paths = args
        .get("changed_paths")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let affected = crate::commands::query::affected_tests_value(workspace, &changed_paths)?;
    let mut targets = affected
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        anyhow::bail!("no test targets matched the requested inputs");
    }
    validate_test_targets(workspace, &targets)?;
    Ok(targets)
}

fn validate_test_targets(workspace: &std::path::Path, targets: &[String]) -> Result<()> {
    let graph = once_frontend::load_graph_workspace(workspace).context("loading graph")?;
    for target_id in targets {
        crate::commands::query::target_id_path(target_id)?;
        let target = graph
            .iter()
            .find(|target| target.label.id == *target_id)
            .with_context(|| format!("no target matches `{target_id}`"))?;
        if !target
            .capabilities
            .iter()
            .any(|capability| capability.name == "test")
        {
            anyhow::bail!("target `{target_id}` does not expose the test capability");
        }
    }
    Ok(())
}

fn run_test_target(workspace: &std::path::Path, target: &str) -> Result<Value> {
    let exe = std::env::current_exe().context("resolving current once executable")?;
    let output = std::process::Command::new(&exe)
        .arg("-C")
        .arg(workspace)
        .arg("--format")
        .arg("json")
        .arg("test")
        .arg(target)
        .output()
        .with_context(|| format!("running `{}` test `{target}`", exe.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let (record, record_parse_error) = parse_json_record(&stdout);
    let results = crate::commands::query::test_results_value(workspace, target).ok();
    Ok(json!({
        "target": target,
        "exit_code": output.status.code().unwrap_or(-1),
        "success": output.status.success(),
        "record": record,
        "record_parse_error": record_parse_error,
        "results": results,
        "stderr": stderr,
    }))
}

fn run_graph_target(workspace: &std::path::Path, capability: &str, target: &str) -> Result<Value> {
    let exe = std::env::current_exe().context("resolving current once executable")?;
    run_graph_target_with_exe(&exe, workspace, capability, target)
}

fn run_graph_target_with_exe(
    exe: &std::path::Path,
    workspace: &std::path::Path,
    capability: &str,
    target: &str,
) -> Result<Value> {
    let output = std::process::Command::new(exe)
        .arg("-C")
        .arg(workspace)
        .arg("--format")
        .arg("json")
        .arg(capability)
        .arg(target)
        .output()
        .with_context(|| format!("running `{}` {capability} `{target}`", exe.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let (record, record_parse_error) = parse_json_record(&stdout);
    Ok(json!({
        "target": target,
        "capability": capability,
        "exit_code": output.status.code().unwrap_or(-1),
        "success": output.status.success(),
        "record": record,
        "record_parse_error": record_parse_error,
        "stderr": stderr,
    }))
}

fn parse_json_record(stdout: &str) -> (Value, Option<String>) {
    if stdout.is_empty() {
        return (Value::Null, None);
    }
    match serde_json::from_str(stdout) {
        Ok(value) => (value, None),
        Err(err) => (Value::String(stdout.to_string()), Some(err.to_string())),
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
struct TargetExecutionArgs {
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartTargetArgs {
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeSessionArgs {
    session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeLogsArgs {
    session_id: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn tool_query_schema(workspace: &Path, args: &Value) -> Result<Value> {
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `kind` argument"))?;
    let schema = once_frontend::rule_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no rule schema matches `{kind}`"))?;
    Ok(serde_json::to_value(schema)?)
}

fn tool_list_rules(workspace: &Path) -> Result<Value> {
    let schemas = once_frontend::rule_schemas_for_workspace(workspace)?;
    let summaries: Vec<RuleSummary> = schemas.into_iter().map(RuleSummary::from).collect();
    Ok(serde_json::to_value(summaries)?)
}

fn tool_validate_target(workspace: &Path, args: &Value) -> Result<Value> {
    let raw_target = args
        .get("target")
        .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?
        .clone();
    let spec: once_frontend::TargetSpec = serde_json::from_value(raw_target)
        .map_err(|err| anyhow::anyhow!("invalid `target`: {err}"))?;
    let schemas = once_frontend::rule_schemas_for_workspace(workspace)?;
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
    let schemas =
        once_frontend::rule_schemas_for_workspace(workspace).context("loading rule schemas")?;
    match once_frontend::apply_operations_with_schemas(&existing, operations, &schemas) {
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
    // The MCP `tools/list` reply is the wire projection of the
    // shared catalog; the doc generator walks the same catalog so
    // the reference page can't drift from the server's advertised
    // surface.
    tool_catalog()
        .into_iter()
        .filter(|tool| allow_run || !is_run_gated_tool(tool.name))
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect()
}

fn is_run_gated_tool(name: &str) -> bool {
    matches!(
        name,
        "once_build_target"
            | "once_run_target"
            | "once_start_target"
            | "once_runtime_status"
            | "once_runtime_logs"
            | "once_stop_runtime"
    )
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
            example_return: "{\n  \"id\": \"apps/ios/App\",\n  \"kind\": \"apple_application\",\n  \"capabilities\": [\n    { \"name\": \"build\", \"output_groups\": [\"default\", \"bundle\", \"dsyms\"],\n      \"requires_outputs\": [] },\n    { \"name\": \"run\", \"output_groups\": [\"default\"],\n      \"requires_outputs\": [\"bundle\"] }\n  ]\n}",
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
            description: "List every rule kind available in the workspace, with its one-line docs and example slugs.",
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
            name: "once_query_tests",
            description: "List targets that expose Once's generic test capability.",
            long_description: "Returns every target with a `test` capability, including its rule kind, dependencies, runner type when the rule exposes `once_test_info`, labels, and normalized result path. Use this as the agent-native test discovery entry point before running or filtering tests.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            example_return: "[\n  {\n    \"id\": \"spec/cli_e2e\",\n    \"kind\": \"shellspec_test\",\n    \"deps\": [],\n    \"runner\": \"shellspec\",\n    \"labels\": [\"e2e\"],\n    \"results_path\": \".once/out/spec/cli_e2e/test/test_results.json\"\n  }\n]",
        },
        ToolDefinition {
            name: "once_query_affected_tests",
            description: "Return test targets likely affected by a set of changed workspace paths.",
            long_description: "Maps changed paths to test targets using generic graph relationships and declared inputs. A test is affected when a changed path belongs to the test target itself or to one of its declared dependencies. The query does not know about ShellSpec, Python, Android, or any native runner.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "changed_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workspace-relative changed paths. An empty list returns every test target."
                    }
                }
            }),
            example_return: "[\n  {\n    \"id\": \"spec/cli_e2e\",\n    \"kind\": \"shellspec_test\",\n    \"reasons\": [\"changed test input `spec/cli_spec.sh`\"]\n  }\n]",
        },
        ToolDefinition {
            name: "once_run_tests",
            description: "Run test targets by id, or run tests affected by changed workspace paths.",
            long_description: "Executes Once's generic `test` capability for either explicit `target` / `targets` or the targets selected by `changed_paths`. This is the MCP-native edit verification loop for coding harnesses: call `once_query_affected_tests` to preview selection, call `once_run_tests` to execute, then read the normalized `once.test_results.v1` results included in each run record. Failed tests are returned as normal tool content with `success: false` rather than a tool protocol error, so agents can inspect failures and iterate.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Single canonical target id to run, e.g. `spec/cli_e2e`."
                    },
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Canonical target ids to run. Used with `target`, this is deduplicated before execution."
                    },
                    "changed_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workspace-relative changed paths. Used only when no explicit target is supplied; an empty list runs every discovered test target."
                    }
                }
            }),
            example_return: "{\n  \"runs\": [\n    {\n      \"target\": \"spec/cli_e2e\",\n      \"exit_code\": 0,\n      \"success\": true,\n      \"record\": { \"target\": \"spec/cli_e2e\", \"capability\": \"test\" },\n      \"results\": { \"schema\": \"once.test_results.v1\", \"status\": \"passed\" },\n      \"stderr\": \"\"\n    }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_query_test_results",
            description: "Read normalized once.test_results.v1 results for a target.",
            long_description: "Reads the normalized result file produced by the target's `test` capability. This is the stable agent-facing interface for pass/fail summaries, case-level failures, attempts, and artifacts; callers should not scrape native runner stdout or stderr.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Canonical target id, e.g. `spec/cli_e2e`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"schema\": \"once.test_results.v1\",\n  \"target\": \"spec/cli_e2e\",\n  \"status\": \"passed\",\n  \"summary\": { \"total\": 2, \"passed\": 2, \"failed\": 0 },\n  \"cases\": []\n}",
        },
        ToolDefinition {
            name: "once_build_target",
            description: "Build a target by running its generic `build` capability.",
            long_description: "Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Executes the same path as `once build <target> --format json`, so dependency traversal, rule-declared actions, cache policy, and output groups stay owned by the CLI and rule graph. The tool returns stdout parsed as JSON when possible, along with exit status and stderr. A failed build is returned as normal tool content with `success: false` so agents can inspect diagnostics.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Target id to build, e.g. `apps/ios/App` or `./App`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"target\": \"apps/ios/App\",\n  \"capability\": \"build\",\n  \"exit_code\": 0,\n  \"success\": true,\n  \"record\": {\n    \"target\": \"apps/ios/App\",\n    \"kind\": \"apple_application\",\n    \"capability\": \"build\",\n    \"cache\": \"miss\",\n    \"outputs\": [\".once/out/apps/ios/App/App.app\"]\n  },\n  \"stderr\": \"\"\n}",
        },
        ToolDefinition {
            name: "once_run_target",
            description: "Run a target by executing its generic `run` capability.",
            long_description: "Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Executes the same path as `once run <target> --format json`, including any prerequisite build outputs declared by the target's `run` capability. Rule-declared execution policy is preserved, so uncacheable actions are executed instead of replayed from the action cache. The tool returns stdout parsed as JSON when possible, plus exit status and stderr.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Target id to run, e.g. `apps/ios/App` or `./App`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"target\": \"apps/ios/App\",\n  \"capability\": \"run\",\n  \"exit_code\": 0,\n  \"success\": true,\n  \"record\": {\n    \"target\": \"apps/ios/App\",\n    \"kind\": \"apple_application\",\n    \"capability\": \"run\",\n    \"cache\": \"bypass\",\n    \"outputs\": [\".once/out/apps/ios/App/run/run.json\"]\n  },\n  \"stderr\": \"\"\n}",
        },
        ToolDefinition {
            name: "once_start_target",
            description: "Start a target in a persisted runtime session and return its session id.",
            long_description: "Opt-in tool exposed only when the MCP server starts with `once mcp --allow-run`. Starts `once run` under a runtime supervisor, persists stdout and stderr under `.once/runtime/<session_id>/`, and returns immediately with the session record. Use the runtime status, logs, and stop tools to follow the process.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Target id to start, e.g. `tools/demo/LaunchApp` or `./LaunchApp`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchApp-123-1812345678901\",\n  \"target\": \"tools/demo/LaunchApp\",\n  \"status\": \"starting\",\n  \"session_dir\": \".once/runtime/tools-demo-LaunchApp-123-1812345678901\",\n  \"stdout\": \".once/runtime/tools-demo-LaunchApp-123-1812345678901/stdout.log\",\n  \"stderr\": \".once/runtime/tools-demo-LaunchApp-123-1812345678901/stderr.log\"\n}",
        },
        ToolDefinition {
            name: "once_runtime_status",
            description: "Return the latest persisted status for a runtime session.",
            long_description: "Reads `.once/runtime/<session_id>/session.json` and returns the supervisor's latest status. Status values include `starting`, `running`, `stopping`, `stopped`, `exited`, and `failed`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session id returned by `once_start_target`."
                    }
                },
                "required": ["session_id"]
            }),
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchApp-123-1812345678901\",\n  \"target\": \"tools/demo/LaunchApp\",\n  \"status\": \"running\",\n  \"pid\": 4242\n}",
        },
        ToolDefinition {
            name: "once_runtime_logs",
            description: "Read stdout or stderr records for a runtime session.",
            long_description: "Reads persisted line-oriented stdout and stderr records from a runtime session. Pass `source` to restrict to `stdout` or `stderr`, and pass a previous `cursor` to read only newer records.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session id returned by `once_start_target`."
                    },
                    "source": {
                        "type": "string",
                        "enum": ["stdout", "stderr"],
                        "description": "`stdout` or `stderr`."
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Cursor returned by a previous log record."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of records to return."
                    }
                },
                "required": ["session_id"]
            }),
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchApp-123-1812345678901\",\n  \"records\": [\n    { \"cursor\": \"stdout:000000000000\", \"source\": \"stdout\", \"level\": \"info\", \"message\": \"ready\" }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_stop_runtime",
            description: "Request that a runtime session stop.",
            long_description: "Writes a stop request into the runtime session directory. The supervisor observes the request, kills the child process, and updates `session.json` to `stopped`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Session id returned by `once_start_target`."
                    }
                },
                "required": ["session_id"]
            }),
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchApp-123-1812345678901\",\n  \"target\": \"tools/demo/LaunchApp\",\n  \"status\": \"stopping\"\n}",
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

    fn seed_custom_rule_workspace(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("rules")).unwrap();
        std::fs::write(
            root.join("once.toml"),
            "[rules]\npaths = [\"rules/*.star\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("rules/demo.star"),
            r#"
demo_rule = rule(
    docs = "Demo rule",
    attrs = [
        attr("message", "string", required = True, docs = "Message to emit"),
    ],
    deps = [],
    providers = ["demo_provider"],
    capabilities = [
        capability("build", ["default"]),
    ],
)
"#,
        )
        .unwrap();
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
    fn tools_list_advertises_the_full_tool_surface() {
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
                "once_query_tests".to_string(),
                "once_query_affected_tests".to_string(),
                "once_run_tests".to_string(),
                "once_query_test_results".to_string(),
                "once_validate_target".to_string(),
                "once_apply_edit".to_string(),
            ]
        );
    }

    #[test]
    fn tools_list_advertises_runtime_tools_when_allowed() {
        let tmp = TempDir::new().unwrap();
        let response =
            run_server(tmp.path().to_path_buf()).dispatch(request("tools/list", json!({})));
        let names: Vec<String> = response.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"once_build_target".to_string()));
        assert!(names.contains(&"once_run_target".to_string()));
        assert!(names.contains(&"once_start_target".to_string()));
        assert!(names.contains(&"once_runtime_status".to_string()));
        assert!(names.contains(&"once_runtime_logs".to_string()));
        assert!(names.contains(&"once_stop_runtime".to_string()));
    }

    #[test]
    fn execution_tools_require_allow_run() {
        let tmp = TempDir::new().unwrap();
        for tool in ["once_build_target", "once_run_target", "once_start_target"] {
            let response = server(tmp.path().to_path_buf()).dispatch(request(
                "tools/call",
                json!({ "name": tool, "arguments": { "target": "App" } }),
            ));
            let result = response.result.expect("result");

            assert_eq!(result["isError"], true);
            assert!(result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("--allow-run"));
        }
    }

    #[test]
    fn run_test_targets_prefers_explicit_deduplicated_targets() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("spec")).unwrap();
        std::fs::write(
            tmp.path().join("spec/once.toml"),
            r#"[[target]]
name = "all"
kind = "shellspec_test"
srcs = ["all_spec.sh"]

[[target]]
name = "other"
kind = "shellspec_test"
srcs = ["other_spec.sh"]
"#,
        )
        .unwrap();
        let targets = run_test_targets(
            tmp.path(),
            &json!({
                "target": "spec/all",
                "targets": ["spec/all", "spec/other"],
                "changed_paths": ["src/lib.rs"]
            }),
        )
        .unwrap();
        assert_eq!(targets, vec!["spec/all", "spec/other"]);
    }

    #[test]
    fn parse_json_record_preserves_error_context() {
        let (record, error) = parse_json_record("{not json");

        assert_eq!(record, Value::String("{not json".to_string()));
        assert!(error.unwrap().contains("line"));
    }

    #[cfg(unix)]
    fn shell_literal(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    #[cfg(unix)]
    #[test]
    fn graph_target_runner_invokes_cli_and_parses_json_record() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let exe = tmp.path().join("once-mock");
        let args_path = tmp.path().join("args.txt");
        let args_file = shell_literal(args_path.to_str().unwrap());
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {args_file}\nprintf '{{\"target\":\"%s\",\"capability\":\"%s\",\"status\":\"completed\"}}\\n' \"$6\" \"$5\"\n",
        );
        std::fs::write(&exe, script).unwrap();
        let mut permissions = std::fs::metadata(&exe).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&exe, permissions).unwrap();

        let build = run_graph_target_with_exe(&exe, tmp.path(), "build", "apps/App").unwrap();
        assert_eq!(build["capability"], "build");
        assert_eq!(build["success"], true);
        assert_eq!(build["record"]["target"], "apps/App");
        assert_eq!(build["record"]["capability"], "build");

        let run = run_graph_target_with_exe(&exe, tmp.path(), "run", "apps/App").unwrap();
        assert_eq!(run["capability"], "run");
        assert_eq!(run["success"], true);
        assert_eq!(run["record"]["target"], "apps/App");
        assert_eq!(run["record"]["capability"], "run");

        let args = std::fs::read_to_string(args_path).unwrap();
        assert!(args.contains("--format\njson\nrun\napps/App\n"));
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
        let value = tool_query_schema(tmp.path(), &json!({ "kind": "apple_library" })).unwrap();
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
        let tmp = TempDir::new().unwrap();
        let value = tool_query_schema(tmp.path(), &json!({ "kind": "apple_library" })).unwrap();
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
        let tmp = TempDir::new().unwrap();
        let value = tool_list_rules(tmp.path()).unwrap();
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
    fn mcp_tools_use_workspace_custom_rules() {
        let tmp = TempDir::new().unwrap();
        seed_custom_rule_workspace(tmp.path());

        let schema = tool_query_schema(tmp.path(), &json!({ "kind": "demo_rule" })).unwrap();
        assert_eq!(schema["kind"], "demo_rule");
        assert_eq!(schema["providers"], json!(["demo_provider"]));

        let rules = tool_list_rules(tmp.path()).unwrap();
        assert!(rules
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["kind"] == "demo_rule"));

        let validation = tool_validate_target(
            tmp.path(),
            &json!({
                "target": {
                    "name": "Hello",
                    "kind": "demo_rule",
                    "attrs": { "message": "hello" }
                }
            }),
        )
        .unwrap();
        assert_eq!(validation["valid"], true);

        let result = apply_edit_to_package(
            tmp.path(),
            "apps/Hello",
            &[once_frontend::EditOperation::Create {
                target: once_frontend::TargetSpec {
                    name: "Hello".to_string(),
                    kind: "demo_rule".to_string(),
                    attrs: serde_json::Map::from_iter([("message".to_string(), json!("hello"))]),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        assert_eq!(result["applied"], true);
    }

    #[test]
    fn validate_target_returns_valid_for_minimal_apple_library() {
        let tmp = TempDir::new().unwrap();
        let value = tool_validate_target(
            tmp.path(),
            &json!({
                "target": {
                    "name": "Hello",
                    "kind": "apple_library",
                    "attrs": { "platform": "ios" }
                }
            }),
        )
        .unwrap();
        assert_eq!(value["valid"], true);
    }

    #[test]
    fn validate_target_returns_structured_diagnostics_on_failure() {
        let tmp = TempDir::new().unwrap();
        let value = tool_validate_target(
            tmp.path(),
            &json!({
                "target": {
                    "name": "Hello",
                    "kind": "apple_library"
                }
            }),
        )
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
