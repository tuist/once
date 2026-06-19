# `once query validate-target`

Validate a proposed `[[target]]` table against its target kind schema

## Synopsis

```text
once query validate-target [OPTIONS]
```

## Description

Reads `{ "target": { ... } }` from `--file` or, if omitted, from stdin.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--file` | `<PATH>` |  | Path to a JSON file. When omitted, the JSON document is read from stdin |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
