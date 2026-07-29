# Modules

Once graph modules are Starlark files that export graph primitives. The primary
exported primitive is a target kind: a schema, an optional resolver that expands
locked dependency metadata into targets, and an optional implementation that
turns one target into cacheable actions. Built-in and project target kinds use
the same public contract, so a project can add a target kind without changing
Once itself.

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
  `build`, `lint`, `run`, `test`, or `metadata`.
- `tools`: `tool(...)` declarations for workspace tools the implementation
  needs during analysis or execution.
- `examples`: `example(...)` declarations for starter workspaces exposed
  to agents.
- `source_references`: `source_reference(...)` declarations that connect this
  target kind to authoritative external rules, plugins, registry records, or
  build-system concepts.
- `impl`: optional function that declares actions and returns a provider
  record.
- `resolver`: optional function that reads declared text inputs and returns
  synthetic targets before graph validation and scheduling.

Attributes can be configurable unless their schema sets
`configurable = False`. Non-configurable attributes reject `select`
during validation before the implementation runs.
Set `implemented = False` on compatibility attributes that should remain
discoverable but are not available yet. Validation returns
`unimplemented_attr` with an attribute-scoped repair before analysis starts.
Values declared with the `target` type use the same package-relative target
syntax as dependency references. Complete-workspace validation confirms that
each referenced target exists and returns an attribute-scoped repair when it
does not.
Use `allowed_values = ["first", "second"]` on a `string` or `target` attribute
when the schema accepts a fixed set. Validation reports an attribute-scoped
repair before analysis when a manifest supplies another value.
Use `disallowed_values = ["", "reserved"]` when the schema accepts arbitrary
strings except for a small reserved set. Validation ignores surrounding
whitespace when comparing these values and reports an attribute-scoped repair
before analysis.
Use `min_count` and `max_count` on `dep(...)` when a dependency role requires
a bounded number of targets. For example,
`dep("deps", ["application"], min_count = 1, max_count = 1)` requires exactly
one default dependency and reports an attribute-scoped repair before analysis.

## Native Project Discovery Contract

`native_project(...)` lets a module recognize an ecosystem-native project and
map it to one ordinary seed target. The seed target's resolver emits the
detailed graph through the same typed resolver contract as
manifest-authored targets.

```python
native = native_project(
    name = "native",
    target_kind = "native_workspace",
    target_name = "native",
    docs = "Recognizes a native project.",
    markers = ["project.native"],
    inputs = ["project.lock", "config/**/*"],
    exclude = ["build"],
    on_match = "stop",
    max_depth = 16,
    requires_tools = ["native-tool"],
)
```

- `target_kind` names the target kind instantiated by the seed. Schema loading
  rejects a native project that references an unknown target kind.
- `markers` lists files that must all exist in one directory. The first entry
  drives the bounded scan.
- `inputs` adds optional text globs to the seed's resolver inputs. Markers are
  always included.
- `target_name` names the virtual or initialized target.
- `exclude` lists directory names skipped during discovery.
- `on_match = "stop"` keeps the shallowest matching root, which suits native
  workspaces that own nested package manifests. `"descend"` also recognizes
  nested projects.
- `max_depth` bounds discovery.
- `requires_tools` reports executables needed when the seed resolver runs.

Detection reads file names only. It does not evaluate executable manifests or
invoke native tools. Previewing or loading the graph runs the seed target's
resolver through the normal trusted analysis boundary.

Discovery retains at most 16 mebibytes of match records. Once stops with an
error if a workspace produces more, rather than allowing repository size to
drive unbounded memory growth. Narrow the workspace include or exclude patterns,
or use `on_match = "stop"` when nested projects belong to one workspace.

Native project discovery provides ephemeral seeds in packages that have no explicit Once
targets. Explicit targets take precedence only in their own package, so an
unrelated root target does not hide native projects elsewhere in a monorepo.
The configured workspace include and exclude patterns still define the
boundary.

Discover and preview native projects from the command line:

```sh
once query native-projects
once query native-project native
```

