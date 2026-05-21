<p align="center">
  <img src="assets/logo.png" alt="Fabrik" width="50%" />
</p>

<p align="center">
  <a href="https://github.com/tuist/fabrik/actions/workflows/fabrik.yml"><img src="https://github.com/tuist/fabrik/actions/workflows/fabrik.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/tuist/fabrik/releases/latest"><img src="https://img.shields.io/github/v/release/tuist/fabrik?display_name=tag&sort=semver" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/tuist/fabrik" alt="License" /></a>
</p>

# Fabrik

Fabrik is a polyglot automation kernel for humans and agents. It sits between your team, their coding agents, and the execution infrastructure beneath them: local cache, remote runners, and the toolchains they invoke. Content-addressed actions, structured declarations, and explicit runtime semantics give humans and coding agents one graph to build, run, test, and debug.

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
