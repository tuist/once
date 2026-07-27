# `once lint`

Run static analysis for a declared target

## Synopsis

```text
once lint [OPTIONS] [TARGET]
```

## Description

Executes the target's `lint` capability, normalizes its report, and returns a failing status when a finding meets `--fail-on`.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<TARGET>` | no | Target id, such as `quality/python` or `./python` |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--sandbox` | `<SANDBOX>` | `off` | Local filesystem sandbox policy for command actions |
| `--fail-on` | `<FAIL_ON>` | `warning` | Lowest finding severity that makes this command fail |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
