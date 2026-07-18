# Modules

Once graph modules are Starlark files that export graph primitives. Today the
primary exported primitive is a target kind: a schema and optional
implementation function that turns one target into cacheable actions. Built-in
and project target kinds use the same public contract, so a project can add a
target kind without changing Once itself.

## Loading Project Modules

Project modules are listed from the root manifest:

```toml
[modules]
paths = ["modules/*.star"]
```

Each matched file exports one or more public target kind symbols. Public
symbols are module globals that do not start with `_`. The exported
symbol name becomes the target kind unless the target kind explicitly sets
`kind`.

```python
def _copy_impl(ctx):
    out = declare_output("copied.txt")
    srcs = glob(ctx["srcs"])
    copy_path(
        srcs[0],
        out,
        inputs = srcs,
        identifier = ctx["label"]["id"] + ":copy",
    )
    return {
        "label_id": ctx["label"]["id"],
        "copied_file": out,
    }

copy_generated = target_kind(
    docs = "Copy one declared source file into the target output directory.",
    attrs = [],
    providers = ["copied_file"],
    capabilities = [capability("build", ["default"])],
    impl = _copy_impl,
)
```

## Target Kind Schema

`target_kind(...)` declares the public contract exposed by `once query schema`
and by [Model Context Protocol](https://modelcontextprotocol.io/) target kind
discovery.

- `kind`: optional override for the target kind used in `once.toml`.
  When omitted, Once uses the exported symbol name.
- `docs`: short human-readable target kind description.
- `attrs`: `attr(...)` declarations. Supported types include `string`,
  `bool`, `int`, `float`, `list<string>`, `map<string, string>`,
  `target`, and nested values used by `select`.
- `deps`: `dep(...)` declarations that name expected providers.
- `providers`: provider names this target kind can return.
- `capabilities`: command surfaces this target kind supports, such as
  `build`, `run`, `test`, or `metadata`.
- `tools`: `tool(...)` declarations for workspace tools the implementation
  needs during analysis or execution.
- `examples`: `example(...)` declarations for starter workspaces exposed
  to agents.
- `source_references`: `source_reference(...)` declarations that connect this
  target kind to authoritative external rules, plugins, registry records, or
  build-system concepts.
- `impl`: optional function that declares actions and returns a provider
  record.

Attributes can be configurable unless their schema sets
`configurable = False`. Non-configurable attributes reject `select`
during validation before the implementation runs.

## Tool Requirements

`tool(name, executables = [])` adds a tool requirement to the target kind
schema and to every loaded graph target of that kind. `name` matches a key in
the workspace `mise.toml`. `executables` lists the commands the target kind may
invoke and defaults to the tool name.

```python
rust_binary = target_kind(
    docs = "Builds a Rust executable.",
    tools = [tool("rust", executables = ["rustc", "cargo"])],
    capabilities = [capability("build", ["binary"])],
    impl = _rust_binary_impl,
)
```

The requirements are returned by `once query schema` and `once query targets`,
so scheduling can collect the complete tool set as soon as the graph is
loaded. Script targets derive the same requirement from their declared
runtime.

Once carries a fixed mise version with each release. On first use it downloads
the matching mise release binary, verifies its published checksum, and stores
it in Once's data directory. A developer-installed mise is never required.
When a graph build session starts, Once installs the union of its declared
tools before analysis and resolves the declared executable names from that
environment. Command actions and scripts then run through `mise exec` with
implicit installation disabled. Once authorizes the selected workspace
configuration for these managed invocations while keeping user-global mise
configuration isolated.

## Starter Examples

`examples` points at runnable starter bundles for a target kind. Each declaration
provides the example slug, title, selection hint, and package-relative path. The
bundle itself is a real workspace directory with manifests and sources.
Use examples when a caller should be able to discover the target kind, choose a
starter by intent, and materialize a working target without reading
prose docs.

`example(slug, name, use_when, path = None)` defaults `path` to
`examples/<slug>`. Paths are resolved relative to the module package,
validated during schema loading, and loaded only when a caller requests
that specific example.

```python
apple_library = target_kind(
    docs = "Compiles Swift, Objective-C, C, and C++ sources into a linkable Apple module.",
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform"),
    ],
    providers = ["apple_linkable", "apple_module"],
    capabilities = [capability("build", ["default"])],
    examples = [
        example(
            "apple-library-minimal",
            name = "Minimal Apple library",
            use_when = "You want a small Swift static library.",
        ),
        example(
            "apple-library-with-objc",
            name = "Apple library with mixed Swift and Objective-C",
            use_when = "Your Swift interface calls into Objective-C sources.",
        ),
    ],
    impl = _apple_library_impl,
)
```

`once query target-kinds` lists the available target kinds with their starter
example slugs. Use `once query schema <kind> --format json` for the full
target kind contract and lightweight example descriptors:

```sh
once query schema apple_library --format json
```

```json
{
  "kind": "apple_library",
  "examples": [
    {
      "slug": "apple-library-minimal",
      "name": "Minimal Apple library",
      "use_when": "Start a small Apple library target."
    }
  ]
}
```

Agents should use `use_when` to pick the closest starter, then fetch its
file bundle:

```sh
once query example apple_library apple-library-minimal --format json
```

```json
{
  "slug": "apple-library-minimal",
  "name": "Minimal Apple library",
  "use_when": "Start a small Apple library target.",
  "files": [
    {
      "path": "apps/Hello/once.toml",
      "contents": "[[target]]\nname = \"Hello\"\nkind = \"apple_library\"\n..."
    }
  ]
}
```

[Model Context Protocol](https://modelcontextprotocol.io/) callers use
`once_list_target_kinds` and `once_query_schema` for discovery, then
`once_query_example` to fetch the file bundle for the chosen starter.

## External Rule Assimilation

A project module can reproduce the useful portion of a rule or plugin that
Once has never seen before. The coding harness owns the translation and keeps
it with the project. Once supplies the typed schema, validation, generic action
primitives, execution, cache, and evidence surfaces.

Start by fetching the authoritative source:

```sh
once query external-source https://example.com/rules/write_file.star --format json
```

The fetch accepts public HTTPS text, does not follow redirects, and returns at
most the requested byte limit. Its digest lets the harness remember which
upstream content it interpreted. Use a final raw source, registry, or
documentation address instead of an unbounded repository page.

Query the authoring contract instead of assuming that a remembered primitive
signature is still current:

```sh
once query module-contract --format json
```

The result contains the declaration helper source, schema invariants,
implementation context, analysis primitives, action primitives, maintenance
invariants, module registration, and runnable starters. For test modules it
also returns a matching target table and an exact normalized result example.
Attribute defaults in schemas are descriptive strings and do not insert
runtime values. An implementation uses `ctx["attr"].get(...)` when an optional
attribute needs a fallback. After writing the project module, validate it in
isolation:

```sh
once query validate-module modules/generated_text.star --format json
```

Attach the upstream relationship to the local target kind:

```python
generated_text = target_kind(
    docs = "Generates one text file from declared lines.",
    attrs = [
        attr("output", "string", required = True, configurable = False),
        attr("lines", "list<string>", required = True),
    ],
    providers = ["generated_file"],
    capabilities = [capability("build", ["default"])],
    source_references = [
        source_reference(
            "Example Build",
            "write_file",
            "https://example.com/rules/write_file.star",
            "Replicate the requested generated-file node and no unrelated graph nodes.",
            content_digest = "digest returned by once_fetch_external_source",
        ),
    ],
    impl = _generated_text_impl,
)
```

`source_reference(...)` is descriptive metadata, not executable trust. It is
returned by target kind discovery so a future maintenance pass can refetch the
source, compare its digest, and decide whether the local translation needs an
update. Set `content_digest` only after a complete, untruncated fetch. Omit it
for documentation-only references.

Translate only the dependency closure necessary for the user's requested
capability. Keep everything else in the source build. Invoke tools directly
with argument lists, declare inputs and outputs, and use portable setup
primitives instead of hiding graph work in a shell command. Then validate the
module, target tables, and complete workspace before executing the capability
and checking fresh evidence.

## Implementation Context

An implementation receives `ctx` with generic graph data:

- `ctx["label"]`: `package`, `name`, and stable `id`.
- `ctx["attr"]`: typed attributes after manifest parsing.
- `ctx["srcs"]`: raw source patterns from the target.
- `ctx["deps"]`: provider records returned by analyzed dependencies.
- `ctx["build_dir"]`: workspace-relative output directory for the
  target.
- `ctx["scratch_dir"]`: workspace-relative scratch directory for
  action-private helper files that are materialized before an action
  runs but are not durable target outputs.
- `ctx["capability"]`: active capability being analyzed.
- `ctx["run"]`: run request options. `ctx["run"]["visible"]` is true when
  the caller requested a visible runtime interface.
- `ctx["test"]`: test request options. `ctx["test"]["filters"]` contains
  stable semantic unit identifiers selected for this test execution. Target
  kinds that declare case filtering translate these identifiers into native
  runner arguments. `ctx["test"]["batch_id"]` is a stable batch identifier
  during automatically scheduled execution and is `None` for a normal
  whole-target execution.

The implementation returns a dictionary of provider fields. Downstream
target kinds should read provider fields from `ctx["deps"]` instead of
inspecting target identities.

## Host Globals

Target kind implementations can use these generic primitives:

- `host_arch()` and `host_os()` return normalized host identifiers.
- `host_env(name)` returns one host environment variable, or an empty
  string when it is unset.
- `workspace_root()` returns the absolute workspace root.
- `host_which(name)` resolves an executable on `PATH`.
- `host_command(argv, env = {})` runs a discovery command and returns
  standard output. Arguments and environment values participate in the
  command-scoped cache key.
- `host_file_exists(path)` checks whether a host path is currently a
  file.
- `host_file_read(path)` reads a host file as
  [Unicode Transformation Format, 8-bit (UTF-8)](https://www.unicode.org/faq/utf_bom.html#UTF8)
  text.
- `host_file_sha256(path)` returns a host file's
  [Secure Hash Algorithm 256-bit](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
  digest as lowercase hexadecimal text.
- `host_file_contains(path, needle)` checks host file text content.
- `glob(patterns)` expands patterns under the active package and returns
  sorted workspace-relative file paths.
- `declare_output(name)` reserves an output under the target build
  directory.
- `cmd_args(args, use_arg_file = None)` creates a structured
  command-line fragment. `args` is a list of strings. When
  `use_arg_file` is set, it is a dictionary with `path` plus optional
  `format` and `arg_format`. The supported `format` value is
  `line-delimited`, which writes one argument per line without shell
  escaping. The caller chooses `path`; use `ctx["scratch_dir"]`
  for action-private helper files and `declare_output` for durable
  target outputs. `arg_format` defaults to `@{}` and must contain
  exactly one `{}` placeholder.
- `run_action(...)` records a command action for Once to execute.
- `write_path(path, content)` materializes generated text or byte-list
  files through normal actions.
- `copy_path(source, destination, inputs = [])` copies one workspace
  file.
- `copy_path(source, destination, kind = "tree", inputs = [])` copies
  one or more directory contents.
- `materialize_host_file(source, destination)` snapshots one absolute host
  toolchain file into a workspace output. Analysis records its
  [256-bit Secure Hash Algorithm digest](https://csrc.nist.gov/pubs/fips/180-4/upd1/final),
  and execution verifies the digest before the output enters the cache.
- `prepare_path(path, kind = "remove")` and
  `prepare_path(path, kind = "directory")` declare uncached cleanup and
  setup actions for workspace paths.
- `write_tree_digest(root, output, include_suffixes = [])` writes a
  deterministic digest listing for a workspace tree.
- `toml_decode(src)` and `json_decode(src)` decode data into Starlark
  values.

Use these primitives to express resolver commands, source filtering, file
formats, tool selection, compiler flags, provider conventions, and action
layout.

## Actions

`run_action` accepts:

- `argv`: command and arguments as strings or `cmd_args` fragments.
- `inputs`: workspace-relative files and directories hashed into the
  action digest.
- `outputs`: workspace-relative outputs the action must produce.
- `clean_paths`: workspace-relative paths to remove before a fresh
  command execution. Cache hits restore outputs without running the
  command.
- `create_dirs`: workspace-relative directories to create before a fresh
  command execution.
- `cwd`: workspace-relative directory to run the command in. Defaults to
  the workspace root when omitted or `None`.
- `env`: string environment variables.
- `sandbox`: local filesystem sandbox policy. `off` uses the current
  workspace view. `inputs` runs in an action-private workspace view
  populated from declared inputs and copies declared outputs back after
  a successful command.
- `cacheable`: `True` by default. Set `False` for interactive or local
  side-effect actions.
- `depends_on_prior_actions`: `True` by default. When true, each action key
  includes prior actions declared by the same target. Set `False` only for
  independent actions that do not read earlier same-target outputs.
- `toolchain_identity`: optional string folded into the action digest.
- `identifier`: stable diagnostic label.

Actions inside one target run in declaration order because later actions
may consume earlier outputs. Independent graph targets run concurrently
once their analysis-backed dependencies are complete.

## Script-backed Test Target Kinds

`once query module-contract --format json` returns a complete `test_starter`
for a project-local script-backed test kind, a matching
`test_target_starter`, and a `normalized_test_result_example`. The starter
resolves its host tool, invokes a package-relative adapter directly, declares
all inputs and outputs, and passes exact unit filters from
`ctx["test"]["filters"]`.

A test target declares the `once_test_info` provider and the generic test
capability:

```python
providers = ["once_test_info"]
capabilities = [capability("test", ["default", "test_results", "logs"])]
```

Its implementation returns `test_info` with this shape:

```python
{
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
    "sharding": {"supported": True, "granularity": "file"},
    "retries": {"supported": False, "default_attempts": 1},
    "execution": {
        "cacheable": True,
        "timeout_ms": None,
        "run_from_workspace_root": True,
    },
    "labels": [],
    "metadata": {},
}
```

The provider may also return `test_discovery_inputs`, a list of
workspace-relative files whose contents can change discovered unit
identifiers. Once fingerprints those files before reusing a manifest. When the
field is absent, Once conservatively fingerprints the provider's complete
`affected_inputs` list.

The adapter writes [JavaScript Object Notation](https://www.json.org/json-en.html)
results below the target test directory. When `ctx["test"]["batch_id"]` is
present, the adapter must add `batches/<batch_id>` to that directory before
declaring its result, log, and native-result paths. This prevents concurrent
batches of one target from overwriting each other. Once merges the batch
records into the target's canonical result after the schedule completes.

```json
{
  "schema": "once.test_results.v1",
  "target": "tests/example",
  "runner": {"type": "scripted", "metadata": {}},
  "status": "passed",
  "summary": {
    "total": 1,
    "passed": 1,
    "failed": 0,
    "skipped": 0,
    "flaky": 0
  },
  "cases": [{
    "id": "tests/example::case-name",
    "name": "case-name",
    "suite": "tests/example",
    "status": "passed",
    "attempts": [{"status": "passed"}],
    "runner_metadata": {}
  }],
  "artifacts": {"logs": ["<declared-log>"], "native_results": []}
}
```

Case identifiers are stable target-qualified semantic names. Declare
`case_filtering = "runner_args"` only when every requested identifier is
translated exactly into the native runner invocation. Otherwise declare
`unsupported`.

Set `sharding.supported` only when exact filtering and batch-isolated outputs
are both implemented. `granularity = "file"` groups all discovered cases with
the same `file` value into one batch. `granularity = "case"` creates one batch
per stable unit. `granularity = "target"`, an absent manifest, or a stale
manifest keeps the whole target as one batch. Batch identity depends on the
target and semantic unit identifiers, not the worker count.

A failed or incomplete runner exits unsuccessfully and writes a
matching failed result when possible. A successful host process status must
never turn a runner crash or missing successful terminal record into a pass.
Once validates this complete record before it derives discovery data or marks
a scheduled batch as successful. A malformed runner, summary, attempt, or
artifact record fails the run instead of being accepted as partial evidence.

## Authoring Target Kinds

Keep ecosystem behavior within its target kind. A target kind can understand a
compiler, package manager, binary format, or platform naming convention while
still exposing the same target schema, action, and provider contracts as every
other kind.

Provider records are the cross-target kind contract. Prefer small, typed,
documented fields such as output paths, transitive inputs, flags, and
metadata. Avoid making consumers branch on a dependency target kind.

Use `toolchain_identity` for tool versions, resolved compiler paths, platform
tool selection, generated file contents, and other non-source inputs that
should invalidate cached actions. Do not log secrets or place secrets in
arguments, provider records, or action metadata.

When adding a public target kind, make it discoverable through `once query
target-kinds` and `once query schema`, provide at least one starter example,
and keep the schema, docs, and examples in sync.
