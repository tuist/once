//! `struo mcp` exposes Struo's graph and memory queries over the Model Context
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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use struo_core::ResourceLimits;

mod args;
mod artifacts;
mod jsonrpc;
mod runs;
mod script;
#[cfg(test)]
mod tests;
mod tools;
mod transport;
mod verification;
mod views;

use args::{
    parse_args, tool_args, BuildTargetArgs, ChangedPathsArgs, EvidenceQueryArgs,
    ExternalSourceArgs, GraphFingerprintArgs, MaterializeExampleArgs, RunTargetArgs, RunTestsArgs,
    RuntimeLogsArgs, RuntimeSessionArgs, StartTargetArgs, TargetExecutionArgs,
    TargetKindFilterArgs, TestAttemptQueryArgs, TestPlanQueryArgs, TriggerActivationArgs,
    TriggerEventArgs, TriggerListArgs, ValidateActionsArgs, ValidateModuleArgs,
    VerificationQueryArgs,
};
use jsonrpc::{
    text_content, tool_error, tool_error_from_error, JsonRpcRequest, JsonRpcResponse,
    ToolCallParams, INVALID_PARAMS, METHOD_NOT_FOUND, PARSE_ERROR,
};
use runs::{run_graph_target, run_test_plan_args, summarize_test_run_results};
pub(crate) use tools::tool_catalog;
use tools::{tool_definitions, tool_requires_allow_run};
use transport::serve_blocking;
use views::{CapabilityView, TargetKindSummary, TargetView};

/// MCP protocol version we negotiate.
const PROTOCOL_VERSION: &str = "2025-11-25";
const SERVER_INSTRUCTIONS: &str = concat!(
    "Struo is a self-describing build graph and cacheable automation server. ",
    "Call struo_query_workspace first to confirm the exact configured root and current graph state. ",
    "For a new typed graph with no named upstream rule, continue with struo_list_target_kinds. ",
    "Pass its optional query when the request names ecosystems, target-kind families, or intent; ",
    "include every named native test runner in one query when adopting a mixed test repository, ",
    "then call struo_query_schema for each chosen kind. For an unchanged starter, call ",
    "struo_materialize_example when it is advertised; it keeps large file bundles out of model ",
    "context and returns canonical targets, workspace validation, and suggested next calls. ",
    "Use struo_query_example only when you need to inspect or adapt the starter contents yourself. ",
    "When the request already points to a specific external rule or plugin, skip the target-kind ",
    "catalog: call struo_query_module_contract, fetch its authoritative source with ",
    "struo_fetch_external_source, write a project-local module, and call struo_validate_module. ",
    "Model only the requested dependency closure. Record upstream source_references and the ",
    "returned content digest when the source was not truncated, then re-fetch and compare the ",
    "digest before future maintenance. After custom edits, inspect canonical ids with ",
    "struo_query_targets, call struo_validate_workspace, and use capability tools to build, run, ",
    "or test. New test targets intentionally begin with one whole-target batch. Run each target ",
    "completely once, then inspect next_plan in the completed struo_run_tests response to see the ",
    "automatic file or case batches established for later runs. The plan field always describes ",
    "the work that just ran. Before change-scoped testing, inspect conservative selection, ",
    "unmatched paths, and stable batches with struo_query_test_plan, which always exports a ",
    "replayable plan. struo_run_tests returns executed scopes, refreshed next scopes, a ",
    "duration-informed dynamic schedule, and results. Pass one listed unit with one explicit ",
    "target to run an exact filtered test ",
    "when the target kind declares filtering support. Use jobs only to cap local concurrency; it ",
    "does not change plan or batch identity. For annotated automation, call struo_validate_script ",
    "before struo_exec_script. Stateful tools require --allow-run. Evidence subjects are target ids ",
    "with an optional capability suffix; script execution returns its evidence subject directly. ",
    "struo_validate_target checks one proposed table, struo_validate_workspace checks the loaded ",
    "graph, and successful execution remains the authoritative current check. Evidence is ",
    "historical provenance, so do not treat an older record as proof that inputs are unchanged. ",
    "When a native project's graph fails to load, or its targets, sources, or dependency edges ",
    "disagree with what the native build system builds, the gap is in Struo, not in the project: ",
    "do not work around it with manifest edits. Offer to run struo_export_graph_repro, which writes ",
    "a copy that keeps native manifests and replaces every other file with an empty placeholder, ",
    "an archive of it, and a drafted issue for the Struo repository. Show the user the copied files ",
    "to review, and with their approval open the issue it drafts and ask them to attach the ",
    "archive. When working on Struo itself, load an export to reproduce the gap and turn it into a ",
    "native graph fixture with a regression test before fixing the target kind."
);

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
pub async fn serve(
    workspace: PathBuf,
    allow_run: bool,
    resource_limits: ResourceLimits,
) -> Result<()> {
    let resources = struo_frontend::analysis::analysis_resource_pool();
    tokio::task::spawn_blocking(move || {
        struo_frontend::analysis::with_analysis_resources_blocking(resources, || {
            serve_blocking(workspace, allow_run, resource_limits)
        })
    })
    .await
    .context("joining MCP server thread")?
}

