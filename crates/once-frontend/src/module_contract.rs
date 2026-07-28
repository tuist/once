use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModuleAuthoringContract {
    pub language: &'static str,
    pub registration: &'static str,
    pub declaration_source: String,
    pub schema_invariants: Vec<&'static str>,
    pub resolver_contract: Vec<ContractEntry>,
    pub context_fields: Vec<ContractEntry>,
    pub analysis_primitives: Vec<ContractEntry>,
    pub action_primitives: Vec<ContractEntry>,
    pub lint_contract: Vec<ContractEntry>,
    pub test_contract: Vec<ContractEntry>,
    pub maintenance_invariants: Vec<&'static str>,
    pub starter: &'static str,
    pub lint_starter: &'static str,
    pub lint_target_starter: &'static str,
    pub lint_adapter_starter: &'static str,
    pub normalized_lint_result_example: Value,
    pub test_starter: &'static str,
    pub test_target_starter: &'static str,
    pub normalized_test_result_example: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContractEntry {
    pub signature: &'static str,
    pub purpose: &'static str,
}

const LINT_STARTER: &str = r#"def _package_path(ctx, path):
    package = ctx["label"]["package"]
    if package:
        return package + "/" + path
    return path

def _scripted_lint_impl(ctx):
    lint_dir = ctx["build_dir"] + "/lint"
    report = lint_dir + "/report.sarif"
    results = lint_dir + "/lint_results.json"
    sources = glob(ctx["srcs"])
    config = glob(ctx["attr"].get("config") or [])
    data = glob(ctx["attr"].get("data") or [])
    program = _package_path(ctx, ctx["attr"]["program"])
    python = host_which(ctx["attr"].get("python") or "python3")
    identity = (
        "scripted_lint.v1\x00" +
        python + "\x00" +
        host_command([python, "--version"], merge_stderr = True).strip()
    )
    argv = [
        python,
        program,
        "--report", execution_path(report),
        "--analyzer", ctx["attr"]["analyzer"],
    ] + (ctx["attr"].get("args") or []) + sources
    findings_exit_code = ctx["attr"].get("findings_exit_code")
    if findings_exit_code == None:
        findings_exit_code = 1
    success_codes = [0]
    if findings_exit_code != 0:
        success_codes.append(findings_exit_code)
    inputs = sources + config + data + [program]

    if ctx["capability"] == "lint":
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [report],
            clean_paths = [report],
            create_dirs = [lint_dir],
            success_exit_codes = success_codes,
            toolchain_identity = identity,
            identifier = "scripted_lint:" + ctx["label"]["id"],
        )

    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "scripted_lint",
        "affected_inputs": inputs,
        "lint_info": {
            "schema": "once.lint_info.v1",
            "target": ctx["label"]["id"],
            "analyzer": {
                "type": ctx["attr"]["analyzer"],
                "display_name": ctx["attr"]["analyzer"],
                "metadata": {},
            },
            "command": {"argv": argv, "env": {}, "cwd": "."},
            "outputs": {
                "sarif": report,
                "results": results,
                "native_results": [],
                "logs": [],
            },
            "scope": {"requested": sources},
            "execution": {
                "cacheable": True,
                "run_from_workspace_root": True,
            },
            "metadata": {},
        },
    }

scripted_lint = target_kind(
    docs = "Runs a project-owned lint adapter and exposes normalized findings.",
    attrs = [
        attr("analyzer", "string", required = True, configurable = False),
        attr("program", "string", required = True, configurable = False),
        attr("python", "string", default = "\"python3\"", configurable = False),
        attr("config", "list<string>", default = "[]", configurable = False),
        attr("data", "list<string>", default = "[]", configurable = False),
        attr("args", "list<string>", default = "[]", configurable = False),
        attr("findings_exit_code", "int", default = "1", configurable = False),
    ],
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [tool("python", executables = ["python3", "python"])],
    impl = _scripted_lint_impl,
)
"#;

const LINT_TARGET_STARTER: &str = r#"[modules]
paths = ["modules/*.star"]

[[target]]
name = "lint"
kind = "scripted_lint"
srcs = ["src/**/*.txt"]

