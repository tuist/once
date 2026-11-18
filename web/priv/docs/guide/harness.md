# Coding Harnesses

Struo can be the automation substrate for a coding harness. The harness does not
need Struo-specific prompt templates or built-in target kinds for every
toolchain. It connects to the
[Model Context Protocol](https://modelcontextprotocol.io/) server, discovers
the available target kinds and examples, creates or edits workspace files,
validates the loaded graph, runs a capability, and checks the resulting
evidence.

## Connect Struo

Start the server in the project directory with editing and execution enabled:

```json
{
  "mcpServers": {
    "struo": {
      "command": "mise",
      "args": [
        "-C",
        "/absolute/path/to/project",
        "exec",
        "--",
        "struo",
        "mcp",
        "--workspace",
        "/absolute/path/to/project",
        "--allow-run"
      ]
    }
  }
}
```

The configuration above can be stored as a Claude Code project server. The
same connection can be registered directly from either harness:

```sh
# Codex
codex mcp add struo -- mise -C /absolute/path/to/project exec -- \
  struo mcp --workspace /absolute/path/to/project --allow-run

# Claude Code, private to the current project
claude mcp add --scope local struo -- mise -C /absolute/path/to/project exec -- \
  struo mcp --workspace /absolute/path/to/project --allow-run
```

These commands configure the server, not the harness approval policy. For a
headless run, also give the harness permission to edit the project and execute
the registered tools inside an appropriate sandbox. Otherwise the client can
reject `struo_apply_edit` or `struo_run_tests` even though the server advertises
them.

Omit `--allow-run` when the harness should only inspect the graph and project
memory. In that mode Struo does not advertise tools that edit, build, test, run,
or start processes.

During initialization the server returns cross-tool workflow instructions.
Every advertised tool also carries a strict input schema, behavioral hints, an
output schema, and structured content. A harness can therefore plan from the
live protocol contract instead of scraping this guide.

Call `struo_query_workspace` before using filesystem tools. It returns the exact
root configured for the server, whether the root manifest exists, the current
target count, any graph loading error, and the next relevant calls. This avoids
writing files under the harness working directory when the server was
configured for a different directory.

## Run Headlessly with Codex and Claude

Headless clients cannot stop to approve a tool call. Struo marks
`struo_apply_edit` as destructive because one edit operation can delete a target.
Use each client's non-interactive permission mode only inside a workspace whose
contents and server configuration you trust.

Run Codex with a turn-local Struo server configuration:

```sh
codex exec \
  --dangerously-bypass-approvals-and-sandbox \
  -C /absolute/path/to/project \
  -c 'mcp_servers.struo.command="mise"' \
  -c 'mcp_servers.struo.args=["exec","--","struo","mcp","--workspace","/absolute/path/to/project","--allow-run"]' \
  'Use the Struo Model Context Protocol server to create, validate, and run this project.'
```

Keep the bypass scoped to the project with `-C`. Omit it for an interactive
session where Codex can ask before applying an edit.

Claude Code accepts its server configuration from a file such as
`struo-server.json`:

```json
{
  "mcpServers": {
    "struo": {
      "command": "mise",
      "args": [
        "exec",
        "--",
        "struo",
        "mcp",
        "--workspace",
        "/absolute/path/to/project",
        "--allow-run"
      ]
    }
  }
}
```

Then run Claude from that project:

```sh
claude -p \
  --dangerously-skip-permissions \
  --mcp-config ./struo-server.json \
  --strict-mcp-config \
  'Use the Struo Model Context Protocol server to create, validate, and run this project.'
```

Place `--strict-mcp-config` after the configuration path. The Claude option
accepts more than one path, so the following option also makes the boundary
between configuration paths and the prompt explicit.

For both clients, launch Struo through `mise exec` so graph actions inherit the
project's pinned compilers and runtime environment. Query canonical target
identifiers with `struo_query_targets`; execution tools also resolve `./Target`
from the configured workspace even when the server process was started
elsewhere.

## Create and Run a Typed Graph

Use this loop for requests such as “build an Android app with Struo” or “add a
Rust command-line tool with Struo”:

1. Call `struo_query_workspace` and use its `root` for every filesystem
   operation.
2. Call `struo_list_target_kinds` and choose a kind whose description and
   `use_when` text match the request. When the request names an ecosystem or
   target-kind family, include it in `query` so it takes priority over generic
   intent words and unrelated target kinds do not consume harness context.
3. Call `struo_query_schema` for its complete attribute, dependency, provider,
   capability, and example contract. When several starters match, prefer the
   narrowest `use_when` description that satisfies the request. Minimal
   starters avoid unrelated targets and dependencies.
4. For an unchanged starter, call `struo_materialize_example`. It copies the
   complete bundle without returning file contents through the model, refuses
   all writes when any destination conflicts, and returns canonical targets,
   workspace validation, and suggested next calls. Use `struo_query_example`
   only when the harness needs to inspect or adapt the starter files itself.
5. Call `struo_query_targets` to obtain canonical target identifiers from the
   loaded workspace.
6. Call `struo_validate_workspace`. Repair every structured diagnostic and
   repeat until `valid` is `true`.
7. Call `struo_query_capabilities` for the chosen target and invoke
   `struo_build_target`, `struo_run_target`, or the testing tools as appropriate.
8. Check `success`, `exit_code`, and the structured output record. Run calls
   include bounded `captured_stdout` and `captured_stderr` records when the
   target declares those logs.
9. Call `struo_query_evidence` with the returned target or capability subject to
   retain and inspect the durable result.

Use `struo_validate_target` before creating a proposed target table from
scratch. After `struo_apply_edit`, always use `struo_validate_workspace` because
table validation cannot detect missing dependencies, incompatible providers,
unmatched source patterns, or dependency cycles.

## Adopt Existing Test Runners

Use this loop when a repository already runs tests through pytest, Ruby
Specification, Minitest, Vitest, Jest, or another native runner:

1. Inspect the existing package manifests, test configuration, and documented
   test commands. Keep each native runner as the source of truth.
2. Call `struo_list_target_kinds` once with the exact runner names in `query`,
   such as `pytest vitest minitest`. The result combines all matching runner
   families without loading unrelated target kinds.
3. Call `struo_query_schema` for every matching test kind. Build one target
   table per coherent test suite, call `struo_validate_target` for each table,
   then create them with one `struo_apply_edit` call.
4. Use workspace-relative runtime paths when the repository owns an
   environment. A pytest target automatically detects `.venv/bin/python` when
   `python` is omitted. Python, Ruby, and Node.js target kinds also accept
   explicit workspace-relative paths, names on the executable search path,
   and absolute paths.
5. Call `struo_validate_workspace`, then run every new target completely once
   with `struo_run_tests`. The initial plan intentionally contains one batch per
   target so the native runners can establish complete manifests. Read
   `next_plan` in the completed response for the file or case batches that the
   next run will use. The `plan` field describes the run that just finished.
6. Call `struo_query_test_manifest` when stable unit identifiers are needed for
   exact execution. Increase `jobs` for later runs after confirming the
   expected batches in `next_plan`.
7. Repeat an unchanged run and inspect its cache decisions. Query an affected
   plan with representative changed paths, and use one manifest unit to prove
   exact execution when the target kind supports it.

The [Testing and Scheduling](/guide/graph/testing) guide provides runner-first
declarations and explains the safety boundary between affected selection and
automatic batching.

For an Android application, the live catalog leads the harness to
`android_binary` and its runnable starter. The harness creates the returned
manifest and sources with `struo_materialize_example`, validates the complete
graph, discovers the starter's canonical target identifier, builds it, and
checks the Android application package output. Android-specific behavior
remains in the target kind, so the harness follows the same loop for other
ecosystems.

## Adopt an Unfamiliar External Rule

The built-in catalog is a shortcut, not the boundary of what a harness can
model. When a requested node comes from an unsupported rule, plugin, or build
system, use this loop:

1. Identify the authoritative external symbol from the source build manifest,
   registry, plugin declaration, or checked-in rule.
2. Call `struo_fetch_external_source` with a public HTTPS address for its source
   code, registry record, or reference. The response is bounded and includes a
   content digest, so the harness can record exactly what it interpreted.
3. Call `struo_query_module_contract`. It returns the exact Starlark declaration
   helpers, schema invariants, implementation context, generic analysis and
   action primitives, the reserved test provider and normalized result
   contract, maintenance invariants, registration snippet, and build and test
   starter modules. Schema defaults are descriptive strings; implementations
   provide optional runtime fallbacks with `ctx["attr"].get(...)`.
4. Write a project-local target kind that represents only the requested node
   and the dependency closure needed to run it. Keep unrelated nodes in the
   existing build system.
5. Add `source_reference(...)` metadata for every external concept the local
   target kind interprets. Record the system, symbol, public address, reason
   this mapping applies, and the returned content digest when the fetch was not
   truncated. During maintenance, re-fetch the same source and compare its
   digest before changing the local graph.
6. Call `struo_validate_module` before registering or using the module. Repair
   its structured diagnostic until the returned target kind contracts match
   the intended graph boundary.
7. Register the module, create its target tables, then continue with target,
   workspace, execution, output, and evidence validation.

This lets the harness maintain the project-specific graph instead of waiting
for Struo or an external rules package to encode every possible integration.
The Struo executor stays ecosystem-neutral. The local module translates the
upstream behavior into explicit inputs, outputs, tools, providers, and portable
actions.

The same path works with a rule from the
[Bazel Central Registry](https://registry.bazel.build/), a
[Buck2 prelude rule](https://buck2.build/docs/prelude/rules/), a
[Gradle plugin](https://plugins.gradle.org/), or another public ecosystem. Struo
does not claim that an arbitrary fetched rule is compatible. The harness must
interpret the source, validate the generated module, and prove the requested
capability by executing it and checking its outputs.

## Run an Annotated Script

Use scripts when the requested automation is one executable action rather than
a typed multi-target graph:

1. Create or update the script with `struo` declarations for inputs, outputs,
   environment names, working directory, dependency scripts, and fingerprints.
2. Call `struo_validate_script` with the workspace-relative path. Repair any
   diagnostic before execution.
3. Call `struo_exec_script`. Check `success`, `exit_code`, captured streams,
   action digest, cache state, and the returned evidence subject.
4. Inspect declared output files through the harness workspace.
5. When cache behavior matters, call `struo_exec_script` again without changing
   declared inputs and require a cache hit.

This route executes the same annotated script contract as `struo exec --script`.
It also materializes declared outputs when a prior action result is reused.

## Compare Fresh Executions

Cache reuse and repeated execution answer different questions. To check for
observed divergence, set `verify_reproducible: true` on `struo_build_target` or
`struo_exec_script`. Both require `struo mcp --allow-run` and authorization to
execute the selected work twice, including its dependencies. For example:

```json
{
  "name": "struo_exec_script",
  "arguments": {
    "path": "scripts/build.sh",
    "verify_reproducible": true
  }
}
```

This returns a `struo.verification.v1` report instead of the normal cached
execution response. Each trial starts from the same whole-workspace snapshot
at the same private path, with a separate, initially empty local Struo cache.
Script dependencies and fingerprints run in each trial. Trial artifacts are
retained without replacing outputs in the original workspace.

Read `success` for the check's pass/fail decision, then inspect
`execution_succeeded`, `artifacts_match`, and `logs_match` separately. Identical
failures never pass. With declared artifacts, log-only drift is advisory;
without artifacts, logs determine the comparison. Do not substitute the stricter
`reproducible` field for `success`, because it also requires matching logs.

Call `struo_query_verification` with the returned `id` to read the same retained
report without executing anything again. This inspection tool is also available
on a read-only server. It matches `struo query verification <ID> --format json`.
Use `trial_directories` and `artifact_differences` to inspect the retained files
instead of guessing from log differences. These local reports are not published
as durable project evidence automatically.

Report a passing result as “no divergence observed in two trials,” not proof of
reproducibility. Host toolchains, host tool caches, absolute paths, network
services, and external side effects are not isolated. Clock, randomness, and
scheduling behavior are not perturbed or controlled. See the
[report reference](/reference/mcp/#verification-reports) for the JSON contract
and [script guide](/guide/scripted/caching#check-repeated-execution) for snapshot
limits, retained files, local-only execution, and network policy.

## Use Project Memory Safely

Struo evidence is durable provenance for completed actions. It records the
subject, status, action and input digests, cache decision, exit code, captured
stream digests, and creation time.

Evidence is historical. A prior passing record does not prove that current
inputs are unchanged. Use it to understand what ran and to correlate results,
then invoke the relevant build, run, test, or script capability when the user
needs a current result. The action cache decides whether that invocation can be
reused safely.

## Follow Build and Test Progress

A harness that runs `struo build` or `struo test` in a shell can show progress
while the command runs. Set `STRUO_PROGRESS` to a file path, and Struo appends one
JSON object per line to that file: a snapshot of the run so far, written at
most once a second when something changed, and a final snapshot when the run
ends. Read only the last complete line.

```json
{"v":1,"command":"build","target":"app","state":"running",
 "planned":40,"done":12,"cached":9,"failed":0,"running":["lib"],
 "tests":{"passed":0,"failed":0,"skipped":0},"elapsed_ms":5321}
```

| Field | Meaning |
| --- | --- |
| `v` | Format version, `1`. A new version means a change older readers cannot interpret. |
| `command` | `build` or `test`. |
| `target` | The target the run was asked for. Empty for a scheduled `struo test` without one target. |
| `state` | `running`, `succeeded`, or `failed`. |
| `planned` | Targets the run will execute, or test batches for a scheduled `struo test`. Zero until planning ends. |
| `done`, `cached`, `failed` | Finished targets or batches, those restored from cache, and those that failed. |
| `running` | Up to five targets in progress. |
| `tests` | Test cases that passed, failed, and were skipped so far. |
| `elapsed_ms` | Time since the run started. |

Struo never fails a run because it cannot write the file. Commands that run
several targets one after another, such as `struo build` without a target, start
a new snapshot sequence for each target in the same file.

## Completion Contract

A harness should report success only after all of these are true:

- the requested workspace files exist;
- the harness wrote them under the root returned by `struo_query_workspace`;
- every project-local module validates against the live authoring contract;
- complete-workspace validation succeeds;
- the requested capability returns `success: true` and exit code `0`;
- important declared outputs exist and have the expected content or type; and
- matching evidence can be queried for the completed action.

The [Model Context Protocol reference](/reference/mcp/) documents transport and
error behavior. The generated [tool catalog](/reference/mcp/tools) is the exact
tool contract served at runtime. The [module reference](/reference/modules/)
defines the target kind authoring surface, while the
[memory reference](/reference/memory/) defines the evidence records that close
the validation loop.
