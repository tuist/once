use serde_json::{json, Value};

/// Build the MCP `tools/list` wire projection from the shared catalog.
pub(super) fn tool_definitions(allow_run: bool) -> Vec<Value> {
    tool_catalog()
        .into_iter()
        .filter(|tool| allow_run || !tool_requires_allow_run(tool.name))
        .map(|tool| {
            let mut input_schema = tool.input_schema;
            if let Some(schema) = input_schema.as_object_mut() {
                schema.insert("additionalProperties".to_string(), Value::Bool(false));
            }
            json!({
                "name": tool.name,
                "description": format!("{}\n\n{}", tool.description, tool.long_description),
                "inputSchema": input_schema,
                "outputSchema": {
                    "type": "object",
                    "properties": { "result": {} },
                    "required": ["result"],
                    "additionalProperties": false
                },
                "annotations": tool_annotations(tool.name),
            })
        })
        .collect()
}

pub(super) fn tool_requires_allow_run(name: &str) -> bool {
    matches!(
        name,
        "once_run_tests"
            | "once_exec_script"
            | "once_build_target"
            | "once_validate_actions"
            | "once_run_target"
            | "once_start_target"
            | "once_runtime_status"
            | "once_runtime_logs"
            | "once_stop_runtime"
            | "once_apply_edit"
    )
}

