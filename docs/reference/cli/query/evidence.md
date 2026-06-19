# `once query evidence`

List durable evidence records, optionally filtered by subject

## Synopsis

```text
once query evidence [OPTIONS] [SUBJECT]
```

## Description

Evidence records are provenance for action outcomes. They record what happened after `once exec`, `once run`, `once build`, or `once test`: the subject, status, action digest, input digest when available, cache state, exit code, and captured output digests when available. Evidence is queryable history; it does not change action-cache reuse rules.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<SUBJECT>` | no | Subject id, e.g. `cli` or `cli:test` |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
