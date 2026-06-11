# `once cache action put`

Store an action result

## Synopsis

```text
once cache action put [OPTIONS] [ACTION]
```

## Description

Identify the action either by passing its digest directly, or by declaring its inputs with `--input`; the same declaration can be used on `get` to read back the result.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<ACTION>` | no | Pre-computed action digest |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--input` | `<SPEC>` |  | Input spec (see `cache hash` for the grammar). Repeatable |
| `--exit-code` | `<EXIT_CODE>` | `0` | Process exit code captured for the action. Defaults to 0 since the common case is recording a success |
| `--stdout` | `<STDOUT>` |  | Optional blob digest containing captured stdout |
| `--stderr` | `<STDERR>` |  | Optional blob digest containing captured stderr |
| `--output` | `<OUTPUTS>` |  | Declared output as `workspace/path=blob_digest`. Repeatable |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |

