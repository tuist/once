use super::runs::{parse_json_record, run_graph_target_with_exe, run_test_plan};
use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use struo_cas::{ActionResult, Digest};
use struo_core::{EvidenceCacheState, EvidenceRecord, EvidenceStore, EvidenceSubject};
use tempfile::TempDir;

fn server(workspace: PathBuf) -> Server {
    Server::new(workspace, false)
}

#[test]
fn summarized_test_run_results_keep_totals_without_cases() {
    let mut value = json!({
        "runs": [
            {
                "target": "tests/unit",
                "results": {
                    "schema": "struo.test_results.v1",
                    "target": "tests/unit",
                    "runner": { "type": "rust_libtest", "metadata": {} },
                    "status": "passed",
                    "summary": {
                        "total": 2,
                        "passed": 1,
                        "failed": 0,
                        "skipped": 1,
                        "flaky": 0
                    },
                    "cases": [
                        { "name": "passes", "status": "passed" },
                        { "name": "skips", "status": "skipped" }
                    ],
                    "artifacts": { "logs": [], "native_results": [] }
                }
            }
        ]
    });

    summarize_test_run_results(&mut value).unwrap();

    assert_eq!(
        value["runs"][0]["results"]["schema"],
        "struo.test_results_summary.v1"
    );
    assert_eq!(value["runs"][0]["results"]["summary"]["total"], 2);
    assert!(value["runs"][0]["results"].get("cases").is_none());
}

#[test]
fn summarized_test_run_results_tolerate_partial_result() {
    // A non-null but partial result (missing `runner`/`artifacts`) must not
    // fail the whole report; absent fields become null.
    let mut value = json!({
        "runs": [
            {
                "target": "tests/unit",
                "results": {
                    "schema": "struo.test_results.v1",
                    "target": "tests/unit",
                    "status": "errored",
                    "summary": { "total": 0, "passed": 0, "failed": 0, "skipped": 0, "flaky": 0 }
                }
            }
        ]
    });

    summarize_test_run_results(&mut value).unwrap();

    assert_eq!(
        value["runs"][0]["results"]["schema"],
        "struo.test_results_summary.v1"
    );
    assert_eq!(value["runs"][0]["results"]["status"], "errored");
    assert!(value["runs"][0]["results"]["runner"].is_null());
    assert!(value["runs"][0]["results"]["artifacts"].is_null());
}

fn run_server(workspace: PathBuf) -> Server {
    Server::new(workspace, true)
}

