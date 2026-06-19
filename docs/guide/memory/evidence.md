# Evidence

Evidence is durable provenance for engineering work. It records a claim
about what happened, what it was about, and which action data supports
it. Evidence is not cache. The action cache answers whether outputs can
be reused; evidence answers whether humans and agents can trust a prior
result when planning the next step.

Today Once records evidence after:

- `once exec`
- `once run`
- `once build`
- `once test`

## Interacting With Evidence

The current interface is the CLI. Use
[`once query evidence`](/reference/cli/query/evidence) to list recent
records, or pass a subject to focus on one command, target, or
capability:

```sh
once query evidence
once query evidence cli:test
```

For scripts and coding agents, use Once's structured output formats
instead of scraping the human rendering:

```sh
once --format json query evidence
once --format toon query evidence cli:test
```

Agents can also use
[`once_query_evidence`](/reference/mcp/tools#once_query_evidence) from
the [MCP tools](/reference/mcp/) catalog. The MCP tool returns the same
record shape as the CLI JSON output, so an agent can inspect the graph,
run the smallest useful check, then query memory without scraping
terminal output.

## What A Record Contains

Action evidence includes:

- **Subject**: The command action or target capability the evidence is about.
- **Kind**: The kind of evidence record, such as an action result.
- **Status**: Whether the action passed or failed.
- **Action Digest**: The content-addressed identity of the action.
- **Input Digest**: The declared input identity when the action has one.
- **Cache State**: Whether the result came from a cache hit, miss, or bypass.
- **Exit Code**: The process exit code recorded for the action.
- **Output Digests**: Digests for captured stdout, stderr, and declared
  outputs when available.
- **Creation Time**: When the evidence record was written.

Storage details live in the [Memory reference](/reference/memory/).

## Why It Helps

Evidence gives Once and coding agents a concrete memory of recent work.
Instead of treating every session as a blank slate, an agent can ask
whether `cli:test` already passed for the current inputs, whether
`app:build` failed for a specific action digest, whether a result came
from cache, or whether a target has no evidence yet.

That changes planning. Once can distinguish known, missing, and stale
information before running anything. A coding agent can use that to run
the smallest useful check instead of repeating the whole suite.

## Cache And Evidence

An action result is the reusable cache fact: given these declared inputs,
this command produced these outputs. When the same action is requested
again, Once can restore the cached result instead of repeating the work.

Evidence is the planning fact around that result. It records that a
subject, such as a command action or target capability, produced a
status at a specific time with specific provenance. That makes the
result inspectable and comparable without turning the evidence itself
into a build artifact.

The distinction matters because engineering decisions usually need more
than reuse. A coding agent needs to know whether a target has fresh test
evidence, whether a failure came from the current inputs, or whether a
passing result was restored from cache. Evidence gives those decisions a
structured record to inspect.

## Stale Evidence

Evidence becomes stale when the facts that supported it no longer match
the current project state. A test result might have passed yesterday,
but if the input digest changed today, the old evidence no longer proves
that the current target is healthy. The same applies when the action
digest changes, the policy or tool that produced the record changes, a
newer record supersedes it, or a retention policy expires it.

Staleness should be derived from evidence and graph state rather than
stored as the source of truth. That lets Once explain why a result can
no longer be trusted.
