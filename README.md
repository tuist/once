<p align="center">
  <img src="assets/logo.png" alt="Fabrik" width="50%" />
</p>

<p align="center">
  <a href="https://github.com/tuist/fabrik/actions/workflows/fabrik.yml"><img src="https://github.com/tuist/fabrik/actions/workflows/fabrik.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/tuist/fabrik/releases/latest"><img src="https://img.shields.io/github/v/release/tuist/fabrik?display_name=tag&sort=semver" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/tuist/fabrik" alt="License" /></a>
</p>

# Fabrik

Fabrik is a polyglot, agent-native build system. It uses content-addressed actions, structured declarations, and explicit runtime semantics so humans and coding agents can build, run, test, and debug the same graph.

## Quick Start

Install `fabrik`, then initialize a project:

```sh
fabrik --list
fabrik init
fabrik init --templates
fabrik init rust-app --path hello
```

Use the canonical template ids printed by `fabrik init --templates`, for example `rust-app`.

## Documentation

Read the documentation at [fabrik.run](https://fabrik.run).

## License

[MIT](LICENSE).
