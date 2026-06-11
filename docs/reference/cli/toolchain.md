# `once toolchain`

Inspect the project toolchain contract

## Synopsis

```text
once toolchain [OPTIONS] <SUBCOMMAND>
```

## Description

Reports the toolchains a project pins (Rust, Swift, mise) and the resolved versions Once will use when running cacheable scripts or graph actions. Pair with `once query schema` when debugging "why did the cache miss?" questions where the toolchain identity is suspect.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |

## Subcommands

- [`once toolchain inspect`](/reference/cli/toolchain/inspect)

