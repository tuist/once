use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModuleAuthoringContract {
    pub language: &'static str,
    pub registration: &'static str,
    pub declaration_source: String,
    pub schema_invariants: Vec<&'static str>,
    pub context_fields: Vec<ContractEntry>,
    pub analysis_primitives: Vec<ContractEntry>,
    pub action_primitives: Vec<ContractEntry>,
    pub test_contract: Vec<ContractEntry>,
    pub maintenance_invariants: Vec<&'static str>,
    pub starter: &'static str,
    pub test_starter: &'static str,
    pub test_target_starter: &'static str,
    pub normalized_test_result_example: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContractEntry {
    pub signature: &'static str,
    pub purpose: &'static str,
}

#[must_use]
pub fn module_authoring_contract() -> ModuleAuthoringContract {
    let common = crate::modules::common_module_source();
    let declaration_source = common
        .split_once("\ndef _ends_with")
        .map_or(common, |(public, _)| public)
        .trim()
        .to_string();
    ModuleAuthoringContract {
        language: "Starlark",
        registration: "[modules]\npaths = [\"modules/*.star\"]\n",
        declaration_source,
        schema_invariants: vec![
            "Supported attribute types are string, bool, int, float, list<string>, map<string,string>, target, and select values for configurable attributes.",
            "attr.default is optional schema documentation and must be a string; it does not insert a runtime value. Implementations must use ctx[\"attr\"].get(...) when an optional attribute needs a fallback.",
            "Set configurable = False when analysis or output identity cannot safely vary through select.",
            "Dependency declarations name provider records accepted from ctx[\"deps\"] and ctx[\"deps_by_role\"], and implementations should consume provider fields instead of dependency target kind names.",
            "An implementation must return a JSON-shaped provider record whose fields satisfy the target kind's declared provider contract.",
        ],
        context_fields: vec![
            entry("ctx[\"label\"]", "Package, name, and stable target id."),
            entry("ctx[\"attr\"]", "Typed target attributes."),
            entry("ctx[\"srcs\"]", "Declared source patterns."),
            entry("ctx[\"deps\"]", "Provider records returned by dependencies."),
            entry(
                "ctx[\"deps_by_role\"]",
                "Provider records grouped by target-kind-defined dependency role, including deps.",
            ),
            entry("ctx[\"build_dir\"]", "Workspace-relative durable output directory."),
            entry("ctx[\"scratch_dir\"]", "Workspace-relative action-private directory."),
            entry("ctx[\"capability\"]", "Capability being analyzed."),
            entry("ctx[\"run\"][\"visible\"]", "Whether a visible runtime was requested."),
            entry(
                "ctx[\"test\"][\"filters\"]",
                "Stable semantic test-unit identifiers requested for this test execution.",
            ),
            entry(
                "ctx[\"test\"][\"batch_id\"]",
                "Stable batch identifier for isolating outputs during parallel test execution, or None for a whole-target execution.",
            ),
        ],
        analysis_primitives: vec![
            entry("glob(patterns)", "Expand package source patterns into sorted workspace paths."),
            entry("host_arch()", "Read the normalized host architecture."),
            entry("host_os()", "Read the normalized host operating system."),
            entry("host_env(name)", "Read one host environment variable."),
            entry("workspace_root()", "Read the absolute workspace root."),
            entry("host_which(name)", "Resolve a required executable."),
            entry("host_which_optional(name)", "Resolve an optional executable."),
            entry(
                "host_command(argv, env = {}, merge_stderr = False)",
                "Run a discovery command whose arguments and environment participate in analysis caching.",
            ),
            entry("host_file_exists(path)", "Test whether a host file exists."),
            entry("host_file_read(path)", "Read a host text file during analysis."),
            entry("host_file_sha256(path)", "Digest a host file used during analysis."),
            entry("host_file_contains(path, needle)", "Search a host text file."),
            entry("host_read_dir(path)", "List sorted names in a host directory."),
            entry("json_decode(source)", "Decode structured JSON data for a resolver."),
            entry("toml_decode(source)", "Decode structured TOML data for a resolver."),
        ],
        action_primitives: vec![
            entry("declare_output(name)", "Reserve a durable target output path."),
            entry("write_path(path, content)", "Declare a portable file-writing action."),
            entry(
                "copy_path(source, destination, kind = \"file\", inputs = [], toolchain_identity = None, identifier = None, cacheable = True)",
                "Declare a portable file or directory copy action.",
            ),
            entry(
                "materialize_host_file(source, destination)",
                "Snapshot a content-verified absolute host toolchain file into a workspace output.",
            ),
            entry(
                "prepare_path(path, kind, identifier = None)",
                "Declare uncached path removal or directory creation when standalone preparation is required.",
            ),
            entry(
                "write_tree_digest(root, output, include_suffixes = [], inputs = [], identifier = None, cacheable = True)",
                "Declare a deterministic workspace tree digest action.",
            ),
            entry(
                "cmd_args(args, use_arg_file = None)",
                "Build a structured argument list, optionally backed by an argument file.",
            ),
            entry(
                "run_action(argv, inputs = [], outputs = [], clean_paths = [], create_dirs = [], cwd = None, env = {}, toolchain_identity = None, identifier = None, cacheable = True, depends_on_prior_actions = True, stdout = None, stderr = None, sandbox = None)",
                "Declare a direct executable invocation with explicit inputs, outputs, setup, caching, and sandbox policy. Use sandbox = \"validate\" for an uncached filesystem contract probe that returns structured repairs without copying outputs back.",
            ),
        ],
        test_contract: vec![
            entry(
                "providers = [\"once_test_info\"]",
                "Declare the reserved provider whenever a target returns a `test_info` record for the generic test discovery and execution surfaces.",
            ),
            entry(
                "capability(\"test\", [\"default\", \"test_results\", \"logs\"])",
                "Expose the generic test capability and its conventional output groups.",
            ),
            entry(
                "provider[\"test_info\"]",
                "Return `schema`, `target`, `runner`, `command`, `outputs`, `listing`, `filtering`, `sharding`, `retries`, `execution`, `labels`, and `metadata`. The test starter shows the complete required shape.",
            ),
            entry(
                "provider[\"test_discovery_inputs\"]",
                "Optionally list the workspace files whose contents can change discovered test identities. Once fingerprints them before reusing a manifest.",
            ),
            entry(
                "ctx[\"build_dir\"] + \"/test[/batches/<batch_id>]/test_results.json\"",
                "Write normalized results under a batch-isolated directory when batch_id is present. The record uses schema `once.test_results.v1` and contains target, runner, status, summary, cases, and artifacts.",
            ),
            entry(
                "case.id = ctx[\"label\"][\"id\"] + \"::\" + semantic_name",
                "Use stable target-qualified unit identifiers. Each case also contains name, suite, status, attempts, and runner_metadata.",
            ),
            entry(
                "filtering.case_filtering = \"runner_args\"",
                "Declare this only when every value in `ctx[\"test\"][\"filters\"]` is translated exactly into native runner arguments. Otherwise declare `unsupported` and ignore no requested filters.",
            ),
            entry(
                "sharding = {\"supported\": True, \"granularity\": \"file\"}",
                "Enable automatic batching only when exact filters and batch-isolated outputs are implemented. Granularity is target, file, or case.",
            ),
            entry(
                "runner exit status and results.status",
                "The runner exits unsuccessfully when the test run fails and writes a matching failed normalized record when possible. A successful process status must never normalize a runner crash or incomplete terminal result as passed.",
            ),
        ],
        maintenance_invariants: vec![
            "Fetch and inspect the external rule or plugin that is authoritative for the requested behavior.",
            "Model only the requested target and its necessary dependency closure; leave unrelated nodes in the source build.",
            "Record the upstream system, symbol, web address, adoption intent, and the content digest of every complete fetched source with source_reference(...). Re-fetch and compare that digest before maintaining the adaptation.",
            "Keep ecosystem interpretation in the project module; use only generic Once primitives in the executor.",
            "Declare command arguments, inputs, outputs, cleanup, and directories explicitly instead of hiding setup in a shell command.",
            "Validate the module, validate target tables, validate the workspace, execute the requested capability, and inspect fresh evidence.",
        ],
        starter: r#"def _generated_text_impl(ctx):
    out = declare_output(ctx["attr"]["output"])
    write_path(out, "\n".join(ctx["attr"]["lines"]) + "\n")
    return {
        "label_id": ctx["label"]["id"],
        "generated_file": out,
    }

generated_text = target_kind(
    docs = "Writes declared lines to a generated text file.",
    attrs = [
        attr("output", "string", required = True, configurable = False),
        attr("lines", "list<string>", required = True),
    ],
    providers = ["generated_file"],
    capabilities = [capability("build", ["default"])],
    impl = _generated_text_impl,
)
"#,
        test_starter: r#"def _project_path(ctx, path):
    package = ctx["label"]["package"]
    if package:
        return package + "/" + path
    return path

def _scripted_test_impl(ctx):
    batch_id = ctx["test"]["batch_id"]
    test_dir = ctx["build_dir"] + "/test" + (("/batches/" + batch_id) if batch_id else "")
    results = test_dir + "/test_results.json"
    log = test_dir + "/scripted-test.log"
    program = _project_path(ctx, ctx["attr"]["program"])
    tool = host_which(ctx["attr"].get("tool") or "python3")
    tool_identity = tool + "\x00" + host_command([tool, "--version"], merge_stderr = True)
    filters = ctx["test"]["filters"]
    argv = [
        tool,
        program,
        "--once-results", results,
        "--once-log", log,
        "--once-target", ctx["label"]["id"],
    ] + (ctx["attr"].get("args") or [])
    for test_filter in filters:
        argv.extend(["--once-test-unit", test_filter])
    test_info = {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {
            "type": "scripted",
            "display_name": "Script-backed test",
            "metadata": {},
        },
        "command": {"argv": argv, "env": {}, "cwd": "."},
        "outputs": {
            "results": results,
            "logs": [log],
            "native_results": [],
            "coverage": [],
        },
        "listing": {"supported": True, "strategy": "normalized_results"},
        "filtering": {"case_filtering": "runner_args"},
        "sharding": {"supported": False},
        "retries": {"supported": False, "default_attempts": 1},
        "execution": {
            "cacheable": True,
            "timeout_ms": ctx["attr"].get("timeout_ms"),
            "run_from_workspace_root": True,
        },
        "labels": ctx["attr"].get("labels") or [],
        "metadata": {},
    }
    if ctx["capability"] == "test":
        run_action(
            argv = argv,
            inputs = glob(ctx["srcs"]),
            outputs = [results, log],
            create_dirs = [test_dir],
            toolchain_identity = tool_identity,
            identifier = "scripted_test:" + ctx["label"]["id"],
        )
    return {
        "label_id": ctx["label"]["id"],
        "test_discovery_inputs": glob(ctx["srcs"]),
        "test_info": test_info,
    }

scripted_test = target_kind(
    docs = "Runs a script-backed test adapter that writes normalized Once results.",
    attrs = [
        attr("program", "string", required = True, docs = "Package-relative adapter program that is also included in srcs.", configurable = False),
        attr("tool", "string", default = "\"python3\"", docs = "Host executable used to run the adapter.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Arguments passed before Once filter arguments.", configurable = False),
        attr("labels", "list<string>", default = "[]", docs = "Labels exposed through test discovery."),
        attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
    ],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    impl = _scripted_test_impl,
)
"#,
        test_target_starter: r#"[modules]
paths = ["modules/*.star"]

[[target]]
name = "scripted_tests"
kind = "scripted_test"
srcs = ["tests/test_adapter.py"]

[target.attrs]
program = "tests/test_adapter.py"
labels = ["scripted"]
"#,
        normalized_test_result_example: json!({
            "schema": "once.test_results.v1",
            "target": "scripted_tests",
            "runner": {
                "type": "scripted",
                "metadata": {}
            },
            "status": "passed",
            "summary": {
                "total": 1,
                "passed": 1,
                "failed": 0,
                "skipped": 0,
                "flaky": 0
            },
            "cases": [{
                "id": "scripted_tests::case-name",
                "name": "case-name",
                "suite": "scripted",
                "status": "passed",
                "attempts": [{ "status": "passed" }],
                "runner_metadata": {}
            }],
            "artifacts": {
                "logs": [".once/out/scripted_tests/test/scripted-test.log"],
                "native_results": []
            }
        }),
    }
}

const fn entry(signature: &'static str, purpose: &'static str) -> ContractEntry {
    ContractEntry { signature, purpose }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_exposes_declarations_actions_and_maintenance_loop() {
        let contract = module_authoring_contract();
        assert!(contract.declaration_source.contains("def target_kind("));
        assert!(contract
            .declaration_source
            .contains("def source_reference("));
        assert!(contract
            .action_primitives
            .iter()
            .any(|entry| entry.signature.starts_with("run_action(")));
        assert!(contract
            .action_primitives
            .iter()
            .any(|entry| entry.signature.starts_with("materialize_host_file(")));
        assert!(contract
            .schema_invariants
            .iter()
            .any(|invariant| invariant.contains("attr.default")));
        assert!(contract
            .test_contract
            .iter()
            .any(|entry| entry.signature.contains("once_test_info")));
        assert!(contract.test_starter.contains("ctx[\"test\"][\"filters\"]"));
        assert!(contract.test_starter.contains("once.test_info.v1"));
        assert!(contract
            .test_target_starter
            .contains("kind = \"scripted_test\""));
        assert_eq!(
            contract.normalized_test_result_example["schema"],
            "once.test_results.v1"
        );
        let test_module = format!("{}\n{}", contract.declaration_source, contract.test_starter);
        let engine = crate::analysis::AnalysisEngine::from_source(test_module).unwrap();
        assert!(engine.target_kind_has_impl("scripted_test"));
        assert!(contract
            .maintenance_invariants
            .iter()
            .any(|invariant| invariant.contains("dependency closure")));
    }
}
