# `swiftlint_lint`

Runs [SwiftLint](https://github.com/realm/SwiftLint) for declared Swift
sources and exposes normalized lint findings.

```toml
[[target]]
name = "lint"
kind = "swiftlint_lint"
srcs = ["Sources/**/*.swift"]
```

Optional attributes are `swiftlint`, `config`, `data`, and `args`. The target
emits `once_lint_info` and exposes the `lint` capability.

Use `once query schema swiftlint_lint` for the complete contract and
`once query example swiftlint_lint swiftlint-lint-minimal` for the starter
files. The [Linting guide](/guide/graph/linting) covers materialization,
execution, failure policy, normalized findings, and cache verification.
