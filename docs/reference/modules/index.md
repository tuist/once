# Modules

Once graph modules are Starlark files that export graph primitives. Today the
primary exported primitive is a target kind: a schema and optional
implementation function that lowers one target into cacheable actions. Built-in
target kinds and project target kinds use the same shape, so a project can add a
target kind without changing the Rust executor.
The built-in prelude also declares its source order in Starlark, so
adding a target kind family updates the prelude rather than hardcoding a new
toolchain in Rust.

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
and by MCP target kind discovery.

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
- `examples`: `example(...)` declarations for starter workspaces exposed
  to agents.
- `impl`: optional function that declares actions and returns a provider
  record.

Attributes can be configurable unless their schema sets
`configurable = False`. Non-configurable attributes reject `select`
during validation before the implementation runs.

## Starter Examples

`examples` points at runnable starter bundles for a target kind. Starlark owns
the example slug, title, selection hint, and package-relative path. The
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
            use_when = "Your Swift API calls into Objective-C sources.",
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

MCP callers use `once_list_target_kinds` and `once_query_schema` for discovery,
then `once_query_example` to fetch the file bundle for the chosen
starter.

## Implementation Context

An implementation receives `ctx` with generic graph data:

- `ctx["label"]`: `package`, `name`, and stable `id`.
- `ctx["attr"]`: typed attributes after manifest parsing.
- `ctx["srcs"]`: raw source patterns from the target.
- `ctx["deps"]`: provider records returned by analyzed dependencies.
- `ctx["build_dir"]`: workspace-relative output directory for the
  target.
- `ctx["capability"]`: active capability being analyzed.

The implementation returns a JSON-shaped provider record. Downstream
target kinds should read provider fields from `ctx["deps"]` instead of
inspecting target identities.

## Host Globals

The Rust executor exposes generic primitives only:

- `host_arch()` and `host_os()` return normalized host identifiers.
- `host_env(name)` returns one host environment variable, or an empty
  string when it is unset.
- `workspace_root()` returns the absolute workspace root.
- `host_which(name)` resolves an executable on `PATH`.
- `host_command(argv, env = {})` runs a discovery command and returns
  stdout. Arguments and env values participate in the command-scoped
  cache key.
- `host_file_exists(path)` checks whether a host path is currently a
  file.
- `host_file_sha256(path)` returns a host file's SHA-256 digest as
  lowercase hex.
- `host_file_contains(path, needle)` checks host file text content.
- `glob(patterns)` expands patterns under the active package and returns
  sorted workspace-relative file paths.
- `declare_output(name)` reserves an output under the target build
  directory.
- `cmd_args(args, use_arg_file = None)` creates a structured
  command-line fragment. `args` is a list of strings. When
  `use_arg_file` is set, it is a dictionary with `path` plus optional
  `format` and `arg_format`. The supported `format` is
  `line-delimited`, which writes one argument per line without shell
  escaping. `arg_format` defaults to `@{}` and must contain exactly one
  `{}` placeholder.
- `run_action(...)` records a command action for the executor.
- `write_path(path, content)` materializes generated text or byte-list
  files through normal actions.
- `copy_path(source, destination, inputs = [])` copies one workspace
  file through a portable Rust action.
- `copy_path(source, destination, kind = "tree", inputs = [])` copies
  one or more directory contents through portable Rust actions.
- `prepare_path(path, kind = "remove")` and
  `prepare_path(path, kind = "directory")` declare uncached portable
  cleanup and setup actions for workspace paths.
- `write_tree_digest(root, output, include_suffixes = [])` writes a
  deterministic digest listing for a workspace tree.
- `toml_decode(src)` and `json_decode(src)` decode data into Starlark
  values.

Toolchain behavior belongs in Starlark on top of these primitives:
resolver commands, source filtering, file formats, SDK selection,
compiler flags, provider conventions, and action layout.

## Actions

`run_action` accepts:

- `argv`: command and arguments as strings or `cmd_args` fragments.
- `inputs`: workspace-relative files and directories hashed into the
  action digest.
- `outputs`: workspace-relative outputs the action must produce.
- `env`: string environment variables.
- `cacheable`: `True` by default. Set `False` for interactive or local
  side-effect actions.
- `toolchain_identity`: optional string folded into the action digest.
- `identifier`: stable diagnostic label.

Actions inside one target run in declaration order because later actions
may consume earlier outputs. Independent graph targets run concurrently
once their analysis-backed dependencies are complete.

## Design Modules

Keep target kind implementations ecosystem-specific and the executor generic.
If a target kind needs to understand a compiler, package manager, SDK, binary
format, or platform naming convention, encode that in Starlark and pass
only declared actions and provider records back to Rust.

Provider records are the cross-target kind contract. Prefer small, typed,
documented fields such as output paths, transitive inputs, flags, and
metadata. Avoid making consumers branch on a dependency target kind.

Use `toolchain_identity` for tool versions, resolved compiler paths, SDK
selection, generated file contents, and other non-source inputs that
should invalidate cached actions. Do not log secrets or place secrets in
arguments, provider records, or action metadata.

When adding a public target kind, make it discoverable through `once query
target-kinds` and `once query schema`, provide at least one starter example,
and keep the schema, docs, and examples in sync.
