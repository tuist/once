# `once mcp`

Expose Once's graph and memory queries to a coding agent over MCP

## Synopsis

```text
once mcp [OPTIONS]
```

## Description

Speaks the Model Context Protocol over stdio so an agent host (Claude Desktop, an IDE plug-in, the Anthropic SDK) can call `once_query_targets`, `once_query_capabilities`, `once_query_schema`, `once_query_example`, and `once_query_evidence` as tools and get JSON back without scraping prose. Mounts inspection tools by default; pass `--allow-run` to expose side-effectful build, run, and runtime session tools.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `--workspace` | `<DIR>` |  | Workspace root the MCP tools resolve targets against. Defaults to the value of the global `-C/--directory` flag (or the current directory) |
| `--allow-run` | (flag) | `false` | Advertise and allow side-effectful execution tools |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |
