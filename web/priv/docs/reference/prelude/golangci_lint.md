# `golangci_lint`

Runs [golangci-lint](https://golangci-lint.run/docs/) for declared Go sources
and exposes normalized lint findings.

```toml
[[target]]
name = "lint"
kind = "golangci_lint"
srcs = ["**/*.go"]

[target.attrs]
packages = ["./..."]
```

Optional attributes are `golangci_lint`, `config`, `data`, `args`, `packages`,
and `cwd`. The target emits `struo_lint_info` and exposes the `lint` capability.

Use `struo query schema golangci_lint` for the complete contract and
`struo query example golangci_lint golangci-lint-minimal` for the starter
files. The [Linting guide](/guide/graph/linting) covers materialization,
execution, failure policy, normalized findings, and cache verification.
