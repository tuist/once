# `once test`

Test a declared target

## Synopsis

```text
once test [OPTIONS] [TARGET]
```

## Description

Builds the target as needed, then executes its `test` capability through the action cache. Output paths and result groups are owned by the target kind that exposes the capability. With `--changed-path` or `--all`, stable target batches are pulled from a duration-informed dynamic queue. `--jobs` caps local workers without changing the plan or batch identities.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<TARGET>` | no | Target id, such as `tests/unit` or `./unit` |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--sandbox` | `<SANDBOX>` | `off` | Local filesystem sandbox policy for command actions |
| `-j, --jobs` | `<COUNT>` |  | Maximum number of test batches to execute concurrently. Defaults to the host's available parallelism for an affected plan |
| `--all` | (flag) | `false` | Run every discovered test target through the dynamic scheduler |
| `--changed-path` | `<PATH>` |  | Select tests affected by a workspace-relative changed path. Repeat for multiple paths. Cannot be combined with a target id |
| `--test-unit` | `<UNIT>` |  | Run one current, filterable unit from `once query test-manifest`. The request is rejected before scheduling when the target does not support exact filtering or the unit is absent from the manifest |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