/// State the dispatcher needs to answer requests.
struct Server {
    workspace: PathBuf,
    allow_run: bool,
    resource_limits: ResourceLimits,
}

#[derive(Debug)]
enum DispatchOutcome {
    Reply(JsonRpcResponse),
    Notification,
}

impl Server {
    #[cfg(test)]
    fn new(workspace: PathBuf, allow_run: bool) -> Self {
        Self::with_resource_limits(workspace, allow_run, ResourceLimits::default())
    }

    fn with_resource_limits(
        workspace: PathBuf,
        allow_run: bool,
        resource_limits: ResourceLimits,
    ) -> Self {
        Self {
            workspace,
            allow_run,
            resource_limits,
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
                        "serverInfo": { "name": "struo-mcp", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": format!(
                            "{SERVER_INSTRUCTIONS} Configured workspace root: `{}`.",
                            self.workspace.display()
                        ),
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
                "state-changing tools require starting `struo mcp --allow-run`",
            );
        }
        let result = match call.name.as_str() {
            "struo_query_workspace" => crate::commands::query::workspace_value(&self.workspace),
            "struo_list_triggers" => self.tool_list_triggers(&call.arguments),
            "struo_validate_trigger" => {
                crate::commands::triggers::validate(tool_args(&call.arguments))
            }
            "struo_edit_triggers" => {
                crate::commands::triggers::edit(&self.workspace, tool_args(&call.arguments))
            }
            "struo_activate_triggers" => self.tool_activate_triggers(&call.arguments),
            "struo_plan_trigger" => self.tool_plan_trigger(&call.arguments),
            "struo_run_trigger" => self.tool_run_trigger(&call.arguments),
            "struo_query_targets" => self.tool_query_targets(&call.arguments),
            "struo_query_capabilities" => self.tool_query_capabilities(&call.arguments),
            "struo_query_artifacts" => self.tool_query_artifacts(&call.arguments),
            "struo_export_artifacts" => self.tool_artifacts(&call.arguments, false),
            "struo_publish_artifacts" => self.tool_artifacts(&call.arguments, true),
            "struo_get_target" => self.tool_get_target(&call.arguments),
            "struo_query_tests" => self.tool_query_tests(),
            "struo_query_affected_tests" => self.tool_query_affected_tests(&call.arguments),
            "struo_query_test_plan" => self.tool_query_test_plan(&call.arguments),
            "struo_run_tests" => self.tool_run_tests(&call.arguments),
            "struo_query_test_results" => self.tool_query_test_results(&call.arguments),
            "struo_query_test_manifest" => self.tool_query_test_manifest(&call.arguments),
            "struo_query_test_attempts" => self.tool_query_test_attempts(&call.arguments),
            "struo_query_evidence" => self.tool_query_evidence(&call.arguments),
            "struo_query_verification" => self.tool_query_verification(&call.arguments),
            "struo_query_graph_fingerprint" => self.tool_query_graph_fingerprint(&call.arguments),
            "struo_export_graph_repro" => {
                crate::commands::query::graph_repro_value(&self.workspace, None)
            }
            "struo_query_module_contract" => crate::commands::query::module_contract_value(),
            "struo_fetch_external_source" => Self::tool_fetch_external_source(&call.arguments),
            "struo_validate_module" => self.tool_validate_module(&call.arguments),
            "struo_validate_workspace" => {
                crate::commands::query::workspace_validation_value(&self.workspace)
            }
            "struo_validate_script" => script::validate(&self.workspace, &call.arguments),
            "struo_exec_script" => {
                script::execute(&self.workspace, &call.arguments, &self.resource_limits)
            }
            "struo_build_target" => self.tool_build_target(&call.arguments),
            "struo_lint_target" => self.tool_lint_target(&call.arguments),
            "struo_validate_actions" => self.tool_validate_actions(&call.arguments),
            "struo_run_target" => self.tool_run_target(&call.arguments),
            "struo_start_target" => self.tool_start_target(&call.arguments),
            "struo_runtime_status" => self.tool_runtime_status(&call.arguments),
            "struo_runtime_logs" => self.tool_runtime_logs(&call.arguments),
            "struo_stop_runtime" => self.tool_stop_runtime(&call.arguments),
            "struo_apply_edit" => self.tool_apply_edit(&call.arguments),
            "struo_materialize_example" => self.tool_materialize_example(&call.arguments),
            "struo_query_schema" => tool_query_schema(&self.workspace, &call.arguments),
            "struo_query_example" => tool_query_example(&self.workspace, &call.arguments),
            "struo_list_target_kinds" | "struo_list_rules" => {
                tool_list_target_kinds(&self.workspace, &call.arguments)
            }
            "struo_validate_target" => tool_validate_target(&self.workspace, &call.arguments),
            other => Err(anyhow::anyhow!("unknown tool `{other}`")),
        };
        match result {
            Ok(value) => {
                let failed = call.name == "struo_run_trigger" && value["status"] == "failed";
                let mut result = json!({
                    "content": [text_content(&value)],
                    "structuredContent": { "result": value },
                });
                if failed {
                    result["isError"] = json!(true);
                }
                JsonRpcResponse::ok(id, result)
            }
            Err(error) => tool_error_from_error(id, &error),
        }
    }

    fn tool_list_triggers(&self, args: &Value) -> Result<Value> {
        let _: TriggerListArgs = parse_args(args)?;
        crate::commands::triggers::list(&self.workspace)
    }

    fn tool_activate_triggers(&self, args: &Value) -> Result<Value> {
        let args: TriggerActivationArgs = parse_args(args)?;
        crate::commands::triggers::activate(&self.workspace, &args.revision)
    }

    fn tool_plan_trigger(&self, args: &Value) -> Result<Value> {
        let args: TriggerEventArgs = parse_args(args)?;
        crate::commands::triggers::plan(&self.workspace, &args.id, args.event)
    }

    fn tool_run_trigger(&self, args: &Value) -> Result<Value> {
        let args: TriggerEventArgs = parse_args(args)?;
        let workspace = self.workspace.clone();
        let limits = self.resource_limits.clone();
        run_async_result(async move {
            crate::commands::triggers::run(&workspace, &args.id, args.event, limits).await
        })
    }

    fn tool_query_targets(&self, args: &Value) -> Result<Value> {
        let kind = args.get("kind").and_then(Value::as_str).map(str::to_string);
        let graph =
            struo_frontend::load_graph_workspace(&self.workspace).context("loading graph")?;
        crate::commands::query::ensure_graph_resolved(&graph)?;
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
            struo_frontend::load_graph_workspace(&self.workspace).context("loading graph")?;
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
            struo_frontend::load_graph_workspace(&self.workspace).context("loading graph")?;
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
        let operations: Vec<struo_frontend::EditOperation> =
            serde_json::from_value(raw_ops.clone())
                .map_err(|err| anyhow::anyhow!("invalid `operations`: {err}"))?;
        apply_edit_to_package(&self.workspace, package, &operations)
    }

    fn tool_materialize_example(&self, args: &Value) -> Result<Value> {
        let args: MaterializeExampleArgs = parse_args(args)?;
        crate::commands::edit::materialize_example_json(
            &self.workspace,
            &args.kind,
            &args.slug,
            &args.destination,
        )
    }

    fn tool_query_tests(&self) -> Result<Value> {
        crate::commands::query::tests_value(&self.workspace)
    }

    fn tool_query_affected_tests(&self, args: &Value) -> Result<Value> {
        let args: ChangedPathsArgs = parse_args(args)?;
        crate::commands::query::affected_tests_value(&self.workspace, &args.changed_paths)
    }

    fn tool_query_test_plan(&self, args: &Value) -> Result<Value> {
        let args: TestPlanQueryArgs = parse_args(args)?;
        let plan = crate::commands::query::test_plan_record(
            &self.workspace,
            &args.changed_paths,
            args.target.as_deref(),
        )?;
        Ok(serde_json::to_value(plan)?)
    }

    fn tool_query_test_results(&self, args: &Value) -> Result<Value> {
        let target_id = args
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?;
        let summary_only = args
            .get("summary_only")
            .map(|value| value.as_bool().context("`summary_only` must be a boolean"))
            .transpose()?
            .unwrap_or(false);
        let case = args
            .get("case")
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    crate::commands::query::test_case::diagnostic(
                        target_id,
                        "invalid_test_result_selection",
                        "case",
                        "Use a string for the exact retained case id.",
                    )
                })
            })
            .transpose()?;
        let attempt = args
            .get("attempt")
            .map(|value| {
                crate::commands::query::test_case::attempt_position(target_id, value.as_u64())
            })
            .transpose()?;
        crate::commands::query::test_case::validate_selection(
            target_id,
            summary_only,
            case,
            attempt,
        )?;
        if let Some(case) = case {
            return crate::commands::query::test_case_value(
                &self.workspace,
                target_id,
                case,
                attempt,
            );
        }
        let value = crate::commands::query::test_results_value(&self.workspace, target_id)?;
        if summary_only {
            return crate::commands::query::test_results_summary_value(&value);
        }
        Ok(value)
    }

    fn tool_query_test_manifest(&self, args: &Value) -> Result<Value> {
        let target_id = args
            .get("target")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing `target` argument"))?;
        crate::commands::query::test_manifest_value(&self.workspace, target_id)
    }

    fn tool_query_test_attempts(&self, args: &Value) -> Result<Value> {
        let args: TestAttemptQueryArgs = parse_args(args)?;
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
        let args: EvidenceQueryArgs = parse_args(args)?;
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

    fn tool_query_graph_fingerprint(&self, args: &Value) -> Result<Value> {
        let args: GraphFingerprintArgs = parse_args(args)?;
        let options = crate::commands::fingerprint::GraphFingerprintOptions {
            include_sources: args.include_sources,
            include_toolchain: args.include_toolchain,
            include_manifest: args.include_manifest,
        };
        crate::commands::query::graph_fingerprint_value(&self.workspace, options)
    }

    fn tool_fetch_external_source(args: &Value) -> Result<Value> {
        let args: ExternalSourceArgs = parse_args(args)?;
        crate::commands::query::external_source_value(&args.url, args.max_bytes)
    }

    fn tool_validate_module(&self, args: &Value) -> Result<Value> {
        let args: ValidateModuleArgs = parse_args(args)?;
        crate::commands::query::module_validation_value(&self.workspace, &args.path)
    }

    fn tool_run_tests(&self, args: &Value) -> Result<Value> {
        let args: RunTestsArgs = parse_args(args)?;
        let plan = run_test_plan_args(&self.workspace, &args)?;
        let workspace = self.workspace.clone();
        let workers = args.jobs;
        let summary_only = args.summary_only;
        let resource_limits = self.resource_limits.clone();
        run_async_result(async move {
            let report = crate::commands::test_schedule::execute(
                &workspace,
                None,
                plan,
                workers,
                struo_core::SandboxMode::Off,
                resource_limits,
            )
            .await?;
            let mut value = serde_json::to_value(report)?;
            if summary_only {
                summarize_test_run_results(&mut value)?;
            }
            Ok(value)
        })
    }

    fn tool_query_verification(&self, args: &Value) -> Result<Value> {
        let args: VerificationQueryArgs = parse_args(args)?;
        Ok(serde_json::to_value(crate::commands::verification::load(
            &self.workspace,
            &args.id,
        )?)?)
    }

    fn tool_build_target(&self, args: &Value) -> Result<Value> {
        let args: BuildTargetArgs = parse_args(args)?;
        if args.verify_reproducible {
            return verification::execute(
                &self.workspace,
                &self.resource_limits,
                &["build".into(), "--verify-reproducible".into(), args.target],
            );
        }
        run_graph_target(
            &self.workspace,
            "build",
            &args.target,
            false,
            &[],
            &self.resource_limits,
        )
    }

    fn tool_lint_target(&self, args: &Value) -> Result<Value> {
        let args: TargetExecutionArgs = parse_args(args)?;
        run_graph_target(
            &self.workspace,
            "lint",
            &args.target,
            false,
            &[],
            &self.resource_limits,
        )
    }

    fn tool_validate_actions(&self, args: &Value) -> Result<Value> {
        let args: ValidateActionsArgs = parse_args(args)?;
        let workspace = self.workspace.clone();
        let cache = crate::cache_provider::resolve(&workspace, &struo_core::Xdg::from_env())?;
        let target = args.target;
        let capability = args.capability;
        let action = args.action;
        let result = run_async_result(async move {
            crate::commands::graph::validate_action_contracts(
                &workspace,
                &cache,
                &target,
                &capability,
                action,
            )
            .await
        })?;
        Ok(serde_json::to_value(result)?)
    }

    fn tool_run_target(&self, args: &Value) -> Result<Value> {
        let args: RunTargetArgs = parse_args(args)?;
        run_graph_target(
            &self.workspace,
            "run",
            &args.target,
            args.visible,
            &args.arguments,
            &self.resource_limits,
        )
    }

    fn tool_start_target(&self, args: &Value) -> Result<Value> {
        let args: StartTargetArgs = parse_args(args)?;
        let target = struo_frontend::normalize_cli_target_from(
            &self.workspace,
            &self.workspace,
            &args.target,
        )
        .context("resolving target argument")?;
        Ok(serde_json::to_value(
            crate::commands::runtime::start_session(&self.workspace, &target)?,
        )?)
    }

    fn tool_runtime_status(&self, args: &Value) -> Result<Value> {
        let args: RuntimeSessionArgs = parse_args(args)?;
        Ok(serde_json::to_value(
            crate::commands::runtime::status_session(&self.workspace, &args.session_id)?,
        )?)
    }

    fn tool_runtime_logs(&self, args: &Value) -> Result<Value> {
        let args: RuntimeLogsArgs = parse_args(args)?;
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
        let args: RuntimeSessionArgs = parse_args(args)?;
        Ok(serde_json::to_value(
            crate::commands::runtime::stop_session(&self.workspace, &args.session_id)?,
        )?)
    }
}

