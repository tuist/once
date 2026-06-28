//! `once mcp` exposes Once's graph and memory queries over the Model Context
//! Protocol so a coding agent can call `query_targets`,
//! `query_capabilities`, `query_schema`, and `query_evidence` as MCP tools.
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
//! Target execution is opt-in because it writes outputs and may trigger target
//! kind side effects. When enabled, tools can build, run, or start persisted
//! runtime sessions and return session ids agents can use to query status, read
//! logs, or stop the process later.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod tools;
pub(crate) use tools::tool_catalog;
use tools::tool_definitions;

/// MCP protocol version we negotiate.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Run the MCP server until stdin closes.
pub async fn serve(workspace: PathBuf, allow_run: bool) -> Result<()> {
    tokio::task::spawn_blocking(move || serve_blocking(workspace, allow_run))
        .await
        .context("joining MCP server thread")?
}

fn serve_blocking(workspace: PathBuf, allow_run: bool) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    let server = Server::new(workspace, allow_run);

    for line in stdin.lock().lines() {
        let line = line.context("reading MCP request")?;
        if line.trim().is_empty() {
            continue;
        }
        match server.dispatch_line(&line) {
            DispatchOutcome::Reply(response) => {
                let bytes = serde_json::to_vec(&response).context("encoding MCP response")?;
                stdout.write_all(&bytes)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
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
            "once_query_evidence" => self.tool_query_evidence(&call.arguments),
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
            "once_query_example" => tool_query_example(&self.workspace, &call.arguments),
            "once_list_target_kinds" | "once_list_rules" => tool_list_target_kinds(&self.workspace),
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

    fn tool_query_evidence(&self, args: &Value) -> Result<Value> {
        let subject = match args.get("subject") {
            Some(Value::Null) | None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("`subject` must be a string"))?
                    .to_string(),
            ),
        };
        let workspace = self.workspace.clone();
        let records = run_async_result(async move {
            crate::commands::query::evidence_records(&workspace, subject.as_deref()).await
        })?;
        Ok(serde_json::to_value(records)?)
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
        run_graph_target(&self.workspace, "build", &args.target, false)
    }

    fn tool_run_target(&self, args: &Value) -> Result<Value> {
        let args: RunTargetArgs = serde_json::from_value(tool_args(args))?;
        run_graph_target(&self.workspace, "run", &args.target, args.visible)
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

fn run_graph_target(
    workspace: &std::path::Path,
    capability: &str,
    target: &str,
    visible: bool,
) -> Result<Value> {
    let exe = std::env::current_exe().context("resolving current once executable")?;
    run_graph_target_with_exe(&exe, workspace, capability, target, visible)
}

fn run_graph_target_with_exe(
    exe: &std::path::Path,
    workspace: &std::path::Path,
    capability: &str,
    target: &str,
    visible: bool,
) -> Result<Value> {
    let mut command = std::process::Command::new(exe);
    command
        .arg("-C")
        .arg(workspace)
        .arg("--format")
        .arg("json")
        .arg(capability);
    if visible {
        command.arg("--visible");
    }
    let output = command
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
struct RunTargetArgs {
    target: String,
    #[serde(default)]
    visible: bool,
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
    let schema = once_frontend::target_kind_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no target kind schema matches `{kind}`"))?;
    Ok(serde_json::to_value(schema)?)
}

fn tool_query_example(workspace: &Path, args: &Value) -> Result<Value> {
    let kind = required_string_arg(args, "kind")?;
    let slug = required_string_arg(args, "slug")?;
    let schema = once_frontend::target_kind_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no target kind schema matches `{kind}`"))?;
    let example = once_frontend::load_target_kind_example(&schema, slug)?;
    Ok(serde_json::to_value(example)?)
}

fn required_string_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    let Some(value) = args.get(name) else {
        return Err(anyhow::anyhow!("missing `{name}` argument"));
    };
    value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("`{name}` must be a string"))
}

fn tool_list_target_kinds(workspace: &Path) -> Result<Value> {
    let schemas = once_frontend::target_kind_schemas_for_workspace(workspace)?;
    let summaries: Vec<TargetKindSummary> =
        schemas.into_iter().map(TargetKindSummary::from).collect();
    Ok(serde_json::to_value(summaries)?)
}

fn tool_validate_target(workspace: &Path, args: &Value) -> Result<Value> {
    let raw_target = args
        .get("target")
        .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?
        .clone();
    let spec: once_frontend::TargetSpec = serde_json::from_value(raw_target)
        .map_err(|err| anyhow::anyhow!("invalid `target`: {err}"))?;
    let schemas = once_frontend::target_kind_schemas_for_workspace(workspace)?;
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
    let schemas = once_frontend::target_kind_schemas_for_workspace(workspace)
        .context("loading target kind schemas")?;
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
struct TargetKindSummary {
    kind: String,
    docs: String,
    examples: Vec<TargetKindExampleSummary>,
}

#[derive(Debug, Serialize)]
struct TargetKindExampleSummary {
    slug: String,
    name: String,
    use_when: String,
}

impl From<once_frontend::TargetKindSchema> for TargetKindSummary {
    fn from(schema: once_frontend::TargetKindSchema) -> Self {
        Self {
            kind: schema.kind,
            docs: schema.docs,
            examples: schema
                .examples
                .into_iter()
                .map(|example| TargetKindExampleSummary {
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

fn run_async_result<T, F>(future: F) -> Result<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T>> + Send + 'static,
{
    let run = move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("creating async runtime for MCP tool")?;
        runtime.block_on(future)
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        match std::thread::spawn(run).join() {
            Ok(result) => result,
            Err(payload) => Err(anyhow::anyhow!(
                "MCP async tool panicked: {}",
                panic_payload_message(payload.as_ref())
            )),
        }
    } else {
        run()
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
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
    use once_cas::{ActionResult, Digest};
    use once_core::{EvidenceCacheState, EvidenceRecord, EvidenceStore, EvidenceSubject};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn server(workspace: PathBuf) -> Server {
        Server::new(workspace, false)
    }

    fn run_server(workspace: PathBuf) -> Server {
        Server::new(workspace, true)
    }

    fn seed_custom_module_workspace(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("modules")).unwrap();
        std::fs::write(
            root.join("once.toml"),
            "[modules]\npaths = [\"modules/*.star\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("modules/demo.star"),
            r#"
demo_kind = target_kind(
    docs = "Demo kind",
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
                "once_query_example".to_string(),
                "once_list_target_kinds".to_string(),
                "once_get_target".to_string(),
                "once_query_tests".to_string(),
                "once_query_affected_tests".to_string(),
                "once_run_tests".to_string(),
                "once_query_test_results".to_string(),
                "once_query_evidence".to_string(),
                "once_validate_target".to_string(),
                "once_apply_edit".to_string(),
            ]
        );
    }

    #[test]
    fn query_evidence_returns_filtered_records() {
        let tmp = TempDir::new().unwrap();
        let record = EvidenceRecord::from_action_result(
            EvidenceSubject::target("cli", "test"),
            Digest::of_bytes(b"action"),
            Some(Digest::of_bytes(b"input")),
            EvidenceCacheState::Miss,
            &ActionResult {
                exit_code: 0,
                stdout: Some(Digest::of_bytes(b"stdout")),
                stderr: None,
                outputs: BTreeMap::default(),
            },
        )
        .unwrap();
        run_async_result({
            let store = EvidenceStore::open_workspace(tmp.path());
            let record = record.clone();
            async move { store.append(&record).await }
        })
        .unwrap();

        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({
                "name": "once_query_evidence",
                "arguments": { "subject": "cli:test" }
            }),
        ));

        assert!(response.error.is_none());
        let result = response.result.expect("result");
        let text = result["content"][0]["text"].as_str().expect("text content");
        let records: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["schema"], "once.evidence.v1");
        assert_eq!(records[0]["subject"]["id"], "cli");
        assert_eq!(records[0]["subject"]["capability"], "test");
        assert_eq!(records[0]["status"], "passed");
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
    fn tools_call_accepts_legacy_list_rules_alias() {
        let tmp = TempDir::new().unwrap();
        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({
                "name": "once_list_rules",
                "arguments": {}
            }),
        ));

        assert!(response.error.is_none());
        let result = response.result.expect("result");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(text.contains("apple_library"));
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
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {args_file}\ncapability=''\ntarget=''\nfor arg do\n  case \"$arg\" in build|run|test) capability=\"$arg\" ;;\n  *) target=\"$arg\" ;;\n  esac\ndone\nprintf '{{\"target\":\"%s\",\"capability\":\"%s\",\"status\":\"completed\"}}\\n' \"$target\" \"$capability\"\n",
        );
        std::fs::write(&exe, script).unwrap();
        let mut permissions = std::fs::metadata(&exe).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&exe, permissions).unwrap();

        let build =
            run_graph_target_with_exe(&exe, tmp.path(), "build", "apps/App", false).unwrap();
        assert_eq!(build["capability"], "build");
        assert_eq!(build["success"], true);
        assert_eq!(build["record"]["target"], "apps/App");
        assert_eq!(build["record"]["capability"], "build");

        let run = run_graph_target_with_exe(&exe, tmp.path(), "run", "apps/App", true).unwrap();
        assert_eq!(run["capability"], "run");
        assert_eq!(run["success"], true);
        assert_eq!(run["record"]["target"], "apps/App");
        assert_eq!(run["record"]["capability"], "run");

        let args = std::fs::read_to_string(args_path).unwrap();
        assert!(args.contains("--format\njson\nrun\n--visible\napps/App\n"));
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
    fn query_schema_returns_example_descriptors() {
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
        assert!(minimal.get("files").is_none());
    }

    #[test]
    fn query_example_returns_materialized_files() {
        let tmp = TempDir::new().unwrap();
        let value = tool_query_example(
            tmp.path(),
            &json!({ "kind": "apple_library", "slug": "apple-library-minimal" }),
        )
        .unwrap();
        assert_eq!(value["slug"], "apple-library-minimal");
        assert!(!value["name"].as_str().unwrap().is_empty());
        assert!(!value["use_when"].as_str().unwrap().is_empty());
        let files = value["files"].as_array().expect("files is an array");
        assert!(files
            .iter()
            .any(|f| f["path"] == "apps/Hello/once.toml"
                && !f["contents"].as_str().unwrap().is_empty()));
    }

    #[test]
    fn query_example_reports_non_string_arguments() {
        let tmp = TempDir::new().unwrap();
        let kind_err = tool_query_example(
            tmp.path(),
            &json!({ "kind": 7, "slug": "apple-library-minimal" }),
        )
        .unwrap_err();
        assert!(kind_err.to_string().contains("`kind` must be a string"));

        let slug_err = tool_query_example(
            tmp.path(),
            &json!({ "kind": "apple_library", "slug": false }),
        )
        .unwrap_err();
        assert!(slug_err.to_string().contains("`slug` must be a string"));
    }

    #[test]
    fn list_target_kinds_includes_every_known_target_kind() {
        let tmp = TempDir::new().unwrap();
        let value = tool_list_target_kinds(tmp.path()).unwrap();
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
    fn mcp_tools_use_workspace_custom_modules() {
        let tmp = TempDir::new().unwrap();
        seed_custom_module_workspace(tmp.path());

        let schema = tool_query_schema(tmp.path(), &json!({ "kind": "demo_kind" })).unwrap();
        assert_eq!(schema["kind"], "demo_kind");
        assert_eq!(schema["providers"], json!(["demo_provider"]));

        let target_kinds = tool_list_target_kinds(tmp.path()).unwrap();
        assert!(target_kinds
            .as_array()
            .unwrap()
            .iter()
            .any(|kind| kind["kind"] == "demo_kind"));

        let validation = tool_validate_target(
            tmp.path(),
            &json!({
                "target": {
                    "name": "Hello",
                    "kind": "demo_kind",
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
                    kind: "demo_kind".to_string(),
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
