# `credo_lint`

Runs [Credo](https://hexdocs.pm/credo/) for declared Elixir sources and
exposes normalized lint findings.

```toml
[[target]]
name = "lint"
kind = "credo_lint"
srcs = ["lib/**/*.ex"]
```

Optional attributes are `mix`, `config`, `data`, `args`, and `cwd`. The target
emits `once_lint_info` and exposes the `lint` capability.

Use `once query schema credo_lint` for the complete contract and
`once query example credo_lint credo-lint-minimal` for the starter files.
The [Linting guide](/guide/graph/linting) covers materialization, execution,
failure policy, normalized findings, and cache verification.
