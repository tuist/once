# `once cache`

Cache management

## Synopsis

```text
once cache [OPTIONS] <SUBCOMMAND>
```

## Description

Inspect, read, and write the content-addressed cache that every Once action runs through. `cache stats` reports counts and on-disk size; `cache blob` and `cache action` expose the CAS and action-result tables as primitives for debugging, reproducibility checks, and external tooling. Useful for answering "did this run hit the cache?" without scraping command output.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |

## Subcommands

- [`once cache stats`](/reference/cli/cache/stats)
- [`once cache blob`](/reference/cli/cache/blob)
- [`once cache action`](/reference/cli/cache/action)