fn tool_query_schema(workspace: &Path, args: &Value) -> Result<Value> {
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing `kind` argument"))?;
    let schema = struo_frontend::target_kind_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no target kind schema matches `{kind}`"))?;
    Ok(serde_json::to_value(schema)?)
}

fn tool_query_example(workspace: &Path, args: &Value) -> Result<Value> {
    let kind = required_string_arg(args, "kind")?;
    let slug = required_string_arg(args, "slug")?;
    let schema = struo_frontend::target_kind_schemas_for_workspace(workspace)?
        .into_iter()
        .find(|schema| schema.kind == kind)
        .with_context(|| format!("no target kind schema matches `{kind}`"))?;
    let example = struo_frontend::load_target_kind_example(&schema, slug)?;
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
    let args: TargetKindFilterArgs = parse_args(args)?;
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
    let spec: struo_frontend::TargetSpec = serde_json::from_value(raw_target)
        .map_err(|err| anyhow::anyhow!("invalid `target`: {err}"))?;
    let schemas = struo_frontend::target_kind_schemas_for_workspace(workspace)?;
    let diagnostics = struo_frontend::validate_target(&spec, &schemas);
    if diagnostics.is_empty() {
        Ok(json!({ "valid": true }))
    } else {
        Ok(json!({ "valid": false, "diagnostics": diagnostics }))
    }
}

fn apply_edit_to_package(
    workspace: &std::path::Path,
    package: &str,
    operations: &[struo_frontend::EditOperation],
) -> Result<Value> {
    let package_dir = crate::commands::edit::resolve_package_dir(workspace, package)?;
    let manifest_path = package_dir.join(struo_frontend::TOML_BUILD_FILE_NAME);
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
    let schemas = struo_frontend::target_kind_schemas_for_workspace(workspace)
        .context("loading target kind schemas")?;
    match struo_frontend::apply_operations_with_schemas(&existing, operations, &schemas) {
        Ok(new_src) => {
            let changed = new_src != existing;
            if changed {
                std::fs::create_dir_all(&package_dir).with_context(|| {
                    format!("creating package directory `{}`", package_dir.display())
                })?;
                std::fs::write(&manifest_path, &new_src)
                    .with_context(|| format!("writing `{}`", manifest_path.display()))?;
            }
            Ok(json!({
                "applied": true,
                "changed": changed,
                "path": manifest_path.to_string_lossy(),
            }))
        }
        Err(diagnostics) => Ok(json!({ "applied": false, "diagnostics": diagnostics })),
    }
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
