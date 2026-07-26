---
next: false
---

# Evidence

Evidence is a durable record of an action outcome. It complements the cache:
the cache stores reusable results, while evidence describes what happened when
Once requested a result.

## Run And Query An Action

This walkthrough uses `scripts/greet.sh` from the
[Scripted Workflows guide](/guide/scripted/). Give it a new input value so the
first run computes a result:

```sh
printf 'evidence %s\n' "$(date +%s)" > message.txt
ONCE_CACHE_PROVIDER=local ./scripts/greet.sh
```

Now inspect the newest record. The example uses
[`jq`](https://jqlang.org/) to select the final item from Once's
[JavaScript Object Notation](https://www.json.org/json-en.html) output:

```sh
once --format json query evidence \
  | jq '.[-1] | {subject, status, cache, action_digest, input_digest}'
```

The record reports `passed` and `miss`. Run the unchanged script again and
repeat the query:

```sh
ONCE_CACHE_PROVIDER=local ./scripts/greet.sh
once --format json query evidence \
  | jq '.[-1] | {subject, status, cache, action_digest, input_digest}'
```

The new record reports `hit` and keeps the same action and input digests. This
shows that Once restored the previously computed result.

Change the declared input, run again, and compare the two newest records:

```sh
printf 'evidence changed %s\n' "$(date +%s)" > message.txt
ONCE_CACHE_PROVIDER=local ./scripts/greet.sh
once --format json query evidence \
  | jq '.[-2:] | map({status, cache, action_digest, input_digest})'
```

The latest record reports a cache miss with different action and input digests.
The previous record remains available as history, but its digests describe the
earlier input state.

## Filter Evidence By Subject

Use [`once query evidence`](/reference/cli/query/evidence) to list all records
with human-readable output:

```sh
once query evidence
```

Graph actions have stable target subjects, so you can filter them by target or
by target and capability:

```sh
once query evidence cli
once query evidence cli:test
```

A literal `once exec` command uses its action digest as its subject. Use the
structured output when you need to select or compare those command records.

Tools can request the same record shape through
[`once_query_evidence`](/reference/mcp/tools#once-query-evidence) in the
[Model Context Protocol](https://modelcontextprotocol.io/) catalog.

## Read A Record

An action evidence record contains:

- **Subject**: The command action, target, or target capability.
- **Status**: Whether the action passed or failed.
- **Action digest**: The identity of the complete action.
- **Input digest**: The identity of declared inputs when available.
- **Cache state**: Whether the result was a hit, miss, or bypass.
- **Exit code**: The process exit code.
- **Stream digests**: References to captured standard output and standard error
  when available.
- **Output digests**: References to declared outputs when available.
- **Creation time**: When Once wrote the record.

Compare digests before applying an older result to current work. Matching input
and action digests show that two records describe the same action state;
different digests show that some relevant part of that state changed.

## Next

Use the [Memory reference](/reference/memory/) for the storage contract. Continue
to the [Graph guide](/guide/graph/) when a scripted workflow needs stable target
subjects and named capabilities such as build, run, or test.
