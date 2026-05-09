# Fabrik

> [!WARNING]
> Fabrik is beta software. The CLI, local cache, Rust targets, task targets, and iOS simulator app flow are usable, but target schemas and plugin behavior can still change.

Fabrik is a polyglot, agent-native build system. It uses content-addressed actions, structured declarations, and explicit runtime semantics so humans and coding agents can build, run, test, and debug the same graph.

## Quick Start

```sh
mise install
mise exec -- cargo build --release
mise exec -- target/release/fabrik targets
```

Try the checked-in examples:

```sh
mise exec -- target/release/fabrik build //examples/rust-app:hello
mise exec -- target/release/fabrik test //examples/rust-app:greeting_test
mise exec -- target/release/fabrik build //examples/ios-app:Demo
```

## Documentation

- [Docs index](docs/README.md)
- [Rust](docs/rust.md)
- [Apple and iOS](docs/apple.md)
- [Tasks](docs/tasks.md)
- [Cache and execution](docs/cache-and-execution.md)
- [Design](docs/design.md)
- [Roadmap](docs/roadmap.md)

## Development

```sh
mise exec -- cargo test --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --all -- --check
mise exec -- cargo build --release
mise exec -- shellspec
```

## License

[MIT](LICENSE).
