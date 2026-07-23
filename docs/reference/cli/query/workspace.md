# `once query workspace`

Describe the configured workspace and graph loading state

## Synopsis

```text
once query workspace [OPTIONS]
```

## Description

Reports the exact workspace root, root manifest state, normalized target operating system and architecture, ordered selection tokens, loaded packages, target count, graph loading errors, and suggested discovery or validation calls.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
