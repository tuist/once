# `once edit materialize-example`

Materialize a target kind starter inside the workspace

## Synopsis

```text
once edit materialize-example [OPTIONS] <KIND> <SLUG>
```

## Description

Copies the complete example bundle without printing file contents. Existing files with identical contents are kept. Any conflicting file rejects the complete operation before Once writes anything.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<KIND>` | yes | Target kind that owns the example |
| `<SLUG>` | yes | Example slug from `once query schema` |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--destination` | `<DIR>` |  | Workspace-relative directory that receives the example |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
