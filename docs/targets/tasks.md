# Command Targets

Fabrik supports checked-in command targets through the built-in
`command` rule. Run them with `fabrik run`.

```toml
[[target]]
name = "print"
rule = "command"

[target.attrs]
argv = ["/bin/sh", "-c", "cat tasks/input.txt"]
srcs = ["input.txt"]
```

```sh
fabrik run tasks/print
```

## Fields

- `argv`: command and arguments.
- `srcs`: package-relative inputs that participate in the cache key.
- `src_globs`: package-relative glob inputs.
- `outputs`: project-root-relative outputs to capture and restore from CAS.
- `env`: environment variables passed to the process.
- `cwd`: project-root-relative working directory.
- `cache`: defaults to `true`. Set to `false` for side-effecting tasks.
- `timeout_ms`: optional timeout.
- `cpu_slots`: optional CPU slot request. Defaults to 1.
- `memory_bytes`: optional memory request. Defaults to 0, which means no memory budget request.

Environment values use a TOML subtable:

```toml
[[target]]
name = "with_env"
rule = "command"

[target.attrs]
argv = ["/bin/sh", "-c", "printf \"$MESSAGE\""]

[target.attrs.env]
MESSAGE = "hello"
```

## Cache Behavior

Tasks are cacheable by default. The cache key includes argv, env, cwd, declared inputs, declared outputs, timeout, and resource request. A cache hit replays stdout, stderr, exit code, and restores declared outputs.

Use `cache = false` for tasks with runtime side effects:

```toml
[[target]]
name = "counter"
rule = "command"

[target.attrs]
argv = ["/bin/sh", "-c", "printf run >> tasks/log"]
cache = false
```

Uncached tasks still run through the same bounded executor, but every invocation gets a fresh action key.

## Build-Like Tasks

Use `srcs`, `outputs`, and the default `cache = true` when a command behaves like a build step:

```toml
[[target]]
name = "bundle"
rule = "command"

[target.attrs]
argv = ["./scripts/bundle.sh"]
src_globs = ["src/**/*.js"]
outputs = [".fabrik/out/app/bundle.js"]
```

This stays a `command` rule target; there is no separate build-task
namespace. Domain-specific rules such as `rust.binary`, `cargo.binary`,
and `apple.simulator_app` are still preferred when Fabrik has native
semantics for the toolchain.

## Runtime Metadata

Add a nested `runtime` table when a command launches or attaches to
software that an agent should inspect or interact with through a stable
surface. Runtime-aware commands are always uncached, regardless of the
`cache` field.

```toml
[[target]]
name = "ios_demo"
rule = "command"

[target.attrs]
argv = ["xcrun", "simctl", "launch", "booted", "dev.fabrik.demo"]

[target.runtime]
kind = "ios_simulator"
target = "App/Demo"
capabilities = ["logs", "ui_tree", "ui_action"]

[[target.runtime.interface]]
name = "logs"
kind = "stream"
argv = ["xcrun", "simctl", "spawn", "booted", "log", "stream"]
description = "Stream simulator logs"

[[target.runtime.interface]]
name = "ui_tree"
kind = "accessibility"
argv = ["axe", "describe-ui", "--udid", "booted"]
description = "Inspect the visible accessibility hierarchy"
```

Under `--format json` and `--format toon`, the run record includes a `runtime` object with:

- `kind`: the runtime family, for example `ios_simulator`.
- `subject`: the target or runtime object being operated on.
- `session`: session directory when `fabrik run --runtime-rpc` is active.
- `rpc`: JSON-RPC transport metadata when `fabrik run --runtime-rpc` is active.
- `capabilities`: coarse feature names such as `logs`, `screenshot`, `ui_tree`, or `ui_action`.
- `interfaces`: named commands an agent can use after launch.

`apple.simulator_app` targets also emit a default `ios_simulator` runtime descriptor after launch. It advertises log streaming, screenshots, accessibility tree inspection, and accessibility-targeted taps so agents have a common starting point for simulator inspection.
