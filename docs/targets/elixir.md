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

## Notes

The launcher script embeds workspace-relative `-pa` paths and locates
the workspace root at run time by walking up to the nearest `.fabrik/`
directory. That keeps the cached launcher byte-identical across
machines with different absolute paths.

A real Elixir toolchain (`elixirc` and `elixir`) is required to build
and run Elixir targets. Add it to your `mise.toml` to pin the version
used for cache keys.
