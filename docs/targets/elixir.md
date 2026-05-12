# Elixir

Fabrik supports granular Elixir targets in `fabrik.toml`. Each target
becomes one cacheable action that runs `elixirc` once and produces a
per-target `.ebin` directory of `.beam` files.

```toml
[[elixir.library]]
name = "greeting"
srcs = ["lib/greeting.ex"]

[[elixir.binary]]
name = "hello"
srcs = ["lib/hello.ex"]
deps = ["greeting"]
entry = "Hello"
```

Build it:

```sh
fabrik build examples/elixir/granular/basic-app/hello
```

Run the produced launcher:

```sh
./.fabrik/out/examples/elixir/granular/basic-app/hello
```

## Target Kinds

- `elixir.library`: compiles one or more `.ex` sources into a `.ebin`
  directory of `.beam` files. Dependents pick it up through `-pa` at
  compile time.
- `elixir.binary`: same compile, plus a tiny launcher script that
  walks up to the workspace root at run time and execs `elixir` with
  the right `-pa` path and entry module. Requires an `entry` attribute
  naming a module with `main/1`.
- `elixir.test`: compiles like `elixir.library`. The ExUnit-based test
  runner is not yet wired up; the target compiles cleanly today and a
  future iteration will run it through `fabrik test`.

## Cache Behavior

- One `elixirc` invocation per target. A source edit to a leaf
  module invalidates that target and its reverse dependencies, not
  unrelated targets.
- Dependents key on the producer's full `action_digest`, so a change
  upstream propagates through the graph.
- The `.ebin` directory is restored from the CAS on a cache hit just
  like any other declared output.

## XDG state layout

Fabrik routes state through XDG Base Directory variables so the same
binary stays well behaved on shared hosts, CI runners, and inside
sandboxes. The split:

- `<workspace>/.fabrik/out/...` - build outputs and runtime sessions.
  Per-project, lives in your checkout, consumed locally.
- `$XDG_CACHE_HOME/fabrik/cas` - content-addressed blobs and action
  results. Shared across projects on the same host so identical
  actions hit.
- `$XDG_RUNTIME_DIR/fabrik` - daemon sockets and other ephemeral
  runtime files. Falls back to a uid-keyed tempdir on hosts where
  `XDG_RUNTIME_DIR` is unset (macOS by default).
- `$XDG_DATA_HOME/fabrik` - long-lived materialized assets like the
  embedded elixir compile daemon script.

Override any of these by setting the corresponding env var. Tests do
this per case via the shellspec helper to keep each run hermetic.

## Compile daemon

Each elixir target's action runs through a `fabrik elixir-compile`
wrapper that talks to a long-lived compile daemon when one is reachable
and falls back to spawning `elixirc` directly otherwise. The wrapper's
argv is identical in both modes, so daemon presence is invisible to the
cache. Outputs must therefore be byte-identical across backends, which
they are: the daemon uses `Code.compile_file/2` against the same
sources, dep paths, and Elixir version.

Start the daemon in a separate terminal (or under a process supervisor)
before kicking off a build:

```sh
fabrik elixir-daemon start          # listens on .fabrik/elixir-daemon.sock
fabrik elixir-daemon status         # probe whether a daemon is reachable
```

Override the socket path with `--socket /custom/path.sock` or by setting
`FABRIK_ELIXIR_DAEMON_SOCKET` in the environment. The latter is declared
on every elixir action's env, so per-shell overrides flow into the
spawned wrappers.

The daemon is opt-in: without it, builds still work via the direct
`elixirc` fallback. Run it when you want to amortize BEAM startup across
many actions, especially on cold builds and in CI.

## Notes

The launcher script embeds workspace-relative `-pa` paths and locates
the workspace root at run time by walking up to the nearest `.fabrik/`
directory. That keeps the cached launcher byte-identical across
machines with different absolute paths.

A real Elixir toolchain (`elixirc` and `elixir`) is required to build
and run Elixir targets. Add it to your `mise.toml` to pin the version
used for cache keys.
