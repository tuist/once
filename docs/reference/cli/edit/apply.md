# `once edit apply`

Apply a batch of operations to one `once.toml` atomically

## Synopsis

```text
once edit apply [OPTIONS]
```

## Description

Reads a JSON document matching the `once_apply_edit` MCP tool input shape (`{ "package": "...", "operations": [...] }`) from `--file` or, if omitted, from stdin. On success, the manifest is rewritten and the resolved path is printed. On failure, structured diagnostics are emitted and the manifest is left untouched.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--file` | `<PATH>` |  | Path to a JSON file. When omitted, the JSON document is read from stdin |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |

