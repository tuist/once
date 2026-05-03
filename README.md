# Fabrik

A polyglot, agent-native build system. Bazel's ambitions, none of its mistakes.

> [!WARNING]
> 🚧 **Pre-alpha.** The CLI runs, the cache works, and a Cargo-backed
> dogfood target builds Fabrik itself. Fine-grained Rust targets and
> full language plugins are still under active development. See the
> [roadmap](docs/roadmap.md) for what lands when.

## What you get

- 🌍 **One build system for the whole polyglot stack.** Rust, Go, C/C++,
  TypeScript, Python, Java/Kotlin, Elixir, Swift, Android, iOS.
- ⚡ **Trustworthy, content-addressed caching** that's shareable across
  machines via the Bazel REAPI protocol.
- 🎯 **Honest boundaries.** When fidelity has to drop (Gradle, Vite, Mix),
  Fabrik says so out loud rather than pretending.
- 🤖 **Agent-native.** The build graph is a typed, queryable data
  structure. Humans and AI agents talk to the same gRPC API.
- 📈 **OpenTelemetry-native** with build-specific semantic conventions.

The full picture is in [docs/design.md](docs/design.md).

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
# Run a declared target.
fabrik run //hello:hello

# Cache an arbitrary command (substrate escape hatch).
fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'echo hello'

# A second identical invocation replays the cached stdout/stderr/exit
# without re-running the command.
fabrik exec -e PATH=/usr/bin:/bin -- /bin/sh -c 'echo hello'

# Cache stats.
fabrik cache stats
```

The cache lives under `<workspace>/.fabrik/`. Use `-C <dir>` to point at
a different workspace, the same way `make -C` works.

## Contributing

Phase 0 (walking skeleton: CAS, action executor, CLI) shipped in 0.1.0.
The next phase brings the Rust language plugin so Fabrik can build
itself. Track progress in the [roadmap](docs/roadmap.md).

To build from source:

```sh
mise install
mise exec -- cargo build --release
mise exec -- cargo test --workspace
mise exec -- shellspec
```

Once a local release binary exists, dogfood the build graph with Fabrik:

```sh
mise exec -- target/release/fabrik targets
mise exec -- target/release/fabrik run //:fabrik
```

CI runs lint, tests, and a Windows compile check on every PR. See
[.github/workflows/ci.yml](.github/workflows/ci.yml) and
[CONTRIBUTING](docs/contributing.md) (coming soon).

## Releases

Releases are driven by conventional commits and `git-cliff`. The
release tooling is pinned in [mise.toml](mise.toml), so install it with:

```sh
mise install git-cliff
```

Useful release tasks:

```sh
mise run release:detect
mise run release:changelog
mise run release:notes --version <version>
```

[.github/workflows/release.yml](.github/workflows/release.yml) packages
release archives and publishes GitHub releases that mise can install
through the `github:` backend shown above.

## License

[MIT](LICENSE).
