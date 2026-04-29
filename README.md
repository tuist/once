# Fabrik

A polyglot, agent-native build system. Bazel's ambitions, none of its mistakes.

> Status: pre-alpha. Walking skeleton. Not yet usable for real builds.

See [docs/design.md](docs/design.md) for the v0 design and
[docs/roadmap.md](docs/roadmap.md) for the phased execution plan.

## Install

Once the first release lands, fabrik will be installable via mise's
ubi backend:

```sh
mise use --global ubi:tuist/fabrik@latest
fabrik --version
```

Releases are produced automatically from `main` on every push that
contains conventional commits (`feat:`, `fix:`, `perf:`, `refactor:`).
See [.github/workflows/release.yml](.github/workflows/release.yml).

## Build from source

```sh
mise install              # installs the pinned Rust + shellspec + git-cliff toolchain
cargo build --release     # builds the workspace
cargo test                # unit tests
mise exec -- shellspec    # end-to-end CLI tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

## What works today

```sh
# Run a command through the action cache.
cargo run -- run -e PATH=/usr/bin:/bin -- /bin/sh -c 'echo hello'

# Cache it under a workspace-relative cwd; second run replays from cache.
cargo run -- -C /tmp/ws run --cwd subdir -e PATH=/usr/bin:/bin -- /bin/sh -c 'pwd'

# Per-action timeout (child is killed via kill_on_drop).
cargo run -- run --timeout-ms 1000 -e PATH=/usr/bin:/bin -- /bin/sh -c 'sleep 5'

# Cache stats.
cargo run -- cache stats

# Verbose tracing (or set RUST_LOG=debug).
cargo run -- -vv run -e PATH=/usr/bin:/bin -- /bin/sh -c 'echo hi'
```

`fabrik run` executes a command and caches its `(stdout, stderr, exit code)`
keyed by argv + env + cwd + timeout. The cache lives in `<workspace>/.fabrik/`.
Failures are not cached unless you pass `--cache-failures`.

## Layout

- `crates/fabrik-cas` — async content-addressed store with streaming put.
- `crates/fabrik-core` — `Action`, `Runner` (semaphore-bounded), executor.
- `crates/fabrik-cli` — the `fabrik` binary.
