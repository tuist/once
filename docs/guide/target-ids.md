# Target IDs

Fabrik addresses targets by a path-shaped ID. The CLI and `fabrik.toml`
files use slightly different resolution rules so each context can be
concise without losing precision.

## On the CLI

CLI target IDs are project-root relative by default:

```sh
fabrik build examples/apple/macos/cli/hello
```

Use `./` or `../` when you want the argument resolved from your current
directory instead:

```sh
cd examples/apple/macos/cli
fabrik build ./hello
```

## In `fabrik.toml`

Build-file deps are resolved from the declaring `fabrik.toml` by
default:

- `Greeter` means another target in the same file's directory.
- `../shared/Logging` is relative to that directory.
- `shared/Logging` is project-root relative.

This means a target in `examples/rust/granular/basic-app/fabrik.toml`
can depend on a sibling with just its name, while still being able to
reach across the project with project-root paths when needed.