fn tool_annotations(name: &str) -> Value {
    let read_only = name.starts_with("once_query_")
        || matches!(
            name,
            "once_list_target_kinds"
                | "once_get_target"
                | "once_fetch_external_source"
                | "once_validate_module"
                | "once_validate_target"
                | "once_validate_workspace"
                | "once_validate_script"
                | "once_runtime_status"
                | "once_runtime_logs"
        );
    let destructive = matches!(name, "once_apply_edit" | "once_stop_runtime");
    let idempotent = read_only
        || matches!(
            name,
            "once_build_target" | "once_runtime_status" | "once_runtime_logs"
        );
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        "openWorldHint": name == "once_fetch_external_source",
    })
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
            description: "List every declared target in the workspace, optionally filtered by target kind.",
            long_description: "Returns the same record shape as `once query targets --format json`: one entry per declared target with its canonical id, package, name, target kind, default dependencies, named dependency roles, and exposed capabilities. The optional `kind` argument narrows results to one target kind.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "description": "Restrict results to a target kind discovered through `once_list_target_kinds`."
                    }
                }
            }),
            example_return: "[\n  { \"id\": \"packages/core/Core\", \"package\": \"packages/core\", \"name\": \"Core\",\n    \"kind\": \"library\", \"deps\": [], \"dependency_edges\": {}, \"capabilities\": [\"build\"] },\n  { \"id\": \"apps/service/Service\", \"package\": \"apps/service\", \"name\": \"Service\",\n    \"kind\": \"application\", \"deps\": [\"packages/core/Core\"],\n    \"dependency_edges\": { \"plugins\": [\"tools/compiler/Plugin\"] },\n    \"capabilities\": [\"build\", \"run\"] }\n]",
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
                        "description": "Canonical target id, such as `apps/service/Service`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"id\": \"apps/service/Service\",\n  \"kind\": \"application\",\n  \"capabilities\": [\n    { \"name\": \"build\", \"output_groups\": [\"default\", \"package\"],\n      \"requires_outputs\": [] },\n    { \"name\": \"run\", \"output_groups\": [\"default\"],\n      \"requires_outputs\": [\"package\"] }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_query_schema",
            description: "Return the typed contract for a target kind: attributes, dep edges, providers, capabilities, source references, and runnable starters.",
            long_description: "Returns the target kind schema (the typed contract a target of that kind must match) as `once query schema <kind> --format json` would. The record carries the target kind's documentation, attribute list (with types, required flag, and whether the attribute is configurable), expected dep providers, emitted providers, exposed capabilities, external source concepts that can guide partial adoption, and a lightweight list of runnable starter examples. Use `once_query_example` to fetch the full file tree for a chosen example.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "description": "Target kind to introspect. Discover names with `once_list_target_kinds`."
                    }
                },
                "required": ["kind"]
            }),
            example_return: "{\n  \"kind\": \"library\",\n  \"docs\": \"Reusable library target...\",\n  \"attrs\": [\n    { \"name\": \"visibility\", \"ty\": \"string\", \"required\": true, \"configurable\": false }\n  ],\n  \"capabilities\": [ { \"name\": \"build\", \"output_groups\": [\"default\"], \"requires_outputs\": [] } ],\n  \"providers\": [\"linkable\", \"module\"],\n  \"source_references\": [\n    { \"system\": \"Example Build\", \"symbol\": \"example_library\",\n      \"url\": \"https://example.com/example_library\", \"use_when\": \"...\",\n      \"content_digest\": \"...\" }\n  ],\n  \"examples\": [\n    {\n      \"slug\": \"library-minimal\",\n      \"name\": \"Minimal library\",\n      \"use_when\": \"...\"\n    }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_query_example",
            description: "Return the complete file bundle for one target kind starter example.",
            long_description: "Returns the same record as `once query example <kind> <slug> --format json`: the selected example's slug, name, selection hint, and every text file a caller can copy to create the starter workspace. Example descriptors are discovered through `once_list_target_kinds` or `once_query_schema`; this tool fetches the file payload only after a caller chooses one.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "description": "Target kind that owns the example."
                    },
                    "slug": {
                        "type": "string",
                        "description": "Example slug from the target kind schema."
                    }
                },
                "required": ["kind", "slug"]
            }),
            example_return: "{\n  \"slug\": \"library-minimal\",\n  \"name\": \"Minimal library\",\n  \"use_when\": \"...\",\n  \"files\": [\n    { \"path\": \"packages/core/once.toml\", \"contents\": \"[[target]]\\nname = \\\"Core\\\"\\nkind = \\\"library\\\"\\n...\" }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_list_target_kinds",
            description: "List target kinds with their docs, external source references, and example slugs, optionally filtered by ecosystem or intent.",
            long_description: "Lightweight discovery entry point. Returns matching target kinds with documentation, external build-system concepts they can partially replace, and bundled starter examples. When the request names one or more ecosystems or runner families, include all of their names in the short `query` copied from the request. Once combines those specific matches while ignoring generic intent words, which lets a harness discover every native test integration in a mixed repository with one call. Omit the query when the intent is unknown. Call `once_query_schema` for the full contract of each chosen target kind. The matching command-line operation is `once query target-kinds --query <text> --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Short ecosystem, target-kind family, or intent text copied from the user's request."
                    }
                }
            }),
            example_return: "[\n  {\n    \"kind\": \"library\",\n    \"docs\": \"Reusable library target...\",\n    \"source_references\": [\n      { \"system\": \"Example Build\", \"symbol\": \"example_library\",\n        \"url\": \"https://example.com/example_library\", \"use_when\": \"...\",\n        \"content_digest\": \"...\" }\n    ],\n    \"examples\": [\n      { \"slug\": \"library-minimal\", \"name\": \"Minimal library\", \"use_when\": \"...\" }\n    ]\n  }\n]",
        },
        ToolDefinition {
            name: "once_query_module_contract",
            description: "Return the complete project-module authoring contract, generic analysis and action primitives, maintenance invariants, and a starter module.",
            long_description: "Use this when no discovered target kind covers an external rule or plugin. The result contains the exact Starlark declaration helpers, schema invariants, implementation context fields, generic host-analysis and action primitives, module registration snippet, maintenance loop, and a runnable starter. A coding harness can use it to author and maintain a project-local target kind without waiting for a built-in integration. The matching command-line operation is `once query module-contract --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            example_return: "{\n  \"language\": \"Starlark\",\n  \"registration\": \"[modules]\\npaths = [\\\"modules/*.star\\\"]\\n\",\n  \"schema_invariants\": [\"attr.default is optional schema documentation and must be a string...\"],\n  \"context_fields\": [\n    { \"signature\": \"ctx[\\\"attr\\\"]\", \"purpose\": \"Typed target attributes.\" }\n  ],\n  \"action_primitives\": [\n    { \"signature\": \"write_path(path, content)\", \"purpose\": \"Declare a portable file-writing action.\" },\n    { \"signature\": \"materialize_host_file(source, destination)\", \"purpose\": \"Snapshot a content-verified absolute host toolchain file into a workspace output.\" }\n  ],\n  \"starter\": \"def _generated_text_impl(ctx): ...\"\n}",
        },
        ToolDefinition {
            name: "once_fetch_external_source",
            description: "Fetch bounded UTF-8 source code, metadata, or documentation from a public HTTPS address.",
            long_description: "Fetches an authoritative external rule, plugin, registry record, or build-system reference for a coding harness to inspect before generating a local Once target kind. Only public HTTPS addresses are accepted, redirects are not followed, and response content is bounded to one mebibyte. The result includes the content, media type, digest, byte count, and truncation state. The matching command-line operation is `once query external-source <url> --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Public HTTPS address for external source code, metadata, or documentation."
                    },
                    "max_bytes": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 1_048_576,
                        "default": 262_144,
                        "description": "Maximum response bytes to return."
                    }
                },
                "required": ["url"]
            }),
            example_return: "{\n  \"url\": \"https://example.com/rules/example.rule\",\n  \"content_type\": \"text/plain\",\n  \"content_digest\": \"...\",\n  \"byte_count\": 4120,\n  \"truncated\": false,\n  \"content\": \"rule implementation...\"\n}",
        },
        ToolDefinition {
            name: "once_validate_module",
            description: "Validate a project-local Starlark module and return its target kind contracts before registration or execution.",
            long_description: "Reads one workspace-relative module file, evaluates it with the public Once declarations and generic primitives, and returns either its discovered target kind schemas or a structured repair diagnostic. Use it after a harness writes or updates an external-rule adaptation and before registering targets that depend on it. The matching command-line operation is `once query validate-module <path> --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative path to a Starlark module file."
                    }
                },
                "required": ["path"]
            }),
            example_return: "{\n  \"valid\": true,\n  \"path\": \"modules/generated_text.star\",\n  \"target_kinds\": [\n    { \"kind\": \"generated_text\", \"providers\": [\"generated_file\"], \"capabilities\": [ { \"name\": \"build\", \"output_groups\": [\"default\"] } ] }\n  ],\n  \"diagnostics\": []\n}",
        },
        ToolDefinition {
            name: "once_get_target",
            description: "Return one resolved target with its sources, dependency roles, typed attributes, capabilities, and providers.",
            long_description: "Returns the same `GraphTarget` record `once_query_targets` emits, scoped to one target id. Includes default dependencies in `deps`, named roles in `dependency_edges`, typed attribute values, exposed capabilities, emitted providers, and manifest diagnostics. Use this before editing a target to learn its current shape.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Canonical target id, such as `packages/core/Core`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"label\": { \"package\": \"packages/core\", \"name\": \"Core\", \"id\": \"packages/core/Core\" },\n  \"kind\": \"library\",\n  \"srcs\": [\"src/**/*.src\"],\n  \"deps\": [],\n  \"dependency_edges\": { \"plugins\": [\"tools/compiler/Plugin\"] },\n  \"attrs\": { \"visibility\": \"public\" },\n  \"capabilities\": [ { \"name\": \"build\", \"output_groups\": [\"default\"], \"requires_outputs\": [] } ],\n  \"providers\": [\"linkable\", \"module\"]\n}",
        },
        ToolDefinition {
            name: "once_query_tests",
            description: "List targets that expose Once's generic test capability.",
            long_description: "Returns every target with a `test` capability, including its target kind, dependencies, runner type when the target kind exposes `once_test_info`, labels, and normalized result path. Use this as the agent test discovery entry point before running or filtering tests.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            example_return: "[\n  {\n    \"id\": \"tests/unit\",\n    \"kind\": \"test_suite\",\n    \"deps\": [],\n    \"runner\": \"unit\",\n    \"labels\": [\"fast\"],\n    \"results_path\": \".once/out/tests/unit/test/test_results.json\"\n  }\n]",
        },
        ToolDefinition {
            name: "once_query_affected_tests",
            description: "Return test targets likely affected by a set of changed workspace paths.",
            long_description: "Maps changed paths to test targets using graph relationships and declared inputs. A test is affected when a changed path belongs to the test target itself or to one of its declared dependencies. Declared source patterns are matched without requiring the changed file to still exist. Changes to manifests, configured graph modules, and paths without a declared owner conservatively select every test. Selection does not depend on a particular test runner.",
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
            example_return: "[\n  {\n    \"id\": \"tests/unit\",\n    \"kind\": \"test_suite\",\n    \"reasons\": [\"changed test input `tests/unit/spec.src`\"]\n  }\n]",
        },
        ToolDefinition {
            name: "once_query_test_plan",
            description: "Create an immutable test plan without assigning work to runners.",
            long_description: "Returns the selection policy, normalized changed paths, unmatched paths, selected tests, and stable execution batches. The plan deliberately contains no local worker, remote provider, or fixed-job assignment, so scheduling can change without changing test identity or invalidating reusable results. Before a target's first complete run, or when its discovery inputs change, the plan intentionally contains one whole-target batch. Run that target once with `once_run_tests`, inspect `once_query_test_manifest`, then query the plan again to see automatic file or case batches. Pass `target` with a `test_unit` from the manifest to create an exact unit-filtered plan. Planning rejects targets that do not declare exact filtering and units absent from the persisted whole-target manifest. The matching command-line operations are `once query test-plan --changed-path <path> --format json` and `once query test-plan --target <target> --test-unit <unit> --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "changed_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workspace-relative changed paths. An empty list creates a full test plan."
                    },
                    "target": {
                        "type": "string",
                        "description": "Explicit canonical test target. When set, changed paths are ignored."
                    },
                    "test_unit": {
                        "type": "string",
                        "description": "One stable unit identifier from once_query_test_manifest. Requires target."
                    }
                }
            }),
            example_return: "{\n  \"schema\": \"once.test_plan.v1\",\n  \"id\": \"<stable digest>\",\n  \"selection\": {\n    \"schema\": \"once.test_selection.v1\",\n    \"policy\": { \"mode\": \"affected\", \"safety\": \"conservative\", \"evidence\": \"declared_graph\" },\n    \"changed_paths\": [\"src/lib.src\"],\n    \"unmatched_paths\": [],\n    \"tests\": [{ \"id\": \"tests/unit\", \"kind\": \"test_suite\", \"reasons\": [\"changed dependency `lib` input `src/lib.src`\"] }]\n  },\n  \"batches\": [{ \"id\": \"<stable digest>\", \"target\": \"tests/unit\", \"test_filters\": [] }]\n}",
        },
        ToolDefinition {
            name: "once_run_tests",
            description: "Run test targets by id, or run tests affected by changed workspace paths.",
            long_description: "Creates the same immutable plan as `once_query_test_plan`, then pulls stable batches from a shared local queue. Batches with longer historical uncached durations are queued first, and idle workers dynamically take the next batch. Explicit `target` or `targets` produce an exact plan; otherwise `changed_paths` drive conservative graph selection. With exactly one target, `test_unit` runs a stable unit returned by `once_query_test_manifest` when the target kind declares filtering support. `jobs` caps workers without changing plan or batch identity. The result's `plan` is the work that just executed. Its `next_plan` is recomputed after complete runs refresh discovery, so use that field to assess file or case batching for the next run. The result also includes actual schedule attempts and normalized test results. Failed tests are returned as normal tool content with `success: false` rather than a tool protocol error, so agents can inspect failures and iterate. The matching command-line operations are `once test --changed-path <path> --jobs <count> --format json` and `once test <target> --test-unit <unit> --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Single canonical target id to run, such as `tests/unit`."
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
                    },
                    "jobs": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 256,
                        "description": "Maximum local workers. Defaults to available host parallelism and never changes plan or batch identity."
                    },
                    "test_unit": {
                        "type": "string",
                        "description": "Run one stable unit identifier returned by once_query_test_manifest. Requires exactly one explicit target."
                    }
                }
            }),
            example_return: "{\n  \"plan\": { \"schema\": \"once.test_plan.v1\", \"id\": \"<executed plan digest>\", \"selection\": {}, \"batches\": [] },\n  \"next_plan\": { \"schema\": \"once.test_plan.v1\", \"id\": \"<next plan digest>\", \"selection\": {}, \"batches\": [] },\n  \"schedule\": {\n    \"schema\": \"once.test_schedule.v1\",\n    \"id\": \"<attempt-specific digest>\",\n    \"plan_id\": \"<executed plan digest>\",\n    \"strategy\": \"longest_estimated_duration_first_dynamic\",\n    \"workers\": 2,\n    \"attempts\": [{ \"batch_id\": \"<stable batch digest>\", \"placement\": \"local\", \"worker\": \"local-1\", \"status\": \"passed\" }]\n  },\n  \"runs\": [\n    {\n      \"batch_id\": \"<stable batch digest>\",\n      \"target\": \"tests/unit\",\n      \"exit_code\": 0,\n      \"success\": true,\n      \"record\": { \"target\": \"tests/unit\", \"capability\": \"test\" },\n      \"results\": { \"schema\": \"once.test_results.v1\", \"status\": \"passed\" },\n      \"stderr\": \"\"\n    }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_query_test_results",
            description: "Read normalized once.test_results.v1 results for a target.",
            long_description: "Reads the normalized result file produced by the target's `test` capability. This is the stable agent-facing interface for pass or fail summaries, case-level failures, attempts, and artifacts. Callers do not need to parse a runner's command output.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Canonical target id, such as `tests/unit`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"schema\": \"once.test_results.v1\",\n  \"target\": \"tests/unit\",\n  \"runner\": { \"type\": \"native\", \"metadata\": {} },\n  \"status\": \"passed\",\n  \"summary\": { \"total\": 1, \"passed\": 1, \"failed\": 0, \"skipped\": 0, \"flaky\": 0 },\n  \"cases\": [{ \"id\": \"tests/unit::case_name\", \"name\": \"case_name\", \"suite\": \"tests/unit\", \"status\": \"passed\", \"attempts\": [{ \"status\": \"passed\" }], \"runner_metadata\": {} }],\n  \"artifacts\": { \"logs\": [], \"native_results\": [] }\n}",
        },
        ToolDefinition {
            name: "once_query_test_manifest",
            description: "List stable test units discovered in a target's normalized results.",
            long_description: "Returns an immutable `once.test_manifest.v1` projection of the target's current normalized test results and its target-kind-declared listing and filtering support. When no results exist, the manifest reports `whole_target_fallback` with no units; run the whole target once to refresh discovery. Unit identifiers can be passed to `once_query_test_plan` or `once_run_tests` only when `case_filtering` is `runner_args`. The matching command-line operation is `once query test-manifest <target> --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Canonical target identifier, such as `tests/unit`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"schema\": \"once.test_manifest.v1\",\n  \"id\": \"<stable digest>\",\n  \"target\": \"tests/unit\",\n  \"runner\": \"native\",\n  \"source\": \"normalized_results\",\n  \"listing_supported\": true,\n  \"case_filtering\": \"runner_args\",\n  \"units\": [{ \"id\": \"tests/unit::case_name\", \"name\": \"case_name\", \"suite\": \"tests/unit\" }]\n}",
        },
        ToolDefinition {
            name: "once_query_test_attempts",
            description: "List persisted test batch attempts and measured durations.",
            long_description: "Returns actual schedule attempts recorded by `once_run_tests` or `once test --changed-path`. Each record connects a stable plan and batch to its local worker, status, cache state, timestamps, measured duration, and the estimate used for ordering. Use `target` to inspect one test target and `limit` to bound history. The matching command-line operation is `once query test-attempts --target <target> --limit <count> --format json`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Optional canonical test target id."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "default": 20,
                        "description": "Newest matching attempts to return."
                    }
                }
            }),
            example_return: "[\n  {\n    \"schema\": \"once.test_batch_attempt.v1\",\n    \"plan_id\": \"<stable plan digest>\",\n    \"batch_id\": \"<stable batch digest>\",\n    \"target\": \"tests/unit\",\n    \"placement\": \"local\",\n    \"worker\": \"local-1\",\n    \"duration_ms\": 842,\n    \"status\": \"passed\",\n    \"cache\": \"miss\"\n  }\n]",
        },
        ToolDefinition {
            name: "once_query_evidence",
            description: "List durable evidence records, optionally filtered by subject.",
            long_description: "Returns the same record shape as `once query evidence --format json`: durable action evidence captured after `once exec`, `once run`, `once build`, or `once test`. Pass `subject` to filter to one command action, target, or target capability, such as `cli` or `cli:test`. The tool returns the newest five matching records by default; set `limit` from 1 through 100 when more or fewer are useful. The matching command-line option is `once query evidence --limit <count>`. Evidence is historical provenance, not proof that inputs remain unchanged; run the relevant capability when a current result is required.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subject": {
                        "type": "string",
                        "description": "Optional subject id or subject-capability pair, such as `cli` or `cli:test`."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "default": 5,
                        "description": "Maximum number of newest matching records to return."
                    }
                }
            }),
            example_return: "[\n  {\n    \"schema\": \"once.evidence.v1\",\n    \"id\": \"8d65122cd9dcddc8d5d9a8458ff42a40fe3dd7acbd4e0563fd7f9e8fb19b0c44\",\n    \"kind\": \"action_result\",\n    \"subject\": { \"kind\": \"target\", \"id\": \"cli\", \"capability\": \"test\" },\n    \"status\": \"passed\",\n    \"action_digest\": \"0476bde2e7d8d1a64d9bd6f589ef5b443d0f60b71e2ad6f1c5bd7a2c4c41223f\",\n    \"input_digest\": \"8ed3f6ad685b959ead7022518e1af76cd816f8e8ec7ccd5f5814ccfb820e6a41\",\n    \"cache\": \"miss\",\n    \"exit_code\": 0,\n    \"stdout\": \"b439bb065d84034c2e7172c1709eb28797c9bd7f2c64c5d1a1d9c1118f6f9d7e\",\n    \"created_at_unix_ms\": 1812345678901\n  }\n]",
        },
        ToolDefinition {
            name: "once_validate_script",
            description: "Parse and validate an annotated script's cache contract.",
            long_description: "Reads a workspace-relative script, validates its shebang and `once` directives, and returns the parsed runtime, inputs, outputs, dependency scripts, fingerprints, environment names, working directory, remote policy, and output symlink policy. Put singular directives with quoted values directly after the shebang, for example `# once input \"input.txt\"` and `# once output \"output.txt\"`. Plural names, colon syntax, and unquoted paths are invalid. Invalid contracts return `{ valid: false, diagnostics: [...] }`, so callers can repair directive typos before execution. The matching command-line operation is `once query script <path>`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative annotated script path."
                    }
                },
                "required": ["path"]
            }),
            example_return: "{\n  \"valid\": true,\n  \"path\": \"scripts/build.sh\",\n  \"contract\": {\n    \"path\": \"scripts/build.sh\",\n    \"runtime\": \"sh\",\n    \"runtime_args\": [],\n    \"inputs\": [\"src/**\"],\n    \"outputs\": [\"dist/**\"],\n    \"needs\": [],\n    \"fingerprints\": [],\n    \"env_vars\": []\n  }\n}",
        },
        ToolDefinition {
            name: "once_exec_script",
            description: "Execute a validated annotated script through Once's action cache.",
            long_description: "Opt-in tool exposed only when the Model Context Protocol server starts with `once mcp --allow-run`. Validates the script contract before running the same path as `once exec --script`, materializes declared outputs, and returns captured streams, exit status, action digest, cache hit or miss state, and matching evidence. Invoke it twice with unchanged declared inputs to verify cache reuse.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative annotated script path."
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments passed to the script after its path."
                    }
                },
                "required": ["path"]
            }),
            example_return: "{\n  \"path\": \"scripts/build.sh\",\n  \"success\": true,\n  \"exit_code\": 0,\n  \"stdout\": \"built\\n\",\n  \"stderr\": \"\",\n  \"record\": {\n    \"action_digest\": \"0476bde2e7d8d1a64d9bd6f589ef5b443d0f60b71e2ad6f1c5bd7a2c4c41223f\",\n    \"cache\": \"hit\",\n    \"exit_code\": 0\n  },\n  \"evidence_subject\": \"0476bde2e7d8d1a64d9bd6f589ef5b443d0f60b71e2ad6f1c5bd7a2c4c41223f\",\n  \"evidence\": []\n}",
        },
        ToolDefinition {
            name: "once_build_target",
            description: "Build a target by running its generic `build` capability.",
            long_description: "This tool is available only when the server starts with `once mcp --allow-run`. It behaves like `once build <target> --format json`, including dependency traversal, target actions, cache policy, and output groups. The result includes the exit status, standard error, and standard output parsed as structured data when possible. A failed build is returned as normal tool content with `success: false` so agents can inspect diagnostics.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Target id to build, such as `apps/service/Service` or `./Service`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"target\": \"apps/service/Service\",\n  \"capability\": \"build\",\n  \"exit_code\": 0,\n  \"success\": true,\n  \"record\": {\n    \"target\": \"apps/service/Service\",\n    \"kind\": \"application\",\n    \"capability\": \"build\",\n    \"cache\": \"miss\",\n    \"outputs\": [\".once/out/apps/service/Service/package\"]\n  },\n  \"stderr\": \"\"\n}",
        },
        ToolDefinition {
            name: "once_validate_actions",
            description: "Run scripted graph actions in a private validation sandbox.",
            long_description: "Runs the same declared action analysis used by the graph, bypasses the action cache, inventories private and real-workspace filesystem changes, and returns structured input and output repairs. The matching command-line operation is `once query validate-actions <target> --capability <name>`. This tool requires `once mcp --allow-run` because it executes commands. Successful reads that leave no filesystem evidence remain a documented limitation of symlink-only validation.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Canonical target id." },
                    "capability": { "type": "string", "default": "build", "description": "Capability to validate." },
                    "action": { "type": "integer", "minimum": 0, "description": "Optional zero-based action index." }
                },
                "required": ["target"]
            }),
            example_return: "{\"valid\":false,\"target\":\"pkg/tool\",\"capability\":\"build\",\"actions_run\":1,\"diagnostics\":[{\"code\":\"undeclared_write\",\"target\":\"pkg/tool\",\"attribute\":\"outputs\",\"repairs\":[\"Declare this path as an output or stop writing it\"]}]}"
        },
        ToolDefinition {
            name: "once_run_target",
            description: "Run a target by executing its generic `run` capability.",
            long_description: "This tool is available only when the server starts with `once mcp --allow-run`. It behaves like `once run <target> --format json`, including prerequisite build outputs declared by the target's `run` capability. Set `visible` to request a visible interface when the target kind supports one. Uncacheable actions run again instead of replaying an action-cache hit. The result includes the exit status, standard error, and standard output parsed as structured data when possible.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Target id to run, such as `apps/service/Service` or `./Service`."
                    },
                    "visible": {
                        "type": "boolean",
                        "description": "Request a visible runtime interface when the target kind supports one."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"target\": \"apps/service/Service\",\n  \"capability\": \"run\",\n  \"exit_code\": 0,\n  \"success\": true,\n  \"record\": {\n    \"target\": \"apps/service/Service\",\n    \"kind\": \"application\",\n    \"capability\": \"run\",\n    \"cache\": \"bypass\",\n    \"outputs\": [\".once/out/apps/service/Service/run.json\"]\n  },\n  \"stderr\": \"\"\n}",
        },
        ToolDefinition {
            name: "once_start_target",
            description: "Start a target in a persisted runtime session and return its session id.",
            long_description: "This tool is available only when the server starts with `once mcp --allow-run`. It starts the target, saves its standard output and standard error under `.once/runtime/<session_id>/`, and returns immediately with the session record. Use the status, logs, and stop tools to follow the process.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Target id to start, such as `tools/demo/LaunchService` or `./LaunchService`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchService-123-1812345678901\",\n  \"target\": \"tools/demo/LaunchService\",\n  \"status\": \"starting\",\n  \"session_dir\": \".once/runtime/tools-demo-LaunchService-123-1812345678901\",\n  \"stdout\": \".once/runtime/tools-demo-LaunchService-123-1812345678901/stdout.log\",\n  \"stderr\": \".once/runtime/tools-demo-LaunchService-123-1812345678901/stderr.log\"\n}",
        },
        ToolDefinition {
            name: "once_runtime_status",
            description: "Return the latest persisted status for a runtime session.",
            long_description: "Reads `.once/runtime/<session_id>/session.json` and returns the latest status. Status values include `starting`, `running`, `stopping`, `stopped`, `exited`, and `failed`.",
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
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchService-123-1812345678901\",\n  \"target\": \"tools/demo/LaunchService\",\n  \"status\": \"running\",\n  \"pid\": 4242\n}",
        },
        ToolDefinition {
            name: "once_runtime_logs",
            description: "Read standard output or standard error records for a runtime session.",
            long_description: "Reads persisted line-oriented standard output and standard error records from a runtime session. Pass `source` to restrict the result to `stdout` or `stderr`, and pass a previous `cursor` to read only newer records.",
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
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchService-123-1812345678901\",\n  \"records\": [\n    { \"cursor\": \"stdout:000000000000\", \"source\": \"stdout\", \"level\": \"info\", \"message\": \"ready\" }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_stop_runtime",
            description: "Request that a runtime session stop.",
            long_description: "Requests that the process stop and updates the session status as the request is handled.",
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
            example_return: "{\n  \"session_id\": \"tools-demo-LaunchService-123-1812345678901\",\n  \"target\": \"tools/demo/LaunchService\",\n  \"status\": \"stopping\"\n}",
        },
        ToolDefinition {
            name: "once_validate_workspace",
            description: "Validate the complete workspace graph before execution.",
            long_description: "Loads every manifest and target kind schema, then checks target attributes, duplicate target ids, missing dependencies, dependency provider compatibility, source patterns, and dependency cycles. Returns stable diagnostics with target and attribute scope plus suggested repairs. Call this after materializing a starter or applying edits and before build, run, or test. The matching command-line operation is `once query validate-workspace`.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            example_return: "{\n  \"valid\": false,\n  \"target_count\": 1,\n  \"diagnostics\": [\n    {\n      \"code\": \"missing_dependency\",\n      \"message\": \"target `apps/service/Service` depends on missing target `packages/core/Core`\",\n      \"target\": \"apps/service/Service\",\n      \"attribute\": \"deps\",\n      \"repairs\": [\"Declare target `packages/core/Core` or remove it from `deps`\"]\n    }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_validate_target",
            description: "Validate a proposed `[[target]]` table against its target kind schema. Returns structured diagnostics instead of prose.",
            long_description: "Schema-only validation: checks that the target declares a known target kind, every named dependency role is declared by that kind, every required attribute is present, every declared attribute is known and has the declared type, and the target name is well-formed. The check is local; it does not resolve dependency references or read other manifests. Returns `{ valid: true }` on success or `{ valid: false, diagnostics: [...] }` where each diagnostic carries a stable `code`, the offending `target` id, the offending `attribute` when applicable, and `repairs` an agent can apply.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "object",
                        "description": "Raw `[[target]]` table shape with `name`, `kind`, optional `deps`, `dependencies`, `srcs`, and `attrs`."
                    }
                },
                "required": ["target"]
            }),
            example_return: "{\n  \"valid\": false,\n  \"diagnostics\": [\n    {\n      \"code\": \"missing_required_attr\",\n      \"message\": \"target kind `library` requires attribute `visibility`\",\n      \"target\": \"Core\",\n      \"attribute\": \"visibility\"\n    }\n  ]\n}",
        },
        ToolDefinition {
            name: "once_apply_edit",
            description: "Apply a batch of `create` / `update` / `delete` operations to one `once.toml` atomically.",
            long_description: "Reads the manifest at `<workspace>/<package>/once.toml`, creating it if needed, and applies the operations only if every operation succeeds. Returns `{ applied: true, path: <manifest path> }` on success or `{ applied: false, diagnostics: [...] }` with the structured diagnostic shape used by `once_validate_target`. A failed batch leaves the original file unchanged.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package": {
                        "type": "string",
                        "description": "Package directory relative to the workspace root, such as `packages/core`. Use `\"\"` for the root manifest."
                    },
                    "operations": {
                        "type": "array",
                        "description": "Ordered list of operations. Each is `{ op: \"create\", target: {...} }`, `{ op: \"update\", target_name: \"...\", set: {...} }`, or `{ op: \"delete\", target_name: \"...\" }`.",
                        "items": { "type": "object" }
                    }
                },
                "required": ["package", "operations"]
            }),
            example_return: "{\n  \"applied\": true,\n  \"path\": \"packages/core/once.toml\"\n}",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_examples_stay_toolchain_neutral() {
        let catalog_text = tool_catalog()
            .into_iter()
            .flat_map(|tool| [tool.description, tool.long_description, tool.example_return])
            .collect::<Vec<_>>()
            .join("\n");

        for forbidden in [
            "android",
            "apple_",
            "cargo",
            "crates.io",
            "ios",
            "jvm",
            "npm",
            "swift",
            "xcode",
        ] {
            assert!(
                !catalog_text.to_ascii_lowercase().contains(forbidden),
                "catalog should not hardcode `{forbidden}` examples"
            );
        }
    }
}
