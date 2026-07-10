# `once exec`

Execute a literal action through the cache

## Synopsis

```text
once exec [OPTIONS] [ARGV]
```

## Description

Low-level action surface for direct commands and script adapters. The cache key is the full argv, declared environment variables, optional working directory, and optional timeout. A second invocation with the same key reuses the captured stdout, stderr, and exit code. With `--script`, or when argv looks like `<runtime> <script> [args...]` and the file has `once` headers, Once applies script-aware parsing instead.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<ARGV>` | no | Command and arguments. Use `--` to separate from once flags |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--sandbox` | `<SANDBOX>` | `off` | Local filesystem sandbox policy for the command action |
| `--script` | (flag) | `false` | Interpret argv as `<runtime> <script> [args...]` and apply `once` headers from the script file. Useful as the explicit form, for example `once exec --script bash scripts/build.sh`, and for directly executable scripts via a shebang such as `#!/usr/bin/env -S once exec -- bash` |
| `-e` | `<ENV>` |  | Pass an environment variable to the command. Repeatable |
| `--cwd` | `<CWD>` |  | Working directory, relative to the project root. Must not be absolute or escape the project |
| `--timeout-ms` | `<MS>` |  | Per-action timeout in milliseconds. The child is killed if it exceeds the deadline |
| `--cache-failures` | (flag) | `false` | Cache non-zero exits the same way zero exits are cached. Off by default; transient failures shouldn't poison the cache |
| `--remote` | (flag) | `false` | Run the command on a compute provider |
| `--compute` | `<PROVIDER>` |  | Compute provider used with --remote. Defaults to the configured execution provider |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
