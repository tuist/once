# `once runtime`

Runtime session inspection and control

## Synopsis

```text
once runtime [OPTIONS] <SUBCOMMAND>
```

## Description

Starts long-lived target runs under a small supervisor and persists their stdout, stderr, and status under `<workspace>/.once/runtime/<session>/`. `runtime status`, `runtime logs`, and `runtime stop` let agents and humans observe or stop a run after the original command has returned. `runtime rpc` serves a JSON-RPC control socket for a session that already has runtime metadata.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |

## Subcommands

- [`once runtime start`](/reference/cli/runtime/start)
- [`once runtime status`](/reference/cli/runtime/status)
- [`once runtime logs`](/reference/cli/runtime/logs)
- [`once runtime stop`](/reference/cli/runtime/stop)
- [`once runtime rpc`](/reference/cli/runtime/rpc)

