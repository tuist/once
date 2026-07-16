# Coding Harnesses

Once can be the automation substrate for a coding harness. The harness does not
need Once-specific prompt templates or built-in target kinds for every
toolchain. It connects to the
[Model Context Protocol](https://modelcontextprotocol.io/) server, discovers
the available target kinds and examples, creates or edits workspace files,
validates the loaded graph, runs a capability, and checks the resulting
evidence.

## Connect Once

Start the server in the project directory with editing and execution enabled:

```json
{
  "mcpServers": {
    "once": {
      "command": "mise",
      "args": [
        "-C",
        "/absolute/path/to/project",
        "exec",
        "--",
        "once",
        "mcp",
        "--workspace",
        "/absolute/path/to/project",
        "--allow-run"
      ]
    }
  }
}
```

Omit `--allow-run` when the harness should only inspect the graph and project
memory. In that mode Once does not advertise tools that edit, build, test, run,
or start processes.

During initialization the server returns cross-tool workflow instructions.
Every advertised tool also carries a strict input schema, behavioral hints, an
output schema, and structured content. A harness can therefore plan from the
live protocol contract instead of scraping this guide.

## Create and Run a Typed Graph

Use this loop for requests such as “build an Android app with Once” or “add a
Rust command-line tool with Once”:

1. Call `once_list_target_kinds` and choose a kind whose description and
   `use_when` text match the request. When the request names an ecosystem or
   target-kind family, include it in `query` so it takes priority over generic
   intent words and unrelated target kinds do not consume harness context.
2. Call `once_query_schema` for its complete attribute, dependency, provider,
   capability, and example contract.
3. Call `once_query_example` for the best starter, then materialize every
   returned file exactly as described.
4. Call `once_query_targets` to obtain canonical target identifiers from the
   loaded workspace.
5. Call `once_validate_workspace`. Repair every structured diagnostic and
   repeat until `valid` is `true`.
6. Call `once_query_capabilities` for the chosen target and invoke
   `once_build_target`, `once_run_target`, or the testing tools as appropriate.
7. Check `success`, `exit_code`, and the structured output record. Inspect any
   output paths that matter to the user.
8. Call `once_query_evidence` with the returned target or capability subject to
   retain and inspect the durable result.

Use `once_validate_target` before creating a proposed target table from
scratch. After `once_apply_edit`, always use `once_validate_workspace` because
table validation cannot detect missing dependencies, incompatible providers,
unmatched source patterns, or dependency cycles.

For an Android application, the live catalog leads the harness to
`android_binary` and its runnable starter. The harness creates the returned
manifest and sources, validates the complete graph, discovers the starter's
canonical target identifier, builds it, and checks the Android application
package output. Android-specific behavior remains in the target kind, so the
harness follows the same loop for other ecosystems.

## Adopt an Unfamiliar External Rule

The built-in catalog is a shortcut, not the boundary of what a harness can
model. When a requested node comes from an unsupported rule, plugin, or build
system, use this loop:

1. Identify the authoritative external symbol from the source build manifest,
   registry, plugin declaration, or checked-in rule.
2. Call `once_fetch_external_source` with a public HTTPS address for its source
   code, registry record, or reference. The response is bounded and includes a
   content digest, so the harness can record exactly what it interpreted.
3. Call `once_query_module_contract`. It returns the exact Starlark declaration
   helpers, schema invariants, implementation context, generic analysis and
   action primitives, maintenance invariants, registration snippet, and a
   starter module. Schema defaults are descriptive strings; implementations
   provide optional runtime fallbacks with `ctx["attr"].get(...)`.
4. Write a project-local target kind that represents only the requested node
   and the dependency closure needed to run it. Keep unrelated nodes in the
   existing build system.
5. Add `source_reference(...)` metadata for every external concept the local
   target kind interprets. Record the system, symbol, public address, reason
   this mapping applies, and the returned content digest when the fetch was not
   truncated. During maintenance, re-fetch the same source and compare its
   digest before changing the local graph.
6. Call `once_validate_module` before registering or using the module. Repair
   its structured diagnostic until the returned target kind contracts match
   the intended graph boundary.
7. Register the module, create its target tables, then continue with target,
   workspace, execution, output, and evidence validation.

This lets the harness maintain the project-specific graph instead of waiting
for Once or an external rules package to encode every possible integration.
The Once executor stays ecosystem-neutral. The local module translates the
upstream behavior into explicit inputs, outputs, tools, providers, and portable
actions.

The same path works with a rule from the
[Bazel Central Registry](https://registry.bazel.build/), a
[Buck2 prelude rule](https://buck2.build/docs/prelude/rules/), a
[Gradle plugin](https://plugins.gradle.org/), or another public ecosystem. Once
does not claim that an arbitrary fetched rule is compatible. The harness must
interpret the source, validate the generated module, and prove the requested
capability by executing it and checking its outputs.

## Run an Annotated Script

Use scripts when the requested automation is one executable action rather than
a typed multi-target graph:

1. Create or update the script with `once` declarations for inputs, outputs,
   environment names, working directory, dependency scripts, and fingerprints.
2. Call `once_validate_script` with the workspace-relative path. Repair any
   diagnostic before execution.
3. Call `once_exec_script`. Check `success`, `exit_code`, captured streams,
   action digest, cache state, and the returned evidence subject.
4. Inspect declared output files through the harness workspace.
5. When cache behavior matters, call `once_exec_script` again without changing
   declared inputs and require a cache hit.

This route executes the same annotated script contract as `once exec --script`.
It also materializes declared outputs when a prior action result is reused.

## Use Project Memory Safely

Once evidence is durable provenance for completed actions. It records the
subject, status, action and input digests, cache decision, exit code, captured
stream digests, and creation time.

Evidence is historical. A prior passing record does not prove that current
inputs are unchanged. Use it to understand what ran and to correlate results,
then invoke the relevant build, run, test, or script capability when the user
needs a current result. The action cache decides whether that invocation can be
reused safely.

## Completion Contract

A harness should report success only after all of these are true:

- the requested workspace files exist;
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