fn seed_custom_module_workspace(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("modules")).unwrap();
    std::fs::write(
        root.join("struo.toml"),
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
    assert_eq!(result["serverInfo"]["name"], "struo-mcp");
    assert!(result["instructions"]
        .as_str()
        .unwrap()
        .contains("struo_list_target_kinds"));
    assert!(result["instructions"]
        .as_str()
        .unwrap()
        .contains(tmp.path().to_string_lossy().as_ref()));
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
            "struo_query_artifacts".to_string(),
            "struo_query_workspace".to_string(),
            "struo_list_triggers".to_string(),
            "struo_validate_trigger".to_string(),
            "struo_plan_trigger".to_string(),
            "struo_query_targets".to_string(),
            "struo_query_capabilities".to_string(),
            "struo_query_schema".to_string(),
            "struo_query_example".to_string(),
            "struo_list_target_kinds".to_string(),
            "struo_query_module_contract".to_string(),
            "struo_fetch_external_source".to_string(),
            "struo_validate_module".to_string(),
            "struo_get_target".to_string(),
            "struo_query_tests".to_string(),
            "struo_query_affected_tests".to_string(),
            "struo_query_test_plan".to_string(),
            "struo_query_test_results".to_string(),
            "struo_query_test_manifest".to_string(),
            "struo_query_test_attempts".to_string(),
            "struo_query_verification".to_string(),
            "struo_query_evidence".to_string(),
            "struo_query_graph_fingerprint".to_string(),
            "struo_validate_script".to_string(),
            "struo_validate_workspace".to_string(),
            "struo_validate_target".to_string(),
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
        json!({ "name": "struo_query_module_contract", "arguments": {} }),
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
            "name": "struo_validate_module",
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
            "name": "struo_fetch_external_source",
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
        tmp.path().join("struo.toml"),
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
            "name": "struo_validate_workspace",
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
            "name": "struo_query_evidence",
            "arguments": { "subject": "cli:test" }
        }),
    ));

    assert!(response.error.is_none());
    let result = response.result.expect("result");
    let text = result["content"][0]["text"].as_str().expect("text content");
    let records: Vec<serde_json::Value> = serde_json::from_str(text).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["schema"], "struo.evidence.v1");
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
            "name": "struo_query_evidence",
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
            "name": "struo_query_evidence",
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
    let response = run_server(tmp.path().to_path_buf()).dispatch(request("tools/list", json!({})));
    let result = response.result.unwrap();
    let names: Vec<String> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"struo_build_target".to_string()));
    assert!(names.contains(&"struo_lint_target".to_string()));
    assert!(names.contains(&"struo_run_target".to_string()));
    assert!(names.contains(&"struo_start_target".to_string()));
    assert!(names.contains(&"struo_runtime_status".to_string()));
    assert!(names.contains(&"struo_runtime_logs".to_string()));
    assert!(names.contains(&"struo_stop_runtime".to_string()));
    assert!(names.contains(&"struo_run_tests".to_string()));
    assert!(names.contains(&"struo_apply_edit".to_string()));
    assert!(names.contains(&"struo_materialize_example".to_string()));
    assert!(names.contains(&"struo_exec_script".to_string()));

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
            "name": "struo_list_rules",
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
        "struo_build_target",
        "struo_export_artifacts",
        "struo_publish_artifacts",
        "struo_lint_target",
        "struo_run_target",
        "struo_start_target",
        "struo_run_tests",
        "struo_exec_script",
        "struo_apply_edit",
        "struo_materialize_example",
        "struo_edit_triggers",
        "struo_activate_triggers",
        "struo_run_trigger",
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
fn trigger_inspection_and_validation_do_not_evaluate_project_modules() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        "[modules]\npaths = [\"missing.star\"]\n",
    )
    .unwrap();
    let server = server(tmp.path().to_path_buf());
    let response = server.dispatch(request(
        "tools/call",
        json!({ "name": "struo_list_triggers", "arguments": {} }),
    ));
    let result = response.result.unwrap();
    assert_ne!(result["isError"], true);
    assert_eq!(
        result["structuredContent"]["result"]["schema"],
        "struo.triggers.v1"
    );
    assert_eq!(
        result["structuredContent"]["result"]["activation"],
        Value::Null
    );

    let response = server.dispatch(request(
        "tools/call",
        json!({
            "name": "struo_validate_trigger",
            "arguments": { "trigger": {
                "id": "verify-main",
                "on": { "event": "vcs.ref_changed", "refs": ["refs/heads/main"] },
                "invoke": [{
                    "target": "checks/core",
                    "capability": "test",
                    "configuration": { "os": "linux", "arch": "x86_64", "tokens": [] }
                }]
            }}
        }),
    ));
    let result = response.result.unwrap();
    assert_ne!(result["isError"], true);
    assert_eq!(result["structuredContent"]["result"]["valid"], true);
}

#[test]
fn trigger_activation_and_plan_are_data_only_and_failed_runs_keep_their_record() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().canonicalize().unwrap();
    std::fs::write(workspace.join(".gitignore"), ".struo/\n").unwrap();
    std::fs::write(
            workspace.join("struo.toml"),
            r#"
[modules]
paths = ["missing.star"]

[[trigger]]
id = "verify-main"
on = { event = "vcs.ref_changed", refs = ["refs/heads/main"] }
invoke = [{ target = "checks/core", capability = "test", configuration = { os = "linux", arch = "x86_64", tokens = [] } }]
"#,
        )
        .unwrap();
    let git = |arguments: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(&workspace)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Struo Test",
        "-c",
        "user.email=struo@example.test",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "trigger fixture",
    ]);
    let revision = git(&["rev-parse", "HEAD"]);
    let server =
        Server::with_resource_limits(workspace, true, ResourceLimits::new(1, 512 * 1024 * 1024));
    let result = server
        .dispatch(request(
            "tools/call",
            json!({ "name": "struo_activate_triggers", "arguments": { "revision": revision } }),
        ))
        .result
        .unwrap();
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["result"]["activated"], true);
    let arguments = json!({
        "id": "verify-main",
        "event": { "event": "vcs.ref_changed", "ref": "refs/heads/main", "revision": revision }
    });
    let result = server
        .dispatch(request(
            "tools/call",
            json!({ "name": "struo_plan_trigger", "arguments": arguments }),
        ))
        .result
        .unwrap();
    assert_ne!(result["isError"], true, "{result}");
    let selection = &result["structuredContent"]["result"];
    assert_eq!(selection["mode"], "simulation");
    assert_eq!(
        selection["trigger"]["invoke"][0]["configuration"],
        json!({ "os": "linux", "arch": "x86_64", "tokens": [] })
    );
    let result = server
        .dispatch(request(
            "tools/call",
            json!({ "name": "struo_run_trigger", "arguments": arguments }),
        ))
        .result
        .unwrap();
    assert_eq!(result["isError"], true, "{result}");
    let record = &result["structuredContent"]["result"];
    assert_eq!(record["schema"], "struo.trigger_run.v1");
    assert_eq!(record["status"], "failed");
    assert_eq!(record["memory_limit"], 512 * 1024 * 1024);
    assert_eq!(record["selection"], *selection);
    assert!(record["error"].as_str().unwrap().contains("missing.star"));
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        *record
    );
}