Initialize only the stable seed when the repository should own the selection:

```sh
once edit init-native-project native
```

Initialization is idempotent and preserves unrelated `once.toml` configuration and
comments. Resolver-emitted dependency and product targets remain derived from
the native manifest and lockfile.

[Model Context Protocol](https://modelcontextprotocol.io/) callers use the
matching `once_list_native_projects`, `once_preview_native_project`, and
`once_init_native_project` tools.

## Dependency Resolver Contract

A target kind resolver imports an authoritative locked dependency graph. It is
not a replacement for the ecosystem package manager. Cargo, Mix and Hex, Swift
Package Manager, Zig, or another native tool continues to own manifest
semantics, registry behavior, version selection, and lockfile updates.

The resolver receives a restricted context:

- `ctx["label"]`: package, name, and stable target identifier.
- `ctx["attr"]`: typed owner attributes. `ctx["attrs"]` is an equivalent
  spelling for resolver compatibility.
- `ctx["configuration"]`: normalized target operating system, architecture,
  and ordered selection tokens.
- `ctx["srcs"]`: declared build source patterns.
- `ctx["files"]`: resolver input files decoded as text, keyed by path relative
  to the owner package. Non-empty `resolver_inputs` patterns define this map.
  When the list is empty or omitted, `srcs` is used for compatibility.

Resolver files are limited to 16 mebibytes per file and 64 mebibytes in total
for one target. Use `resolver_inputs` to keep binary, generated, or large build
sources out of this text-only context while retaining them in `srcs` for build
actions. A resolver should consume bounded text such as a manifest, lock file,
or checked-in graph snapshot.

It may use generic host discovery primitives, including `host_command`, to ask
the native package manager for locked metadata. It cannot declare actions or
outputs. Resolution happens while the graph loads, so the returned value must
be deterministic for the declared files, target attributes, module source, and
resolver toolchain.

The compact return form is a list of target records. The detailed form also
chooses owner roots and adds resolver-owned attributes to the owner:

```python
def _resolve(ctx):
    return {
        "targets": [
            {
                "name": "package-1.2.3",
                "kind": "ecosystem_package",
                "deps": ["./transitive-4.5.6"],
                "dependencies": {
                    "build_deps": ["./build-helper-2.0.0"],
                },
                "srcs": ["third_party/package-1.2.3/src/**"],
                "attrs": {
                    "version": "1.2.3",
                    "content_hash": "...",
                },
            },
        ],
        "roots": ["package-1.2.3"],
        "attrs": {
            "resolution_state": "locked",
        },
    }
```

Each target record accepts only `name`, `kind`, `deps`, `dependencies`, `srcs`,
`visibility`, and `attrs`. Dependency references and exact visibility entries
use the same syntax as a manifest, including `./name` for a target in the owner
package. A bare root name that matches an emitted target is also package-local.
When `roots` is omitted, every emitted target becomes an owner dependency.
Resolver-owned and synthetic attributes must be declared in their target kind
schemas, including internal bookkeeping attributes. A resolver attribute
cannot replace a value declared by the owner target. Expansion stops at
100,000 workspace targets so a recursive resolver cannot grow the graph
without bound.

Ordinary builds consume locked metadata and already materialized sources. A
separate explicit update workflow should invoke the native package manager when
the lockfile or source set needs to change. Modules may use the
[PubGrub version-solving algorithm](https://github.com/dart-lang/pub/blob/master/doc/solver.md)
for an ecosystem that has no authoritative resolver, but it is not the primary
integration boundary.

See [Ecosystems: Import Locked Third-Party Graphs](/guide/graph/ecosystems#import-locked-third-party-graphs)
for the shared update, source-materialization, query, and first-party consumer
workflow.

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

Agents should use `use_when` to pick the closest starter, then materialize it
without copying file payloads through model context:

```sh
once edit materialize-example apple_library apple-library-minimal
```

Fetch the file bundle only when the caller needs to inspect or adapt it:

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

Text files use `contents`. Binary files use `contents_base64` so direct
materialization preserves their exact bytes.

[Model Context Protocol](https://modelcontextprotocol.io/) callers use
`once_list_target_kinds` and `once_query_schema` for discovery, then
`once_materialize_example` for direct setup or `once_query_example` to inspect
the file bundle. Repository contributors can run
`mise run examples:verify-portable` to materialize and execute the portable
starter set.

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
- `ctx["deps_by_role"]`: provider records grouped by the dependency roles
  declared in the target kind schema. The `deps` entry contains the same
  providers as `ctx["deps"]`.
- `ctx["build_dir"]`: workspace-relative output directory for the
  target.
- `ctx["scratch_dir"]`: workspace-relative scratch directory for
  action-private helper files that are materialized before an action
  runs but are not durable target outputs.
- `ctx["capability"]`: active capability being analyzed.
- `ctx["run"]`: run request options. `ctx["run"]["visible"]` is true when
  the caller requested a visible runtime interface. `ctx["run"]["args"]`
  contains caller arguments for the run capability and is empty for other
  capabilities. Their interpretation is target-kind-specific.
- `ctx["test"]`: test request options. `ctx["test"]["filters"]` contains
  stable semantic unit identifiers selected for this test execution. Target
  kinds that declare case filtering translate these identifiers into native
  runner arguments. `ctx["test"]["batch_id"]` is a stable batch identifier
  during automatically scheduled execution and is `None` for a normal
  whole-target execution.

The implementation returns a dictionary of provider fields. Downstream
target kinds should read provider fields from `ctx["deps"]` or
`ctx["deps_by_role"]` instead of inspecting target identities.

When an artifact has different link-time and runtime dependency closures,
publish those closures as separate provider fields. Do not flatten them into
one transitive list. Runtime bundle records should retain the owning target,
bundle path, module name, and complete file outputs so final packaging actions
can de-duplicate, materialize, and sign the closure without rediscovering the
graph.

## Host Globals

Target kind implementations can use these generic primitives:

Modules are trusted analysis code. `host_command` executes on the host while
the graph is loading, so load modules only from trusted sources and reserve it
for deterministic discovery. It must not mutate workspace sources or the build
output tree, and resolver scratch state belongs only under `.once/tmp`. Prefer
declared lock files and checked-in graph snapshots when a package manager can
supply them. Build work and source fetching belong in explicit actions or a
separate update workflow.

- `host_arch()` and `host_os()` return normalized host identifiers.
- `host_env(name)` returns one host environment variable, or an empty
  string when it is unset.
- `workspace_root()` returns the absolute workspace root.
- `host_which(name)` resolves an executable on `PATH`.
- `host_command(argv, env = {}, cwd = None, merge_stderr = False)` runs a
  discovery command and returns standard output. Arguments, environment
  values, the working directory, and stream merging participate in the
  command-scoped cache key. When set, `cwd` must be an absolute path, normally
  derived from `workspace_root()`. Each captured stream is limited to 16
  mebibytes.
- `host_file_exists(path)` checks whether a host path is currently a
  file.
- `host_file_read(path)` reads a host file as
  [Unicode Transformation Format, 8-bit (UTF-8)](https://www.unicode.org/faq/utf_bom.html#UTF8)
  text. Analysis rejects files larger than 16 mebibytes.
- `host_file_sha256(path)` returns a host file's
  [Secure Hash Algorithm 256-bit](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
  digest as lowercase hexadecimal text.
- `host_file_contains(path, needle)` checks host file text content and applies
  the same 16 mebibyte file limit.
- `glob(patterns, exclude = [])` expands patterns under the active package,
  omits matches selected by package-relative exclude patterns, and returns
  sorted workspace-relative file paths.
- `walk_files(root, excluded_paths = [], excluded_names = [])` walks a
  package-relative directory and returns sorted workspace-relative file and
  symbolic-link paths. Exact root-relative exclusions and file names prune
  whole trees before traversal.
- `declare_output(name)` reserves an output under the target build
  directory.
- `execution_path(path)` returns a stable command value for a
  workspace-relative path. Once resolves it against the local, sandbox, or
  remote execution root immediately before launching the process. Use it for
  tools that require absolute paths without embedding a host workspace path in
  the action digest.
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
  path by value. A directory symlink is materialized at the destination.
- `copy_path(source, destination, kind = "tree", inputs = [])` copies
  one or more directory contents while preserving their symlink layout.
- `materialize_host_file(source, destination)` snapshots one absolute host
  toolchain file into a workspace output. Analysis records its
  [256-bit Secure Hash Algorithm digest](https://csrc.nist.gov/pubs/fips/180-4/upd1/final),
  and execution verifies the digest before the output enters the cache.
- `link_path(source, destination)` declares an uncached, relative workspace
  link. The source must exist. Linked contents are not copied into the action
  cache, while downstream actions still hash them when they declare the link
  as an input. Windows hosts require permission to create symbolic links.
- `prepare_path(path, kind = "remove")` and
  `prepare_path(path, kind = "directory")` declare uncached cleanup and
  setup actions for workspace paths.
- `write_tree_digest(root, output, include_suffixes = [])` writes a
  deterministic digest listing for a workspace tree.
- `write_archive(entries, output, sha256_output = None, format = "tar")`
  writes a deterministic uncompressed
  [tar archive](https://www.gnu.org/software/tar/manual/html_node/Standard.html).
  Each entry declares a `file`, `directory`, or recursive `tree`, its archive
  path, fixed mode, numeric owner and group identifiers, and modification
  time. When requested, `sha256_output` records the
  [Secure Hash Algorithm 256-bit](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
  digest.
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
  a successful command. `copied-inputs` provides the same isolation but
  materializes private input copies for tools that cannot consume symbolic
  links. Both isolated modes create parent directories for declared outputs
  inside the private workspace. Use `once query validate-actions` to run an
  uncached contract investigation that checks created, modified, and deleted
  paths and returns structured repairs. Reads that leave no observable
  filesystem evidence remain a limitation of this investigation.
- `success_exit_codes`: integer list whose members mean the command completed
  and its declared outputs are valid. Defaults to `[0]`. Use this for tools
  that report valid findings with a nonzero process code.
- `cacheable`: `True` by default. Set `False` for interactive or local
  side-effect actions.
- `inherit_parent_env`: `False` by default. Set `True` only on an
  uncacheable, unsandboxed local run action that should behave like a command
  launched from the user's development shell. Explicit `env` values take
  precedence.
- `depends_on_prior_actions`: `True` by default. When true, each action key
  includes prior actions declared by the same target. Set `False` only for
  independent actions that do not read earlier same-target outputs.
- `toolchain_identity`: optional string folded into the action digest.
- `identifier`: stable diagnostic label.

Actions inside one target run in declaration order because later actions
may consume earlier outputs. Independent graph targets run concurrently
once their analysis-backed dependencies are complete.

## Executable And Container Providers

Native binary target kinds can publish the shared `once_executable` provider.
It carries the executable path, runtime files, operating system, architecture,
architecture variant, and linkage. Consumers depend on that provider instead
of branching on the language that produced the program.

The built-in `oci_layer` and `oci_image` target kinds use
[`oci` as the abbreviation for Open Container Initiative](https://opencontainers.org/).
An `oci_layer` packages native executable providers and source files into a
deterministic filesystem layer. It can also pass through an existing
uncompressed layer archive during incremental adoption. An `oci_image`
assembles ordered layers, runtime configuration, a content-addressed image
layout, and an archive accepted by
[Docker](https://www.docker.com/).
Both native images and Dockerfile-backed images publish the shared
`container_image` provider.

```toml
[[target]]
name = "server_layer"
kind = "oci_layer"

[target.dependencies]
programs = ["server"]

[[target]]
name = "server_image"
kind = "oci_image"

[target.dependencies]
layers = ["server_layer"]

[target.attrs]
tag = "server:latest"
cmd = ["serve"]
env = { LOG_LEVEL = "info" }
working_dir = "/app"
```

When a layer contains exactly one executable, the image uses that executable
as its default entry point. Explicit `entrypoint`, `cmd`, `env`, `user`,
`working_dir`, `stop_signal`, labels, annotations, and exposed ports override
or extend the runtime configuration. Layer metadata defaults to zero for the
numeric owner, group, and modification time so identical inputs produce
identical bytes.

The `dockerfile_image` target kind gives complete Dockerfile semantics to
[BuildKit](https://docs.docker.com/build/buildkit/). Starlark declares the
context inputs, build arguments, platform, target stage, network policy,
timestamp, archive format, and output contract. BuildKit interprets
multi-stage files, resolves base images, and executes `RUN` instructions,
including package installation.
Docker and its selected Docker Buildx builder must be reachable when Once
analyzes a build so the toolchain identity can include the builder driver and
BuildKit version. Metadata and affected-file queries infer context inputs
without contacting Docker.

```toml
[[target]]
name = "service_image"
kind = "dockerfile_image"

[target.attrs]
tag = "service:latest"
platform = "linux/arm64"
build_args = { MODE = "release" }
```

When `srcs` is empty, Once infers every local file and symbolic link that the
build context can expose, including hidden paths. It also adds the Dockerfile
and the effective [Docker ignore file](https://docs.docker.com/build/building/context/#dockerignore-files)
automatically. Dockerfile-specific ignore files take precedence over the
context's ignore file, matching Docker behavior.

Inference recognizes simple literal ignore paths plus `**/<name>` exclusions
and prunes their trees before walking the context. Wildcards, exceptions, and
other complex ignore rules remain declared as inputs, then BuildKit applies
their complete semantics during the build. This conservative direction can
cause extra rebuilds, but it cannot hide a file that the Dockerfile may read.
Workspace runtime directories named `.once` are always excluded so outputs
never become their own inputs.

Empty directories do not participate in the inferred input set yet. Declare a
placeholder file when a Dockerfile must copy an otherwise empty directory.

Set `srcs` only when a smaller, manually maintained input set is worth the
precision. Once still adds the Dockerfile and effective ignore file. Dockerfile
actions use an input-only workspace view, so an explicit set must cover every
local context file the build may read.

The rule rejects multiple values in `platform`; one target describes one
exported platform. Selecting `network = "host"` also grants BuildKit's matching
host-network entitlement.

Dockerfile actions are not stored in the Once action cache by default because
image tags, package registries, and network instructions may change without a
source edit. BuildKit still reuses its own layer cache. Set `format = "oci"`,
`cacheable = true`, and `pull = false` only when every remote input is
immutable, such as base images pinned by digest and network access disabled.
Open Container Initiative export requires a Docker Buildx builder that
supports archive export.

When the selected builder supports direct export, both archive formats are
written without loading the image into the Docker engine. The built-in Docker
driver cannot write an archive directly, so Docker output falls back to
loading and saving the requested tag. That compatibility path uses shared
engine state, leaves the loaded image behind, and always bypasses the Once
action cache. Use a container or remote builder when builds sharing the same
tag may run concurrently.

## Lint Target Kinds

A lint target declares the reserved `once_lint_info` provider and generic
lint capability:

```python
providers = ["once_lint_info"]
capabilities = [capability("lint", ["default", "lint_results"])]
```

Its `lint_info` record names the analyzer, requested source scope, command,
portable report path, normalized result path, native artifacts, logs, and
execution policy. The report uses
[Static Analysis Results Interchange Format](https://docs.oasis-open.org/sarif/sarif/v2.1.0/sarif-v2.1.0.html).
Tool-specific invocation and native configuration remain in the target kind.
The Rust execution and result layers stay ecosystem-neutral.

`once query module-contract --format json` returns a complete `lint_starter`,
matching target and adapter starters, and a normalized result example. See
[Custom lint target kinds](/reference/modules/linting) for a complete direct
reporter, native report conversion, validation, execution, and cache
verification workflow.

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
