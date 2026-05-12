# Rules and Targets

Fabrik build files describe targets. A target names a rule, and the
rule decides how attributes become actions, providers, runnable
metadata, and runtime metadata.

```toml
[[target]]
name = "cli"
rule = "rust.binary"

[target.attrs]
srcs = ["src/main.rs"]
deps = ["core"]
edition = "2021"
```

This is the canonical shape. Older domain-specific tables such as
`[[rust.binary]]` and `[[apple.simulator_app]]` are compatibility sugar
for built-in rules.

## Consumer Model

Rust library and binary:

```toml
[[target]]
name = "core"
rule = "rust.library"

[target.attrs]
srcs = ["src/lib.rs"]
edition = "2021"

[[target]]
name = "cli"
rule = "rust.binary"

[target.attrs]
srcs = ["src/main.rs"]
deps = ["core"]
edition = "2021"
```

iOS simulator app:

```toml
[[target]]
name = "Demo"
rule = "apple.simulator_app"

[target.attrs]
platform = "ios"
bundle_id = "dev.example.demo"
srcs = ["Sources/App.swift"]
minimum_os = "17.0"
```

Nx-style cached command:

```toml
[[target]]
name = "bundle"
rule = "command"

[target.attrs]
argv = ["pnpm", "vite", "build"]
src_globs = ["src/**/*.ts", "src/**/*.tsx", "index.html"]
outputs = ["dist"]
env = { NODE_ENV = "production" }
cache = true
```

Runtime-aware command:

```toml
[[target]]
name = "dev"
rule = "command"

[target.attrs]
argv = ["pnpm", "vite", "--host", "127.0.0.1"]
cache = false

[target.runtime]
kind = "web_server"
capabilities = ["logs", "http"]

[[target.runtime.interface]]
name = "logs"
kind = "stream"
argv = ["tail", "-f", ".fabrik/runtime/dev/stdout.log"]
```

Run any runnable target with the same verb:

```sh
fabrik run cli
fabrik run Demo
fabrik run --runtime-rpc dev
```

## Built-In Rules

- `command`: generic process rule for cached or uncached commands.
- `rust.library`, `rust.binary`, `rust.test`, `rust.proc_macro`.
- `cargo.binary`, `cargo.build_script`.
- `apple.simulator_app`, `apple.ios_app`, `apple.swift_library`,
  `apple.static_framework`, `apple.dynamic_framework`,
  `apple.macos_command_line_application`.

## Rule Authoring

Rules are not defined in `fabrik.toml`. `fabrik.toml` consumes rules by
declaring targets. Rule packages should be authored separately.

The intended extension language is Starlark because it gives Fabrik a
deterministic rule and macro language without requiring third-party
rules to be compiled into Fabrik itself.

An eventual rule package could look like this:

```python
def _command_impl(ctx):
    output_files = [ctx.actions.declare_file(path) for path in ctx.attr.outputs]
    ctx.actions.run(
        argv = ctx.attr.argv,
        inputs = ctx.files.srcs,
        outputs = output_files,
        env = ctx.attr.env,
    )
    return [
        DefaultInfo(files = output_files),
        RunInfo(argv = ctx.attr.argv),
    ]

command = rule(
    implementation = _command_impl,
    attrs = {
        "argv": attr.string_list(mandatory = True),
        "srcs": attr.label_list(allow_files = True),
        "outputs": attr.string_list(),
        "env": attr.string_dict(),
        "cache": attr.bool(default = True),
    },
    runnable = True,
)
```

Macros should live in the same Starlark layer and expand to rule
invocations:

```python
def rust_cli(name, srcs, deps = []):
    rust_binary(
        name = name,
        srcs = srcs,
        deps = deps,
        edition = "2021",
    )
```

Fabrik still owns scheduling, caching, CAS storage, runtime sessions,
and the final action execution. Starlark rules should analyze
attributes and declare actions; they should not run builds directly.
