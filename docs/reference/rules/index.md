# Rules

Once rules are Starlark records that describe a target schema and,
optionally, an implementation function that lowers one target into
cacheable actions. Built-in rules and project rules use the same shape,
so a project can add a rule without changing the Rust executor.
The built-in prelude also declares its source order in Starlark, so
adding a rule family updates the prelude rather than hardcoding a new
toolchain in Rust.

## Loading Project Rules

Project rules are listed from the root manifest:

```toml
[rules]
paths = ["rules/*.star"]
```

Each matched file exports one or more public rule symbols. Public
symbols are module globals that do not start with `_`. The exported
symbol name becomes the target kind unless the rule explicitly sets
`kind`.

```python
def _copy_impl(ctx):
    out = declare_output("copied.txt")
    srcs = glob(ctx["srcs"])
    run_action(
        argv = [host_which("cp"), srcs[0], out],
        inputs = srcs,
        outputs = [out],
        identifier = ctx["label"]["id"] + ":copy",
    )
    return {
        "label_id": ctx["label"]["id"],
        "copied_file": out,
    }

copy_file = rule(
    docs = "Copy one declared source file into the target output directory.",
    attrs = [],
    providers = ["copied_file"],
    capabilities = [capability("build", ["default"])],
    impl = _copy_impl,
)
```

## Rule Schema

`rule(...)` declares the public contract exposed by `once query schema`
and by MCP rule discovery.

- `kind`: optional override for the target kind used in `once.toml`.
  When omitted, Once uses the exported symbol name.
- `docs`: short human-readable rule description.
- `attrs`: `attr(...)` declarations. Supported types include `string`,
  `bool`, `int`, `float`, `list<string>`, `map<string, string>`,
  `target`, and nested values used by `select`.
- `deps`: `dep(...)` declarations that name expected providers.
- `providers`: provider names this rule can return.
- `capabilities`: command surfaces this rule supports, such as
  `build`, `run`, `test`, or `metadata`.
- `examples`: starter example slugs exposed to agents.
- `impl`: optional function that declares actions and returns a provider
  record.

Attributes can be configurable unless their schema sets
`configurable = False`. Non-configurable attributes reject `select`
during validation before the implementation runs.

## Starter Examples

`examples` points at runnable starter bundles for a rule. Use it when a
caller should be able to discover the rule, choose a starter by intent,
and materialize a working target without reading prose docs.

```python
apple_library = rule(
    docs = "Compiles Swift, Objective-C, C, and C++ sources into a linkable Apple module.",
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform"),
    ],
    providers = ["apple_linkable", "apple_module"],
    capabilities = [capability("build", ["default"])],
    examples = [
        "apple-library-minimal",
        "apple-library-with-objc",
    ],
    impl = _apple_library_impl,
)
```

`once query rules` lists the available rule kinds with their starter
example slugs. Use `once query schema <kind> --format json` for the full
example bundle:

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
      "use_when": "Start a small Apple library target.",
      "files": [
        {
          "path": "apps/Hello/once.toml",
          "contents": "[[target]]\nname = \"Hello\"\nkind = \"apple_library\"\n..."
        }
      ]
    }
  ]
}
```

Agents should use `use_when` to pick the closest starter, then create
each returned file using its relative `path` and `contents`. MCP callers
get the same data from `once_list_rules` and `once_query_schema`.

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
rules should read provider fields from `ctx["deps"]` instead of
inspecting target kinds.

## Host Globals

The Rust executor exposes generic primitives only:

- `host_arch()` and `host_os()` return normalized host identifiers.
- `workspace_root()` returns the absolute workspace root.
- `host_which(name)` resolves an executable on `PATH`.
- `host_command(argv, env = {})` runs a discovery command and returns
  stdout. Arguments and env values participate in the command-scoped
  cache key.
- `glob(patterns)` expands patterns under the active package and returns
  sorted workspace-relative file paths.
- `declare_output(name)` reserves an output under the target build
  directory.
- `run_action(...)` records a command action for the executor.
- `write_file(path, content)` and `write_bytes(path, bytes)` materialize
  generated files through normal actions.
- `toml_decode(src)` and `json_decode(src)` decode data into Starlark
  values.

Toolchain behavior belongs in Starlark on top of these primitives:
resolver commands, source filtering, file formats, SDK selection,
compiler flags, provider conventions, and action layout.

## Actions

`run_action` accepts:

- `argv`: command and arguments.
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

## Design Rules

Keep rule implementations ecosystem-specific and the executor generic.
If a rule needs to understand a compiler, package manager, SDK, binary
format, or platform naming convention, encode that in Starlark and pass
only declared actions and provider records back to Rust.

Provider records are the cross-rule contract. Prefer small, typed,
documented fields such as output paths, transitive inputs, flags, and
metadata. Avoid making consumers branch on a dependency target kind.

Use `toolchain_identity` for tool versions, resolved compiler paths, SDK
selection, generated file contents, and other non-source inputs that
should invalidate cached actions. Do not log secrets or place secrets in
arguments, provider records, or action metadata.

When adding a public rule, make it discoverable through `once query
rules` and `once query schema`, provide at least one starter example,
and keep the schema, docs, and examples in sync.
