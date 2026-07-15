# `once toolchain inspect`

Print the toolchain contract derived from mise.toml

## Synopsis

```text
once toolchain inspect [OPTIONS]
```

## Description

Shows the workspace tool requests, lock information, selected platform, and
the mise version carried by this Once release.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--platform` | `<PLATFORM>` |  | Mise platform key to inspect, e.g. linux-x64. Defaults to the current host platform |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
