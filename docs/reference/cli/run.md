# `once run`

Run a declared target

## Synopsis

```text
once run [OPTIONS] [TARGET]
```

## Description

Resolves the target id against the workspace graph and executes its `run` capability through the action cache. Use `--remote` to ask a compute provider to execute the command.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<TARGET>` | no | Target id, e.g. `examples/hello/hello` or `./hello` |

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--sandbox` | `<SANDBOX>` | `off` | Local filesystem sandbox policy for command actions |
| `--visible` | (flag) | `false` | Ask graph target kinds to open a visible runtime interface when supported |
| `--runtime-rpc` | (flag) | `false` | Serve a local JSON-RPC runtime control socket for this run |
| `--runtime-rpc-socket` | `<RUNTIME_RPC_SOCKET>` |  | Runtime RPC socket path. Defaults to `.once/runtime/<session>/control.sock` |
| `--remote` | (flag) | `false` | Run the target's action on a compute provider |
| `--compute` | `<PROVIDER>` |  | Compute provider used with --remote. Defaults to the configured execution provider |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
