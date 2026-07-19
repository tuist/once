# `once query`

Query the typed build graph

## Synopsis

```text
once query [OPTIONS] [QUERY] <SUBCOMMAND>
```

## Description

Inspectable-first surface for humans and agents. `query
targets` lists every declared target id with its target kind
and capabilities; `query capabilities` shows what a specific
target exposes (`build`, `run`, `test`); `query schema`
returns the typed attribute and provider shape for a target kind;
`query example` returns the files in a chosen starter; `query script` validates
an annotated script contract; `query validate-workspace` checks the
complete loaded graph; and `query evidence` lists durable action evidence
captured from prior executions. A quoted
`MATCH ... RETURN ...` expression can explore the graph through
a read-only Cypher-like pattern. All query surfaces respect
`--format json` and `--format toon` so consumers can plan
against the graph without scraping prose.

## Query Expressions

`once query '<QUERY>'` accepts a read-only subset of Cypher backed
by the Cypher tree-sitter grammar. The first supported shape is a
single `MATCH` pattern with optional `WHERE` equality predicates
and explicit `RETURN` projections.

```sh
once query 'MATCH (app:Target {id: "services/api/Api"})-[:DEPENDS_ON*]->(dep:Target) RETURN dep.id, dep.kind'
once query 'MATCH (t:Target)-[:EXPOSES]->(c:Capability {name: "test"}) RETURN t.id'
```

Supported labels are `Target`, `Capability`, and `Provider`. Labels
use the `:Label` form, for example `(t:Target)`. Bare node names
without a colon are aliases, so `(Target)` binds a variable named
`Target` instead of filtering by the `Target` label. Supported
relationships are `DEPENDS_ON`, `EXPOSES`, and `EMITS`. The `*`
suffix on a relationship performs transitive traversal, for example
`[:DEPENDS_ON*]`.

String literals can be quoted with single or double quotes and
support `\n`, `\r`, `\t`, `\\`, `\"`, and `\'` escapes. Other
escape forms, including Unicode escapes, are rejected.

## Arguments

| Argument | Required | Description |
| --- | --- | --- |
| `<QUERY>` | no | Read-only Cypher-like graph query expression |

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
- [`once query example`](/reference/cli/query/example)
- [`once query target-kinds`](/reference/cli/query/target-kinds)
- [`once query module-contract`](/reference/cli/query/module-contract)
- [`once query external-source`](/reference/cli/query/external-source)
- [`once query target`](/reference/cli/query/target)
- [`once query tests`](/reference/cli/query/tests)
- [`once query affected-tests`](/reference/cli/query/affected-tests)
- [`once query test-plan`](/reference/cli/query/test-plan)
- [`once query test-results`](/reference/cli/query/test-results)
- [`once query test-manifest`](/reference/cli/query/test-manifest)
- [`once query test-attempts`](/reference/cli/query/test-attempts)
- [`once query evidence`](/reference/cli/query/evidence)
- [`once query validate-target`](/reference/cli/query/validate-target)
- [`once query script`](/reference/cli/query/script)
- [`once query validate-workspace`](/reference/cli/query/validate-workspace)
- [`once query validate-module`](/reference/cli/query/validate-module)
