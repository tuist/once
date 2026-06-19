# `once cache action get`

Fetch an action result

## Synopsis

```text
once cache action get [OPTIONS] [ACTION]
```

## Description

Identify the action either by passing its digest directly, or by declaring its inputs with `--input`; the same declaration must be used on `put` to write under the same key.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<ACTION>` | no | Pre-computed action digest |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--input` | `<SPEC>` |  | Input spec (see `cache hash` for the grammar). Repeatable; inputs are hashed in order and combined into the action digest |
| `--if-success` | (flag) | `false` | Exit 0 only when there is a hit AND the recorded exit code is 0. On miss or on a cached failure, exit non-zero |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
