# Fabrik

A polyglot, agent-native build system. Bazel's ambitions, none of its mistakes.

> [!WARNING]
> 🚧 **Pre-alpha.** Not yet usable for real builds. The CLI runs and the cache works, but
> there is no language plugin yet (it's the next phase). See the
> [roadmap](docs/roadmap.md) for what lands when.

## Install

The recommended path is [mise](https://mise.jdx.dev) with the GitHub backend:

```sh
mise use --global github:tuist/fabrik@latest
fabrik --version
```

Pin to a specific release if you want reproducibility:

```sh
mise use --global github:tuist/fabrik@0.1.0
```

Or download a prebuilt archive directly from
[releases](https://github.com/tuist/fabrik/releases). Each release ships
binaries for:

- 🐧 Linux x86_64
- 🍎 macOS x86_64 and arm64
- 🪟 Windows x86_64

## Quick taste

```sh
# Run a command through the action cache.
fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'echo hello'

# A second identical invocation replays the cached stdout/stderr/exit
# without re-running the command.
fabrik run -e PATH=/usr/bin:/bin -- /bin/sh -c 'echo hello'

# Cache stats.
fabrik cache stats
```

The cache lives under `<workspace>/.fabrik/`. Use `-C <dir>` to point at
a different workspace, the same way `make -C` works.

## What the design promises

- **One build system for the whole polyglot stack.** Rust, Go, C/C++,
  TypeScript, Python, Java/Kotlin, Elixir, Swift, Android, iOS.
- **Trustworthy, content-addressed caching** that's shareable across
  machines via the Bazel REAPI protocol.
- **Honest boundaries.** When fidelity has to drop (Gradle, Vite, Mix),
  Fabrik says so out loud rather than pretending.
- **Agent-native.** The build graph is a typed, queryable data
  structure. Humans and AI agents talk to the same gRPC API.
- **OpenTelemetry-native** with build-specific semantic conventions.

The full picture is in [docs/design.md](docs/design.md).

## Project status

Phase 0 (walking skeleton: CAS, action executor, CLI) shipped in 0.1.0.
The next phase brings the Rust language plugin so Fabrik can build
itself. Track progress in the [roadmap](docs/roadmap.md).

## Build from source

```sh
mise install
cargo build --release
cargo test
mise exec -- shellspec
```

CI runs lint, tests, and a Windows compile check on every PR. See
[.github/workflows/ci.yml](.github/workflows/ci.yml) and
[CONTRIBUTING](docs/contributing.md) (coming soon).

## License

[MIT](LICENSE).
