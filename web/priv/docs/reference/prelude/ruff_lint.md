# `ruff_lint`

Runs [Ruff](https://docs.astral.sh/ruff/linter/) for declared Python sources
and exposes normalized lint findings.

```toml
[[target]]
name = "lint"
kind = "ruff_lint"
srcs = ["src/**/*.py"]
```

Optional attributes are `ruff`, `config`, `data`, and `args`. The default
configuration inputs are `pyproject.toml`, `ruff.toml`, and `.ruff.toml`.
The target emits `once_lint_info` and exposes the `lint` capability.

Use `once query schema ruff_lint` for the complete contract and
`once query example ruff_lint ruff-lint-minimal` for the starter files.
The [Linting guide](/guide/graph/linting) covers Ruff setup, materialization,
execution, failure policy, normalized findings, and cache verification.
