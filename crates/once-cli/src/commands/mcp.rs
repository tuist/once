//! `once mcp` exposes Once's graph and memory queries over the Model Context
//! Protocol so a coding agent can call `query_targets`,
//! `query_capabilities`, `query_schema`, and `query_evidence` as MCP tools.
//!
//! Transport is newline-delimited JSON over stdio: every request is
//! one line of JSON-RPC 2.0 in, every response is one line out. The
//! handshake negotiates a supported protocol version: the client sends
//! `initialize`, we reply with server instructions and the `tools`
//! capability, the client sends `notifications/initialized` and then
//! `tools/list` to discover what we can do, then `tools/call` for each tool.
//!
//! Editing and execution are opt-in because they write workspace state or may
//! trigger target-kind side effects. When enabled, tools can edit manifests,
//! build, test, run, or start persisted runtime sessions and return session ids
//! agents can use to query status, read logs, or stop the process later.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod script;
mod tools;
pub(crate) use tools::tool_catalog;
use tools::{tool_definitions, tool_requires_allow_run};

/// MCP protocol version we negotiate.
const PROTOCOL_VERSION: &str = "2025-11-25";
const SERVER_INSTRUCTIONS: &str = "Once is a self-describing build graph and cacheable automation server. For a new typed graph with no named upstream rule, start with once_list_target_kinds. Pass its optional query when the request names ecosystems, target-kind families, or intent; include every named native test runner in one query when adopting a mixed test repository, then call once_query_schema and once_query_example for each chosen kind. When the request already points to a specific external rule or plugin, skip the target-kind catalog: call once_query_module_contract, fetch its authoritative source with once_fetch_external_source, write a project-local module, and call once_validate_module. Model only the requested dependency closure. Record upstream source_references and the returned content digest when the source was not truncated, then re-fetch and compare the digest before future maintenance. Materialize target files, inspect canonical ids with once_query_targets, call once_validate_workspace, and use capability tools to build, run, or test. New test targets intentionally begin with one whole-target batch. Run each target completely once, then inspect next_plan in the completed once_run_tests response to see the automatic file or case batches established for later runs. The plan field always describes the work that just ran. Before change-scoped testing, inspect conservative selection, unmatched paths, and stable batches with once_query_test_plan; once_run_tests returns that execution plan, the refreshed next plan, a duration-informed dynamic schedule, and execution results. Pass one listed unit with one explicit target to plan or run an exact filtered test when the target kind declares filtering support. Use jobs only to cap local concurrency; it does not change plan or batch identity. For annotated automation, call once_validate_script before once_exec_script. Stateful tools require --allow-run. Evidence subjects are target ids with an optional capability suffix; script execution returns its evidence subject directly. once_validate_target checks one proposed table, once_validate_workspace checks the loaded graph, and successful execution remains the authoritative current check. Evidence is historical provenance, so do not treat an older record as proof that inputs are unchanged.";

