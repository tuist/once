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

ExUnit-based test targets are not in scope today. `use ExUnit.Case`
registers test modules inside the BEAM that subsequently runs the
suite, so splitting compile and run into separate cached actions
needs a same-VM design pass. A future iteration will add the right
shape once that's clear; for now, run tests through `mix test` from
within your project.

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

### Concurrency

The daemon socket is per-user, not per-workspace, so any concurrent
`fabrik build` (or `fabrik elixir-compile`) on the same host talks to
the same long-running BEAM. Each connection runs in its own Erlang
process for I/O, but `Code.compile_file/1` and `Code.prepend_path/1`
mutate VM-global state. Letting two compiles overlap there would race
the code path and the loaded-modules table and silently corrupt either
job.

Every compile therefore funnels through one `Fabrik.CompileWorker`
GenServer that processes a single job at a time. Concurrent clients
queue and observe a consistent VM, never each other's `-pa` paths. The
serialization is intentional for v1: the daemon's value prop is
amortizing BEAM startup, not parallel compilation; a future revision
can fan out across `:peer` nodes if benchmarks show queue contention
dominates wall time.

### Backpressure

Pending plus in-flight jobs are capped at a bounded queue (default
`4 × erlang:system_info(schedulers_online)`, overridable via
`FABRIK_ELIXIR_DAEMON_MAX_QUEUE` when starting the daemon). Submissions
beyond the cap get an immediate `{"ok": false, "retryable": true}`
response instead of growing the queue. The `fabrik elixir-compile`
wrapper treats that signal exactly like "no daemon listening" and
falls back to spawning `elixirc` directly for that one action, so a
saturated daemon never blocks progress.

The cap is wired into fabrik's `ResourcePool` via a named-slot axis.
The elixir plugin publishes a `"elixir_compile"` slot whose pool size
defaults to the host's CPU count (see `fabrik_elixir::ELIXIR_COMPILE_SLOT`
and `default_compile_slot_limit`); every elixir action declares a
`ResourceRequest` that reserves one of those slots for the duration
of its run. The CLI passes the published pool size into every Runner
it constructs, so `fabrik build`, `fabrik run`, and `fabrik test` all
gate elixir-action concurrency on the same bound.

The daemon's own `MAX_QUEUE` default uses the same scheduler-count
formula, so the runner and the daemon agree on how many in-flight
compiles the host should absorb. When several `fabrik build`
invocations on the same host share the per-user daemon, the daemon's
bounded queue is the cross-process backstop and the wrapper's busy
fallback keeps individual actions moving even when the queue is
saturated.

Tuning notes for advanced operators:

- Override the daemon's cap with `FABRIK_ELIXIR_DAEMON_MAX_QUEUE` at
  daemon-start time.
- The fabrik-side pool size is currently set from the CLI; future
  work could lift it into `fabrik.toml` so projects can tune it
  without rebuilding fabrik. Keep the two values aligned to avoid
  the runner admitting more actions than the daemon will accept.

## Notes

The launcher script embeds workspace-relative `-pa` paths and locates
the workspace root at run time by walking up to the nearest `.fabrik/`
directory. That keeps the cached launcher byte-identical across
machines with different absolute paths.

A real Elixir toolchain (`elixirc` and `elixir`) is required to build
and run Elixir targets. The repo's own `mise.toml` pins
`elixir = "1.19.5-otp-28"` and `erlang = "28.5"` so the shellspec
suite exercises real compilation; pin the same versions in your
project's `mise.toml` to keep cache keys honest across machines.
