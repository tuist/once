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

You can inspect it with:

```sh
once query evidence
once query evidence cli:test
```

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

The local records are stored in the memory database at `.once/once.sqlite`.
Large output bytes are not stored in SQLite. The database stores digests
for stdout, stderr, and declared outputs; the byte payloads stay in the
content-addressed store.

## Why It Helps

Evidence gives Once and coding agents a concrete memory:

```text
cli:test passed for these inputs.
app:build failed for this action digest.
this result came from a cache hit.
this target has no evidence yet.
```

That changes planning. An agent can ask what is already known, what is
missing, and what became stale after a change. It can run the smallest
useful check instead of repeating the whole suite.

## Cache And Evidence

An action result says:

```text
Given these declared inputs, this command produced these outputs.
```

An evidence record says:

```text
At this time, this subject produced this status with this provenance.
```

Action results can be reused by the cache. Evidence can be queried,
compared, and used to decide what should happen next, but it should not
be treated as a reusable build artifact.

## Stale Evidence

Evidence becomes stale when the facts that supported it no longer match
the current project state. Common causes are:

- the input digest changed
- the action digest changed
- the policy or tool that produced the record changed
- the record was superseded by newer evidence
- the record expired by policy

Staleness should be derived from evidence and graph state rather than
stored as the source of truth. That lets Once explain why a result can
no longer be trusted.