#[test]
fn trigger_handlers_reject_unknown_arguments_instead_of_ignoring_them() {
    let tmp = TempDir::new().unwrap();
    let server = Server::new(tmp.path().to_path_buf(), true);
    for name in [
        "struo_list_triggers",
        "struo_validate_trigger",
        "struo_edit_triggers",
        "struo_activate_triggers",
        "struo_plan_trigger",
        "struo_run_trigger",
    ] {
        let response = server.dispatch(request(
            "tools/call",
            json!({ "name": name, "arguments": { "unexpected": true } }),
        ));
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true, "{name}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("unknown tool"), "{name}: {text}");
    }
}

#[test]
fn analysis_tool_errors_include_structured_diagnostics() {
    let diagnostic = struo_frontend::Diagnostic::new(
        "target_kind_analysis_failed",
        "target kind implementation failed",
    )
    .with_target("App")
    .with_attribute("mode")
    .with_repair("Set a supported mode");
    let error = anyhow::Error::new(struo_frontend::analysis::AnalysisFailure { diagnostic });

    let response = tool_error_from_error(json!(1), &error);
    let result = response.result.expect("tool result");

    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "target_kind_analysis_failed"
    );
    assert_eq!(
        result["structuredContent"]["error"]["diagnostics"][0]["attribute"],
        "mode"
    );
}