fn negotiated_protocol_version(params: Option<&Value>) -> String {
    let requested = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str);
    match requested {
        Some(version @ ("2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25")) => {
            version.to_string()
        }
        _ => PROTOCOL_VERSION.to_string(),
    }
}

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
            "initialize" => {
                let protocol_version = negotiated_protocol_version(request.params.as_ref());
                JsonRpcResponse::ok(
                    id,
                    json!({
                        "protocolVersion": protocol_version,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "once-mcp", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": SERVER_INSTRUCTIONS,
                    }),
                )
            }
            "notifications/initialized" => JsonRpcResponse::ok(id, Value::Null),
            "ping" => JsonRpcResponse::ok(id, json!({})),
            "resources/list" => JsonRpcResponse::ok(id, json!({ "resources": [] })),
            "resources/templates/list" => {
                JsonRpcResponse::ok(id, json!({ "resourceTemplates": [] }))
            }
            "prompts/list" => JsonRpcResponse::ok(id, json!({ "prompts": [] })),
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
        if tool_requires_allow_run(&call.name) && !self.allow_run {
            return tool_error(
                id,
                "state-changing tools require starting `once mcp --allow-run`",
            );
        }
        let result = match call.name.as_str() {
            "once_query_targets" => self.tool_query_targets(&call.arguments),
            "once_query_capabilities" => self.tool_query_capabilities(&call.arguments),
            "once_get_target" => self.tool_get_target(&call.arguments),
            "once_query_tests" => self.tool_query_tests(),
            "once_query_affected_tests" => self.tool_query_affected_tests(&call.arguments),
            "once_query_test_plan" => self.tool_query_test_plan(&call.arguments),
            "once_run_tests" => self.tool_run_tests(&call.arguments),
            "once_query_test_results" => self.tool_query_test_results(&call.arguments),
            "once_query_test_manifest" => self.tool_query_test_manifest(&call.arguments),
            "once_query_test_attempts" => self.tool_query_test_attempts(&call.arguments),
            "once_query_evidence" => self.tool_query_evidence(&call.arguments),
            "once_query_module_contract" => crate::commands::query::module_contract_value(),
            "once_fetch_external_source" => Self::tool_fetch_external_source(&call.arguments),
            "once_validate_module" => self.tool_validate_module(&call.arguments),
            "once_validate_workspace" => {
                crate::commands::query::workspace_validation_value(&self.workspace)
            }
            "once_validate_script" => script::validate(&self.workspace, &call.arguments),
            "once_exec_script" => script::execute(&self.workspace, &call.arguments),
            "once_build_target" => self.tool_build_target(&call.arguments),
            "once_run_target" => self.tool_run_target(&call.arguments),
            "once_start_target" => self.tool_start_target(&call.arguments),
            "once_runtime_status" => self.tool_runtime_status(&call.arguments),
            "once_runtime_logs" => self.tool_runtime_logs(&call.arguments),
            "once_stop_runtime" => self.tool_stop_runtime(&call.arguments),
            "once_apply_edit" => self.tool_apply_edit(&call.arguments),
            "once_query_schema" => tool_query_schema(&self.workspace, &call.arguments),
            "once_query_example" => tool_query_example(&self.workspace, &call.arguments),
            "once_list_target_kinds" | "once_list_rules" => {
                tool_list_target_kinds(&self.workspace, &call.arguments)
            }
            "once_validate_target" => tool_validate_target(&self.workspace, &call.arguments),
            other => Err(anyhow::anyhow!("unknown tool `{other}`")),
        };
        match result {
            Ok(value) => JsonRpcResponse::ok(
                id,
                json!({
                    "content": [text_content(&value)],
                    "structuredContent": { "result": value },
                }),
            ),
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
        let args: ChangedPathsArgs = serde_json::from_value(tool_args(args))?;
        crate::commands::query::affected_tests_value(&self.workspace, &args.changed_paths)
    }

    fn tool_query_test_plan(&self, args: &Value) -> Result<Value> {
        let args: TestPlanQueryArgs = serde_json::from_value(tool_args(args))?;
        let plan = match (args.target.as_deref(), args.test_unit.as_deref()) {
            (Some(target), Some(test_unit)) => {
                crate::commands::query::explicit_test_unit_plan(&self.workspace, target, test_unit)?
            }
            (Some(target), None) => {
                crate::commands::query::explicit_test_plan(&self.workspace, &[target.to_string()])?
            }
            (None, None) => {
                crate::commands::query::test_plan_for_paths(&self.workspace, &args.changed_paths)?
            }
            (None, Some(_)) => anyhow::bail!("a test unit requires an explicit target"),
        };
        Ok(serde_json::to_value(plan)?)
    }

    fn tool_query_test_results(&self, args: &Value) -> Result<Value> {
        let target_id = args
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?;
        crate::commands::query::test_results_value(&self.workspace, target_id)
    }

    fn tool_query_test_manifest(&self, args: &Value) -> Result<Value> {
        let target_id = args
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?;
        crate::commands::query::test_manifest_value(&self.workspace, target_id)
    }

    fn tool_query_test_attempts(&self, args: &Value) -> Result<Value> {
        let args: TestAttemptQueryArgs = serde_json::from_value(tool_args(args))?;
        if !(1..=100).contains(&args.limit) {
            anyhow::bail!("`limit` must be between 1 and 100");
        }
        let workspace = self.workspace.clone();
        let records = run_async_result(async move {
            crate::commands::query::test_attempt_records(
                &workspace,
                args.target.as_deref(),
                Some(args.limit),
            )
            .await
        })?;
        Ok(serde_json::to_value(records)?)
    }

    fn tool_query_evidence(&self, args: &Value) -> Result<Value> {
        let args: EvidenceQueryArgs = serde_json::from_value(tool_args(args))?;
        if !(1..=100).contains(&args.limit) {
            anyhow::bail!("`limit` must be between 1 and 100");
        }
        let workspace = self.workspace.clone();
        let records = run_async_result(async move {
            crate::commands::query::evidence_records(
                &workspace,
                args.subject.as_deref(),
                Some(args.limit),
            )
            .await
        })?;
        Ok(serde_json::to_value(records)?)
    }

    fn tool_fetch_external_source(args: &Value) -> Result<Value> {
        let args: ExternalSourceArgs = serde_json::from_value(tool_args(args))?;
        crate::commands::query::external_source_value(&args.url, args.max_bytes)
    }

    fn tool_validate_module(&self, args: &Value) -> Result<Value> {
        let args: ValidateModuleArgs = serde_json::from_value(tool_args(args))?;
        crate::commands::query::module_validation_value(&self.workspace, &args.path)
    }

    fn tool_run_tests(&self, args: &Value) -> Result<Value> {
        let args: RunTestsArgs = serde_json::from_value(tool_args(args))?;
        let plan = run_test_plan_args(&self.workspace, &args)?;
        let workspace = self.workspace.clone();
        let workers = args.jobs;
        run_async_result(async move {
            let report = crate::commands::test_schedule::execute(
                &workspace,
                None,
                plan,
                workers,
                once_core::SandboxMode::Off,
            )
            .await?;
            Ok(serde_json::to_value(report)?)
        })
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

#[cfg(test)]
fn run_test_plan(
    workspace: &std::path::Path,
    args: &Value,
) -> Result<crate::commands::query::test_plan::TestPlan> {
    let args: RunTestsArgs = serde_json::from_value(tool_args(args))?;
    run_test_plan_args(workspace, &args)
}

fn run_test_plan_args(
    workspace: &std::path::Path,
    args: &RunTestsArgs,
) -> Result<crate::commands::query::test_plan::TestPlan> {
    let mut targets = args.targets.clone();
    if let Some(target) = &args.target {
        targets.push(target.clone());
    }
    if !targets.is_empty() {
        targets.sort();
        targets.dedup();
        validate_test_targets(workspace, &targets)?;
        if let Some(test_unit) = &args.test_unit {
            if targets.len() != 1 {
                anyhow::bail!("`test_unit` requires exactly one explicit test target");
            }
            return crate::commands::query::explicit_test_unit_plan(
                workspace,
                &targets[0],
                test_unit,
            );
        }
        return crate::commands::query::explicit_test_plan(workspace, &targets);
    }

    if args.test_unit.is_some() {
        anyhow::bail!("`test_unit` requires an explicit `target`");
    }

    let plan = crate::commands::query::test_plan_for_paths(workspace, &args.changed_paths)?;
    if plan.batches.is_empty() {
        anyhow::bail!("no test targets matched the requested inputs");
    }
    let targets = plan
        .batches
        .iter()
        .map(|batch| batch.target.clone())
        .collect::<Vec<_>>();
    validate_test_targets(workspace, &targets)?;
    Ok(plan)
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

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ChangedPathsArgs {
    #[serde(default)]
    changed_paths: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TestPlanQueryArgs {
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    test_unit: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct EvidenceQueryArgs {
    #[serde(default)]
    subject: Option<String>,
    #[serde(default = "default_evidence_limit")]
    limit: usize,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TestAttemptQueryArgs {
    #[serde(default)]
    target: Option<String>,
    #[serde(default = "default_test_attempt_limit")]
    limit: usize,
}

const fn default_test_attempt_limit() -> usize {
    20
}

const fn default_evidence_limit() -> usize {
    5
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalSourceArgs {
    url: String,
    #[serde(default = "default_external_source_max_bytes")]
    max_bytes: usize,
}

const fn default_external_source_max_bytes() -> usize {
    256 * 1024
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateModuleArgs {
    path: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TargetKindFilterArgs {
    #[serde(default)]
    query: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RunTestsArgs {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    jobs: Option<usize>,
    #[serde(default)]
    test_unit: Option<String>,
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

fn tool_list_target_kinds(workspace: &Path, args: &Value) -> Result<Value> {
    let args: TargetKindFilterArgs = serde_json::from_value(tool_args(args))?;
    let schemas =
        crate::commands::query::matching_target_kind_schemas(workspace, args.query.as_deref())?;
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    source_references: Vec<once_frontend::SourceReference>,
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
            source_references: schema.source_references,
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
        assert!(result["instructions"]
            .as_str()
            .unwrap()
            .contains("once_list_target_kinds"));
    }

    #[test]
    fn initialize_negotiates_a_supported_client_protocol_version() {
        let tmp = TempDir::new().unwrap();
        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "initialize",
            json!({ "protocolVersion": "2025-06-18" }),
        ));
        assert_eq!(response.result.unwrap()["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn optional_discovery_probes_return_empty_catalogs() {
        let tmp = TempDir::new().unwrap();
        let server = server(tmp.path().to_path_buf());

        assert_eq!(
            server
                .dispatch(request("resources/list", json!({})))
                .result
                .unwrap()["resources"],
            json!([])
        );
        assert_eq!(
            server
                .dispatch(request("resources/templates/list", json!({})))
                .result
                .unwrap()["resourceTemplates"],
            json!([])
        );
        assert_eq!(
            server
                .dispatch(request("prompts/list", json!({})))
                .result
                .unwrap()["prompts"],
            json!([])
        );
        assert_eq!(
            server.dispatch(request("ping", json!({}))).result.unwrap(),
            json!({})
        );
    }

    #[test]
    fn tools_list_advertises_the_full_tool_surface() {
        let tmp = TempDir::new().unwrap();
        let response = server(tmp.path().to_path_buf()).dispatch(request("tools/list", json!({})));
        let result = response.result.unwrap();
        let names: Vec<String> = result["tools"]
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
                "once_query_module_contract".to_string(),
                "once_fetch_external_source".to_string(),
                "once_validate_module".to_string(),
                "once_get_target".to_string(),
                "once_query_tests".to_string(),
                "once_query_affected_tests".to_string(),
                "once_query_test_plan".to_string(),
                "once_query_test_results".to_string(),
                "once_query_test_manifest".to_string(),
                "once_query_test_attempts".to_string(),
                "once_query_evidence".to_string(),
                "once_validate_script".to_string(),
                "once_validate_workspace".to_string(),
                "once_validate_target".to_string(),
            ]
        );
    }

    #[test]
    fn module_contract_and_validation_support_local_target_kind_authoring() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("modules")).unwrap();
        std::fs::write(
            tmp.path().join("modules/generated.star"),
            r#"
def _impl(ctx):
    out = declare_output(ctx["attr"]["output"])
    write_path(out, ctx["attr"]["content"])
    return {"generated_file": out}

generated_file = target_kind(
    docs = "Generate one file.",
    attrs = [
        attr("output", "string", required = True),
        attr("content", "string", required = True),
    ],
    providers = ["generated_file"],
    capabilities = [capability("build", ["default"])],
    source_references = [
        source_reference(
            "Example Build",
            "write_file",
            "https://example.com/write_file",
            "Replicate only requested generated files.",
        ),
    ],
    impl = _impl,
)
"#,
        )
        .unwrap();
        let server = server(tmp.path().to_path_buf());

        let contract = server.dispatch(request(
            "tools/call",
            json!({ "name": "once_query_module_contract", "arguments": {} }),
        ));
        let contract = contract.result.unwrap()["structuredContent"]["result"].clone();
        assert!(contract["declaration_source"]
            .as_str()
            .unwrap()
            .contains("def target_kind("));
        assert!(contract["action_primitives"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["signature"]
                .as_str()
                .unwrap()
                .starts_with("run_action(")));

        let validation = server.dispatch(request(
            "tools/call",
            json!({
                "name": "once_validate_module",
                "arguments": { "path": "modules/generated.star" }
            }),
        ));
        let validation = validation.result.unwrap()["structuredContent"]["result"].clone();
        assert_eq!(validation["valid"], true);
        assert_eq!(validation["target_kinds"][0]["kind"], "generated_file");
        assert_eq!(
            validation["target_kinds"][0]["source_references"][0]["symbol"],
            "write_file"
        );
    }

    #[test]
    fn external_source_fetch_rejects_non_https_addresses() {
        let tmp = TempDir::new().unwrap();
        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({
                "name": "once_fetch_external_source",
                "arguments": { "url": "http://example.com/rule" }
            }),
        ));
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("must use HTTPS"));
    }

    #[test]
    fn validate_workspace_returns_structured_graph_diagnostics() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("once.toml"),
            r#"[[target]]
name = "Build"
kind = "script"
srcs = ["missing.sh"]

[target.attrs]
script_path = "missing.sh"
script_runtime = "sh"
"#,
        )
        .unwrap();

        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({
                "name": "once_validate_workspace",
                "arguments": {}
            }),
        ));

        assert!(response.error.is_none());
        let result = response.result.expect("result");
        assert_eq!(result["structuredContent"]["result"]["valid"], false);
        assert_eq!(result["structuredContent"]["result"]["target_count"], 1);
        assert_eq!(
            result["structuredContent"]["result"]["diagnostics"][0]["code"],
            "missing_source"
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
        assert_eq!(
            result["structuredContent"]["result"][0]["subject"]["id"],
            "cli"
        );
    }

    #[test]
    fn query_evidence_bounds_the_default_history() {
        let tmp = TempDir::new().unwrap();
        let store = EvidenceStore::open_workspace(tmp.path());
        for index in 0..7 {
            let record = EvidenceRecord::from_action_result(
                EvidenceSubject::target("service", "build"),
                Digest::of_bytes(format!("action-{index}").as_bytes()),
                Some(Digest::of_bytes(format!("input-{index}").as_bytes())),
                EvidenceCacheState::Miss,
                &ActionResult {
                    exit_code: 0,
                    stdout: None,
                    stderr: None,
                    outputs: BTreeMap::default(),
                },
            )
            .unwrap();
            run_async_result({
                let store = store.clone();
                async move { store.append(&record).await }
            })
            .unwrap();
        }

        let default_response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({
                "name": "once_query_evidence",
                "arguments": { "subject": "service:build" }
            }),
        ));
        assert_eq!(
            default_response.result.unwrap()["structuredContent"]["result"]
                .as_array()
                .unwrap()
                .len(),
            5
        );

        let limited_response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({
                "name": "once_query_evidence",
                "arguments": { "subject": "service:build", "limit": 2 }
            }),
        ));
        assert_eq!(
            limited_response.result.unwrap()["structuredContent"]["result"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn tools_list_advertises_runtime_tools_when_allowed() {
        let tmp = TempDir::new().unwrap();
        let response =
            run_server(tmp.path().to_path_buf()).dispatch(request("tools/list", json!({})));
        let result = response.result.unwrap();
        let names: Vec<String> = result["tools"]
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
        assert!(names.contains(&"once_run_tests".to_string()));
        assert!(names.contains(&"once_apply_edit".to_string()));
        assert!(names.contains(&"once_exec_script".to_string()));

        for tool in result["tools"].as_array().unwrap() {
            assert!(tool["outputSchema"].is_object());
            assert!(tool["annotations"]["readOnlyHint"].is_boolean());
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
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
        for tool in [
            "once_build_target",
            "once_run_target",
            "once_start_target",
            "once_run_tests",
            "once_exec_script",
            "once_apply_edit",
        ] {
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
    fn run_test_plan_prefers_explicit_deduplicated_targets() {
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
        let plan = run_test_plan(
            tmp.path(),
            &json!({
                "target": "spec/all",
                "targets": ["spec/all", "spec/other"],
                "changed_paths": ["src/lib.rs"]
            }),
        )
        .unwrap();
        assert_eq!(
            plan.batches
                .iter()
                .map(|batch| batch.target.as_str())
                .collect::<Vec<_>>(),
            vec!["spec/all", "spec/other"]
        );
    }

    #[test]
    fn run_test_plan_rejects_non_string_targets() {
        let tmp = TempDir::new().unwrap();
        let error = run_test_plan(tmp.path(), &json!({ "targets": [42] })).unwrap_err();
        assert!(error.to_string().contains("invalid type"));
    }

    #[test]
    fn run_test_plan_rejects_explicit_units_for_unsupported_runners() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("once.toml"),
            r#"[[target]]
name = "unit"
kind = "shellspec_test"
srcs = ["unit_spec.sh"]
"#,
        )
        .unwrap();

        let error = run_test_plan(
            tmp.path(),
            &json!({
                "target": "unit",
                "test_unit": "unit::example"
            }),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "target `unit` does not support explicit test-unit filtering"
        );
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
    fn query_schema_returns_android_source_references() {
        let tmp = TempDir::new().unwrap();
        let value = tool_query_schema(tmp.path(), &json!({ "kind": "android_binary" })).unwrap();
        let references = value["source_references"].as_array().unwrap();
        assert!(references.iter().any(|reference| {
            reference["system"] == "Bazel rules_android" && reference["symbol"] == "android_binary"
        }));
        assert!(references.iter().any(|reference| {
            reference["system"] == "Buck2" && reference["symbol"] == "android_binary"
        }));
        assert!(references.iter().any(|reference| {
            reference["system"] == "Android Gradle plugin"
                && reference["symbol"] == "com.android.application"
        }));
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
        let value = tool_list_target_kinds(tmp.path(), &json!({})).unwrap();
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
    fn list_target_kinds_filters_by_ecosystem_intent() {
        let tmp = TempDir::new().unwrap();
        let value = tool_list_target_kinds(tmp.path(), &json!({ "query": "elixir" })).unwrap();
        let kinds = value
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(kinds, vec!["elixir_library", "elixir_test"]);
    }

    #[test]
    fn list_target_kinds_prioritizes_a_named_family_over_generic_intent_words() {
        let tmp = TempDir::new().unwrap();
        let value = tool_list_target_kinds(
            tmp.path(),
            &json!({ "query": "typed Rust library executable test" }),
        )
        .unwrap();
        let kinds = value
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert!(kinds.contains(&"rust_library"));
        assert!(kinds.contains(&"rust_binary"));
        assert!(kinds.contains(&"rust_test"));
        assert!(kinds.iter().all(|kind| kind.starts_with("rust_")));
    }

    #[test]
    fn list_target_kinds_combines_multiple_runner_ecosystems() {
        let tmp = TempDir::new().unwrap();
        let value = tool_list_target_kinds(
            tmp.path(),
            &json!({ "query": "mixed repository native test runners Python JavaScript TypeScript Rust Go Ruby" }),
        )
        .unwrap();
        let kinds = value
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["kind"].as_str().unwrap())
            .collect::<Vec<_>>();

        assert!(kinds.contains(&"pytest_test"));
        assert!(kinds.contains(&"vitest_test"));
        assert!(kinds.contains(&"jest_test"));
        assert!(kinds.contains(&"rspec_test"));
        assert!(kinds.contains(&"minitest_test"));
        assert!(kinds.contains(&"rust_test"));
        assert!(!kinds.contains(&"apple_library"));
        assert!(!kinds.contains(&"android_test"));
        assert!(!kinds.contains(&"go_test"));
    }

    #[test]
    fn mcp_tools_use_workspace_custom_modules() {
        let tmp = TempDir::new().unwrap();
        seed_custom_module_workspace(tmp.path());

        let schema = tool_query_schema(tmp.path(), &json!({ "kind": "demo_kind" })).unwrap();
        assert_eq!(schema["kind"], "demo_kind");
        assert_eq!(schema["providers"], json!(["demo_provider"]));

        let target_kinds = tool_list_target_kinds(tmp.path(), &json!({})).unwrap();
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
