# `once test`

Test a declared target

## Synopsis

```text
once test [OPTIONS] [TARGET]
```

## Description

Builds the target as needed, then executes its `test` capability through the action cache. Output paths and result groups are owned by the rule that exposes the capability.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<TARGET>` | no | Target id, e.g. `apps/ios/AppTests` or `./AppTests` |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |

