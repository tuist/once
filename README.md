# Fabrik

A polyglot, agent-native build system. Bazel's ambitions, none of its mistakes.

> Status: pre-alpha. Phase 0 walking skeleton. Not yet usable for real builds.

See [docs/design.md](docs/design.md) for the v0 design and
[docs/roadmap.md](docs/roadmap.md) for the phased execution plan.

## Quickstart

```sh
mise install      # installs the pinned Rust toolchain
cargo build       # builds the workspace
cargo test        # runs unit tests
```

## What works today (Phase 0)

```sh
cargo run -- run -e PATH=/usr/bin:/bin -- /bin/sh -c 'echo hello'
cargo run -- cache stats
```

`fabrik run` executes a command and caches its `(stdout, stderr, exit code)`
keyed by argv + env. A second invocation with the same key replays the
cached result without re-running the command. The cache lives in
`./.fabrik/`.

## Layout

- `crates/fabrik-cas` — local content-addressed store and action-result cache.
- `crates/fabrik-core` — action types and execution.
- `crates/fabrik-cli` — `fabrik` binary.
