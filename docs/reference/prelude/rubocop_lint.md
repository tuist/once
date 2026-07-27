# `rubocop_lint`

Runs [RuboCop](https://docs.rubocop.org/rubocop/) for declared Ruby sources
and exposes normalized lint findings.

```toml
[[target]]
name = "lint"
kind = "rubocop_lint"
srcs = ["lib/**/*.rb"]
```

Optional attributes are `rubocop`, `ruby`, `config`, `data`, and `args`. The
target emits `once_lint_info` and exposes the `lint` capability.

Use `once query schema rubocop_lint` for the complete contract and
`once query example rubocop_lint rubocop-lint-minimal` for the starter files.
The [Linting guide](/guide/graph/linting) covers materialization, execution,
failure policy, normalized findings, and cache verification.