#[test]
fn materialize_example_creates_a_valid_workspace_without_returning_contents() {
    let tmp = TempDir::new().unwrap();
    let response = run_server(tmp.path().to_path_buf()).dispatch(request(
        "tools/call",
        json!({
            "name": "struo_materialize_example",
            "arguments": {
                "kind": "rust_library",
                "slug": "rust-library-minimal"
            }
        }),
    ));

    assert!(response.error.is_none());
    let result = response.result.unwrap()["structuredContent"]["result"].clone();
    assert_eq!(result["materialized"], true);
    assert_eq!(result["workspace_validation"]["valid"], true);
    assert!(result["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|target| target["id"] == "crates/hello/hello"));
    assert!(!result.to_string().contains("pub fn"));
    assert!(tmp.path().join("crates/hello/src/lib.rs").is_file());
}

#[test]
fn query_workspace_makes_an_empty_configured_root_explicit() {
    let tmp = TempDir::new().unwrap();
    let response = server(tmp.path().to_path_buf()).dispatch(request(
        "tools/call",
        json!({
            "name": "struo_query_workspace",
            "arguments": {}
        }),
    ));

    let result = response.result.unwrap()["structuredContent"]["result"].clone();
    assert_eq!(result["root"], tmp.path().to_string_lossy().as_ref());
    assert_eq!(result["target_count"], 0);
    assert_eq!(result["empty"], true);
    assert_eq!(
        result["suggested_calls"][0]["tool"],
        "struo_list_target_kinds"
    );
}

#[test]
fn graph_fingerprint_returns_a_stable_digest_with_categorized_components() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("struo.toml"),
        "[workspace]\ninclude = [\"struo.toml\", \"pkg/struo.toml\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("pkg")).unwrap();
    std::fs::write(
        tmp.path().join("pkg/struo.toml"),
        "[[target]]\nname = \"lib\"\nkind = \"library\"\nsrcs = [\"lib.rs\"]\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("pkg/lib.rs"), "fn lib() {}").unwrap();
    std::fs::write(tmp.path().join("mise.toml"), "[tools]\nrust = \"1.96.0\"\n").unwrap();

    let digest_for = |include_sources: bool| {
        let response = server(tmp.path().to_path_buf()).dispatch(request(
            "tools/call",
            json!({
                "name": "struo_query_graph_fingerprint",
                "arguments": { "include_sources": include_sources }
            }),
        ));
        assert!(response.error.is_none(), "{response:?}");
        response.result.unwrap()["structuredContent"]["result"].clone()
    };

    let with_sources = digest_for(true);
    assert_eq!(with_sources["schema"], "struo.graph_fingerprint.v1");
    assert_eq!(with_sources["target_count"], 1);
    assert_eq!(with_sources["source_count"], 1);
    let digest = with_sources["digest"].as_str().unwrap().to_string();
    assert_eq!(digest.len(), 64);

    let categories = with_sources["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["category"].as_str().unwrap().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(categories.contains("target"));
    assert!(categories.contains("source"));
    assert!(categories.contains("toolchain"));

    // Same graph and inputs resolve to the same digest.
    assert_eq!(digest_for(true)["digest"], digest);

    // Excluding sources produces a different digest and no source component.
    let structure_only = digest_for(false);
    assert_ne!(structure_only["digest"], digest);
    assert_eq!(structure_only["source_count"], 0);
}

#[test]
fn an_empty_edit_does_not_create_a_manifest() {
    let tmp = TempDir::new().unwrap();

    let result = apply_edit_to_package(tmp.path(), "new/package", &[]).unwrap();

    assert_eq!(result["applied"], true);
    assert_eq!(result["changed"], false);
    assert!(!tmp.path().join("new/package/struo.toml").exists());
}

#[test]
fn run_test_plan_prefers_explicit_deduplicated_targets() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("spec")).unwrap();
    std::fs::write(
        tmp.path().join("spec/struo.toml"),
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
fn exact_batch_selection_rejects_ambiguous_requests() {
    let tmp = TempDir::new().unwrap();
    for args in [
        json!({"test_batch": "unknown"}),
        json!({"target": "tests", "test_unit": "a", "test_batch": "unknown"}),
        json!({"target": "tests", "test_batch": "unknown", "changed_paths": ["source"]}),
    ] {
        assert!(run_test_plan(tmp.path(), &args).is_err());
    }
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
        tmp.path().join("struo.toml"),
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
    let exe = tmp.path().join("struo-mock");
    let args_path = tmp.path().join("args.txt");
    let cwd_path = tmp.path().join("cwd.txt");
    let args_file = shell_literal(args_path.to_str().unwrap());
    let cwd_file = shell_literal(cwd_path.to_str().unwrap());
    let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {args_file}\npwd > {cwd_file}\ncapability=''\ntarget=''\nnext_is_target=false\nfor arg do\n  case \"$arg\" in\n    build|run|test) capability=\"$arg\"; next_is_target=true ;;\n    --visible|--) ;;\n    *) if \"$next_is_target\"; then target=\"$arg\"; next_is_target=false; fi ;;\n  esac\ndone\nmkdir -p .struo/out/mock/run\nprintf 'hello from target\\n' > .struo/out/mock/run/stdout.log\nprintf '{{\"target\":\"%s\",\"capability\":\"%s\",\"status\":\"completed\",\"outputs\":[\".struo/out/mock/run/stdout.log\"]}}\\n' \"$target\" \"$capability\"\n",
        );
    std::fs::write(&exe, script).unwrap();
    let mut permissions = std::fs::metadata(&exe).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&exe, permissions).unwrap();

    let build =
        run_graph_target_with_exe(&exe, tmp.path(), "build", "apps/App", false, &[]).unwrap();
    assert_eq!(build["capability"], "build");
    assert_eq!(build["success"], true);
    assert_eq!(build["record"]["target"], "apps/App");
    assert_eq!(build["record"]["capability"], "build");

    let arguments = vec![
        "serve".to_string(),
        "--port".to_string(),
        "4001".to_string(),
    ];
    let run =
        run_graph_target_with_exe(&exe, tmp.path(), "run", "apps/App", true, &arguments).unwrap();
    assert_eq!(run["capability"], "run");
    assert_eq!(run["success"], true);
    assert_eq!(run["record"]["target"], "apps/App");
    assert_eq!(run["record"]["capability"], "run");
    assert_eq!(run["captured_stdout"]["text"], "hello from target\n");

    let args = std::fs::read_to_string(args_path).unwrap();
    assert!(args.contains("--format\njson\nrun\n--visible\napps/App\n--\nserve\n--port\n4001\n"));
    let cwd = std::fs::read_to_string(cwd_path).unwrap();
    assert_eq!(
        std::fs::canonicalize(cwd.trim()).unwrap(),
        std::fs::canonicalize(tmp.path()).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn graph_target_runner_preserves_structured_command_diagnostics() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let exe = tmp.path().join("struo-mock");
    let script = r#"#!/bin/sh
printf '%s\n' 'ERROR struo: session failed' >&2
printf '%s\n' '{"schema":"struo.error.v1","error":{"code":"invalid_lint_provider_output","message":"invalid lint provider","diagnostics":[{"code":"invalid_lint_provider_output","message":"invalid lint provider","target":"quality/lint","attribute":"lint_info.outputs.results","repairs":["Return the results path"]}]}}' >&2
exit 2
"#;
    std::fs::write(&exe, script).unwrap();
    let mut permissions = std::fs::metadata(&exe).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&exe, permissions).unwrap();

    let result =
        run_graph_target_with_exe(&exe, tmp.path(), "lint", "quality/lint", false, &[]).unwrap();

    assert_eq!(result["success"], false);
    assert_eq!(result["error"]["code"], "invalid_lint_provider_output");
    assert_eq!(
        result["error"]["diagnostics"][0]["attribute"],
        "lint_info.outputs.results"
    );
}

#[test]
fn unknown_method_returns_method_not_found_error() {
    let tmp = TempDir::new().unwrap();
    let response = server(tmp.path().to_path_buf()).dispatch(request("does/not/exist", json!({})));
    let error = response.error.expect("error");
    assert_eq!(error.code, METHOD_NOT_FOUND);
    assert!(error.message.contains("does/not/exist"));
}

#[test]
fn tools_call_missing_required_argument_lands_in_is_error_payload() {
    let tmp = TempDir::new().unwrap();
    // `struo_query_capabilities` requires `target`; omit it.
    let response = server(tmp.path().to_path_buf()).dispatch(request(
        "tools/call",
        json!({ "name": "struo_query_capabilities", "arguments": {} }),
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
    assert!(files.iter().any(
        |f| f["path"] == "apps/Hello/struo.toml" && !f["contents"].as_str().unwrap().is_empty()
    ));
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
    // The list summarizes; full schema lives behind struo_query_schema.
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
        &json!({ "query": "create a minimal runnable Rust command-line application" }),
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
    assert!(kinds.contains(&"go_test"));
    assert!(!kinds.contains(&"apple_library"));
    assert!(!kinds.contains(&"android_test"));
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
        &[struo_frontend::EditOperation::Create {
            target: struo_frontend::TargetSpec {
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
        &[struo_frontend::EditOperation::Create {
            target: struo_frontend::TargetSpec {
                name: "Hello".to_string(),
                kind: "apple_library".to_string(),
                attrs: serde_json::Map::from_iter([("platform".to_string(), json!("ios"))]),
                ..Default::default()
            },
        }],
    )
    .unwrap();
    assert_eq!(result["applied"], true);
    let written = std::fs::read_to_string(tmp.path().join("apps/Hello/struo.toml")).unwrap();
    assert!(written.contains("name = \"Hello\""));
}

#[test]
fn apply_edit_returns_diagnostics_on_failure_without_writing() {
    let tmp = TempDir::new().unwrap();
    let result = apply_edit_to_package(
        tmp.path(),
        "apps/Hello",
        &[struo_frontend::EditOperation::Delete {
            target_name: "Missing".to_string(),
        }],
    )
    .unwrap();
    assert_eq!(result["applied"], false);
    assert_eq!(result["diagnostics"][0]["code"], "target_not_found");
    // Nothing should have been written when the batch failed.
    assert!(!tmp.path().join("apps/Hello/struo.toml").exists());
}

#[test]
fn get_target_returns_resolved_target_record() {
    let tmp = TempDir::new().unwrap();
    let pkg = tmp.path().join("apps/Hello");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("struo.toml"),
        r#"[[target]]
name = "Runtime"
kind = "kotlin_jvm_library"

[[target]]
name = "Hello"
kind = "kotlin_jvm_binary"

[target.dependencies]
runtime_deps = ["./Runtime"]

[target.attrs]
main_class = "dev.struo.hello.MainKt"
"#,
    )
    .unwrap();
    let value = server(tmp.path().to_path_buf())
        .tool_get_target(&json!({ "target": "apps/Hello/Hello" }))
        .unwrap();
    assert_eq!(value["label"]["id"], "apps/Hello/Hello");
    assert_eq!(value["kind"], "kotlin_jvm_binary");
    assert_eq!(
        value["dependency_edges"]["runtime_deps"],
        json!(["apps/Hello/Runtime"])
    );
    assert_eq!(value["attrs"]["main_class"], "dev.struo.hello.MainKt");
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
                        "kind": "kotlin_jvm_binary",
                        "dependencies": { "runtime_deps": ["./Runtime"] },
                        "attrs": { "main_class": "dev.struo.hello.MainKt" }
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
    assert_eq!(target["kind"], "kotlin_jvm_binary");
    assert_eq!(
        target["dependency_edges"]["runtime_deps"],
        json!(["apps/Hello/Runtime"])
    );
}