[target.attrs]
analyzer = "TODO checker"
program = "tools/lint_adapter.py"
findings_exit_code = 1
"#;

const LINT_ADAPTER_STARTER: &str = r#"import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--report", required=True)
parser.add_argument("--analyzer", required=True)
parser.add_argument("sources", nargs="*")
args = parser.parse_args()

findings = []
for source in args.sources:
    path = Path(source)
    for line_number, line in enumerate(path.read_text().splitlines(), start=1):
        if "TODO" not in line:
            continue
        findings.append({
            "ruleId": "todo",
            "level": "warning",
            "message": {"text": "Resolve this TODO before merging."},
            "locations": [{
                "physicalLocation": {
                    "artifactLocation": {"uri": source},
                    "region": {"startLine": line_number, "startColumn": 1},
                },
            }],
        })

report = {
    "version": "2.1.0",
    "runs": [{
        "tool": {"driver": {"name": args.analyzer, "rules": []}},
        "results": findings,
    }],
}
Path(args.report).write_text(json.dumps(report, indent=2))
raise SystemExit(1 if findings else 0)
"#;

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
            "Set allowed_values on string or target attributes when validation must reject values outside a fixed set before analysis.",
            "Set disallowed_values on string or target attributes when validation must reject a small reserved set before analysis. Surrounding whitespace is ignored when comparing values.",
            "Set implemented = False only for discoverable compatibility attributes that validation must reject until the target kind gives them behavior.",
            "attr.default is optional schema documentation and must be a string; it does not insert a runtime value. Implementations must use ctx[\"attr\"].get(...) when an optional attribute needs a fallback.",
            "Set configurable = False when analysis or output identity cannot safely vary through select.",
            "Dependency declarations name provider records accepted from ctx[\"deps\"] and ctx[\"deps_by_role\"], and implementations should consume provider fields instead of dependency target kind names.",
            "Set dep.min_count and dep.max_count when validation must reject a dependency role with too few or too many targets before analysis.",
            "An implementation must return a JSON-shaped provider record whose fields satisfy the target kind's declared provider contract.",
            "Resolver-owned attributes and synthetic target attributes must be declared in their target kind schemas.",
            "Modules are trusted analysis code. Host commands must be deterministic, must not mutate workspace sources or the build output tree, and may keep resolver scratch state only under .once/tmp. Fetching and build work belong in explicit workflows and actions.",
        ],
        resolver_contract: vec![
            entry(
                "resolver(ctx)",
                "Import an authoritative locked dependency graph before validation and scheduling.",
            ),
            entry(
                "ctx[\"files\"]",
                "Files selected by non-empty resolver_inputs, or srcs when resolver_inputs is empty or omitted, decoded as text and keyed relative to the owner package.",
            ),
            entry(
                "[{name, kind, deps, dependencies, srcs, visibility, attrs}]",
                "Compact resolver return form containing synthetic targets.",
            ),
            entry(
                "{targets, roots, attrs}",
                "Detailed resolver return form with owner dependency roots and typed owner metadata. Resolver attributes cannot replace values declared by the owner target.",
            ),
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
            entry(
                "ctx[\"configuration\"]",
                "Target operating system, architecture, and ordered select tokens configured by the workspace.",
            ),
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
            entry(
                "walk_files(root, excluded_paths = [], excluded_names = [])",
                "Walk a package-relative directory into sorted workspace file and symbolic-link paths while pruning exact root-relative paths and names.",
            ),
            entry("host_arch()", "Read the normalized host architecture."),
            entry("host_os()", "Read the normalized host operating system."),
            entry("host_env(name)", "Read one host environment variable."),
            entry("workspace_root()", "Read the absolute workspace root."),
            entry("host_which(name)", "Resolve a required executable."),
            entry("host_which_optional(name)", "Resolve an optional executable."),
            entry(
                "host_command(argv, env = {}, cwd = None, merge_stderr = False)",
                "Run a trusted discovery command whose arguments, environment, and working directory participate in analysis caching. It may write scratch state only under .once/tmp.",
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
            entry(
                "execution_path(path)",
                "Resolve a workspace-relative path against the local, sandbox, or remote execution root immediately before process launch.",
            ),
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
                "link_path(source, destination, identifier = None)",
                "Declare an uncached relative workspace link from an existing source without copying or caching the linked contents.",
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
                "write_archive(entries, output, sha256_output = None, format = \"tar\", inputs = [], identifier = None, cacheable = True)",
                "Declare a deterministic archive from explicit file, directory, and tree entries with fixed metadata.",
            ),
            entry(
                "cmd_args(args, use_arg_file = None)",
                "Build a structured argument list, optionally backed by an argument file.",
            ),
            entry(
                "run_action(argv, inputs = [], outputs = [], clean_paths = [], create_dirs = [], cwd = None, env = {}, toolchain_identity = None, identifier = None, cacheable = True, depends_on_prior_actions = True, stdout = None, stderr = None, sandbox = None, success_exit_codes = [0])",
                "Declare a direct executable invocation with explicit inputs, outputs, setup, caching, sandbox policy, and exit codes that indicate valid outputs. Use `once query validate-actions` to investigate filesystem contract drift without changing the sandbox policy.",
            ),
        ],
        lint_contract: vec![
            entry(
                "providers = [\"once_lint_info\"]",
                "Declare the reserved provider whenever a target returns a `lint_info` record for generic lint execution.",
            ),
            entry(
                "capability(\"lint\", [\"default\", \"lint_results\"])",
                "Expose the generic lint capability and its conventional output group.",
            ),
            entry(
                "provider[\"lint_info\"]",
                "Return schema, target, analyzer, command, outputs, scope, execution, and metadata. The lint starter shows the complete required shape.",
            ),
            entry(
                "run_action(..., success_exit_codes = [0, findings_code])",
                "Treat a documented findings exit code as valid completion so the analyzer report is captured and cached. Unexpected codes remain failures.",
            ),
            entry(
                "lint_info.outputs.sarif",
                "Name the declared Static Analysis Results Interchange Format report produced by the target actions.",
            ),
            entry(
                "lint_info.outputs.results",
                "Reserve the normalized `once.lint_results.v1` destination written by `once lint` after it reads the portable report.",
            ),
            entry(
                "lint_info.outputs.native_results",
                "List native analyzer reports retained for inspection when a second declared action converts them to the portable report.",
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
        lint_starter: LINT_STARTER,
        lint_target_starter: LINT_TARGET_STARTER,
        lint_adapter_starter: LINT_ADAPTER_STARTER,
        normalized_lint_result_example: normalized_lint_result_example(),
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

fn normalized_lint_result_example() -> Value {
    json!({
        "schema": "once.lint_results.v1",
        "target": "lint",
        "status": "completed",
        "complete": true,
        "summary": {
            "total": 1,
            "errors": 0,
            "warnings": 1,
            "notes": 0
        },
        "findings": [{
            "analyzer": "TODO checker",
            "rule_id": "todo",
            "severity": "warning",
            "message": "Resolve this TODO before merging.",
            "location": {
                "path": "src/example.txt",
                "line": 1,
                "column": 1
            }
        }],
        "artifacts": {
            "portable_report": ".once/out/lint/lint/report.sarif"
        }
    })
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
            .resolver_contract
            .iter()
            .any(|entry| entry.signature == "{targets, roots, attrs}"));
        assert!(contract
            .test_contract
            .iter()
            .any(|entry| entry.signature.contains("once_test_info")));
        assert!(contract
            .lint_contract
            .iter()
            .any(|entry| entry.signature.contains("once_lint_info")));
        assert!(contract
            .lint_starter
            .contains("success_exit_codes = success_codes"));
        assert!(contract.lint_starter.contains("once.lint_info.v1"));
        assert!(contract
            .lint_target_starter
            .contains("kind = \"scripted_lint\""));
        assert!(contract
            .lint_adapter_starter
            .contains("\"version\": \"2.1.0\""));
        assert_eq!(
            contract.normalized_lint_result_example["schema"],
            "once.lint_results.v1"
        );
        let lint_module = format!("{}\n{}", contract.declaration_source, contract.lint_starter);
        let engine = crate::analysis::AnalysisEngine::from_source(lint_module).unwrap();
        assert!(engine.target_kind_has_impl("scripted_lint"));
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
