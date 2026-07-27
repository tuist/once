# `eslint_lint`

Runs [ESLint](https://eslint.org/) for declared JavaScript and TypeScript
sources and exposes normalized lint findings.

```toml
[[target]]
name = "lint"
kind = "eslint_lint"
srcs = ["src/**/*.ts"]
```

Optional attributes are `eslint`, `node`, `config`, `data`, and `args`. Once
prefers the package-local `node_modules/.bin/eslint` executable when present.
The target emits `once_lint_info` and exposes the `lint` capability.

Use `once query schema eslint_lint` for the complete contract and
`once query example eslint_lint eslint-lint-minimal` for the starter files.
The [Linting guide](/guide/graph/linting) covers materialization, execution,
failure policy, normalized findings, and cache verification.
