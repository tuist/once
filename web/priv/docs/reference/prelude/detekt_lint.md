# `detekt_lint`

Runs [detekt](https://detekt.dev/) for declared Kotlin sources and exposes
normalized lint findings.

```toml
[[target]]
name = "lint"
kind = "detekt_lint"
srcs = ["src/**/*.kt"]
```

Optional attributes are `detekt`, `config`, `data`, and `args`. The target
emits `struo_lint_info` and exposes the `lint` capability.

Use `struo query schema detekt_lint` for the complete contract and
`struo query example detekt_lint detekt-lint-minimal` for the starter files.
The [Linting guide](/guide/graph/linting) covers materialization, execution,
failure policy, normalized findings, and cache verification.
