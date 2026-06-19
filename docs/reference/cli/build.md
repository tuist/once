# `once build`

Build a declared target

## Synopsis

```text
once build [OPTIONS] [TARGET]
```

## Description

Resolves the target id against the workspace graph, ensures every transitive dep is built first, and executes the target's `build` capability through the action cache. Targets that match a cached action key reuse the prior outputs; everything else runs and lands its declared outputs in `<workspace>/.once/out/<target>/`. Use `once query targets` to list available ids.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<TARGET>` | no | Target id, such as `services/api/Api` or `./Api` |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
