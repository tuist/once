# `once query`

Query the typed build graph

## Synopsis

```text
once query [OPTIONS] <SUBCOMMAND>
```

## Description

Inspectable-first surface for humans and agents. `query targets` lists every declared target id with its rule kind and capabilities; `query capabilities` shows what a specific target exposes (`build`, `run`, `test`); `query schema` returns the typed attribute and provider shape for a rule. All three respect `--format json` so consumers can plan against the graph without scraping prose.

## Options

| Flag | Value | Default | Description |
| --- | --- | --- | --- |
| `-C, --directory` | `<DIR>` |  | Project root. Defaults to the current directory; the cache lives under `<project>/.once/`. Mirrors `make -C` |
| `--format` | `<FORMAT>` | `human` | Output format for Once's structured data (`cache stats`, `run`/`exec` trailers). Defaults to a human-readable rendering; pass `json` or `toon` to get machine-parseable output for scripting and for agent consumers |
| `-v, --verbose` | (flag) | `0` | Increase log verbosity. Repeat for more (-v: info, -vv: debug, -vvv: trace). Overridden by `RUST_LOG` |
| `-q, --quiet` | (flag) | `false` | Suppress human-mode success and progress trailers. Errors and the structured envelope of `--format json`/`toon` still print. Mirrors the `-q` flag of common build tools |
| `--list` | (flag) | `false` | Print the command surface at the current command depth |

## Subcommands

- [`once query targets`](/reference/cli/query/targets)
- [`once query capabilities`](/reference/cli/query/capabilities)
- [`once query schema`](/reference/cli/query/schema)
- [`once query rules`](/reference/cli/query/rules)
- [`once query target`](/reference/cli/query/target)
- [`once query validate-target`](/reference/cli/query/validate-target)

