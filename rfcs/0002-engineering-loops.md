# RFC 0002: Once Engineering Loops

## Summary

Once should grow from a cacheable build substrate into a substrate for
engineering loops. Build, run, and test remain the first mature loop because
they are the easiest to make deterministic. The broader product opportunity is
to model the recurring work around software engineering: dependency review, CI
repair, pull request review, release readiness, production regression triage,
and agent-assisted implementation.

This RFC proposes a small extension to the current model:

1. Keep deterministic work as cacheable actions.
2. Record non-deterministic work as durable evidence, not cache hits.
3. Model multi-step workflows as typed loop targets.
4. Make human gates, policy gates, and agent checks queryable graph concepts.
5. Prefer executable hooks, tools, tests, and schemas over growing prompt
   context.

The result is an engineering graph rather than only a build graph. Build graph
targets are still first-class. Engineering loops sit above them and call into
the same capability, cache, diagnostic, runtime, and MCP surfaces.

## Motivation

Once already describes actions with inputs, outputs, environment, working
directory, runtime metadata, and provider capabilities. That is a narrow waist
between developer intent and execution infrastructure.

Agentic engineering stresses the same waist in more places than builds:

- Agents run several implementations in parallel.
- CI repair loops repeatedly collect failures, change code, and verify fixes.
- Dependency changes need security, license, and native dependency checks.
- Pull request review needs durable findings, decisions, and baseline
  comparison.
- Release readiness combines tests, API compatibility, docs drift, human
  approval, and provenance.
- Production telemetry should feed back into specifications and verification.

These loops should not be expressed only as prose in agent instructions. Prose
context rots, diverges, and cannot reliably block unsafe changes. Whenever a
team can express a rule as a hook, test, typed workflow, schema, or executable
policy, Once should make that the default shape.

## Influences

This direction reconciles three outside ideas with Once without absorbing their
entire product surface.

Fabro's strongest idea is a deterministic harness around non-deterministic
agents: version-controlled workflow graphs, human gates, model routing,
parallel branches, event streams, run checkpoints, and resumability.

Swamp's strongest idea is typed automation resources: model types with input
and output schemas, YAML definitions agents can author, immutable data artifacts,
vault-backed secrets, remote workers, and queryable stored data.

Jose Valim's "Agentic engineering is still software engineering" argues for a
discipline Once should encode directly: use deterministic hooks and tools
before context, keep humans in the loop where automation has not earned trust,
and prefer formal, executable, and test specifications over descriptive prose.

Once's contribution is the narrower engineering substrate: content-addressed
actions, action cache records, typed graph targets, capabilities, structured
diagnostics, remote execution, runtime introspection, and MCP-first discovery.

## Goals

- Let teams model recurring engineering workflows as typed graph data.
- Preserve strict cache semantics for deterministic actions.
- Store non-deterministic agent and human work as evidence with provenance.
- Make evidence queryable by humans, CLI scripts, and MCP clients.
- Let loop targets call existing `build`, `run`, and `test` capabilities.
- Keep loop definitions schema-checked and editable through graph operations.
- Support local execution first, while leaving room for remote workers and
  shared evidence stores.

## Non-Goals

- Replace CI systems, issue trackers, chat products, or incident tools.
- Turn Once into a general business automation engine.
- Replay agent output as if it were a deterministic build artifact.
- Make natural language specs the primary source of truth.
- Require a hosted Once service for the first useful implementation.
- Add arbitrary executable graph construction to target declarations.

## Product Model

Developer-facing language should avoid presenting this as a stack of layers.
The clearer framing is:

```text
Once gives every project a graph and a memory.
```

The project graph answers:

```text
What exists?
What depends on what?
What can be built, tested, run, reviewed, or released?
```

Project memory answers:

```text
What happened?
What passed or failed?
What does that prove?
Is that proof still fresh?
What should happen next?
```

The product concepts then become:

- Project graph: source-controlled intent, including targets, dependencies,
  capabilities, providers, policies, gates, and loop definitions.
- Runs: execution history for Once invocations, actions, cache state, and
  action results.
- Evidence: durable claims from runs, tools, agents, policies, and humans.
- Views: current answers computed from graph, runs, and evidence, such as
  affected tests, stale evidence, blocked work, and release readiness.
- Loops: repeatable workflows that use the graph, runs, evidence, and views to
  coordinate engineering work over time.

The pitch should stay concrete:

```text
The graph tells Once what matters.
Runs tell Once what happened.
Evidence tells Once what can be trusted.
Views tell developers and agents what to do next.
Loops make recurring engineering work repeatable.
```

Project memory should live locally in `.once/once.sqlite`. Evidence is one
family of tables in that database, not a separate evidence-only database.
Future remote persistence should exchange evidence and run records with
provenance, not synchronize raw SQLite database files.

The distinction between action results and evidence records is important.

An action result says:

```text
Given these declared inputs, this command produced these outputs.
```

An evidence record says:

```text
At this time, with this context and these tool/model identities, this check,
agent, policy, or human produced this conclusion.
```

Action results can be reused. Evidence records can be inspected, compared, and
used as prerequisites, but they must not be blindly replayed as truth.

## Loop Targets

A loop target is a typed target whose primary output is evidence, not a build
artifact.

```toml
[[target]]
name = "new-dependency-review"
kind = "engineering_loop"

[target.attrs]
trigger = { files = ["Cargo.toml", "Cargo.lock"] }
stages = [
  { id = "metadata", kind = "action", run = "cargo metadata" },
  { id = "advisory-scan", kind = "action", run = "cargo deny check advisories" },
  { id = "agent-review", kind = "agent_check", policy = "review-new-dependency" },
  { id = "approve", kind = "human_gate" },
]
```

The exact TOML shape is illustrative. The invariant is that loop stages are
typed data, validated against schemas, and available through the same query and
MCP surfaces as graph targets.

### Stage Kinds

Initial stage kinds should be small:

- `action`: run a deterministic command through the action cache.
- `capability`: run an existing target capability such as `build`, `run`, or
  `test`.
- `agent_check`: ask an agent for a structured finding or recommendation.
- `policy_check`: evaluate deterministic policy over files, graph data, or
  evidence.
- `human_gate`: suspend until a human approves, rejects, or requests revision.
- `evidence_query`: select prior evidence to feed a later stage.

The key rule is that each stage declares whether its result is cacheable,
observable, or gated.

## Evidence Records

Evidence records should be content-addressed where possible and stored in the
same spirit as CAS and action results. A record should include:

- `id`: stable digest or generated id.
- `target`: loop target or graph target id.
- `stage`: stage id when part of a loop.
- `kind`: action outcome, finding, decision, approval, rejection, telemetry
  import, policy result, or agent output.
- `inputs`: declared file, graph, environment, action, and prior evidence
  inputs.
- `outputs`: structured data plus links to CAS blobs for logs, reports, diffs,
  and artifacts.
- `producer`: tool, model, human, provider, or policy identity.
- `status`: pass, fail, warning, blocked, approved, rejected, stale, or
  superseded.
- `timestamp`: wall-clock time for provenance.
- `expires_at`: optional lifecycle hint for evidence that should be refreshed.
- `supersedes`: prior evidence replaced by this record.
- `diagnostics`: structured diagnostics and candidate repairs.

Sensitive values must not be stored in evidence. The record may reference a
secret by logical name or provider handle, but not capture plaintext.

## Graph Query Expressions

Once now has a read-only graph expression surface:

```sh
once query 'MATCH (t:Target)-[:EXPOSES]->(c:Capability {name: "test"}) RETURN t.id'
```

Engineering loops should build on this surface rather than introduce a separate
query language for every new concept. The first implementation can keep
`once query evidence` and `once query gates` as ergonomic commands, but they
should be views over a shared query model.

In that model, deterministic build graph data already has nodes such as
`Target`, `Capability`, and `Provider`. Engineering-loop data can add nodes such
as:

- `Loop`
- `Stage`
- `Evidence`
- `Gate`
- `Policy`
- `Decision`

And relationships such as:

- `REQUIRES`: a loop or stage requires evidence, a policy, or a gate.
- `PRODUCED`: a stage produced an evidence record.
- `HAS_EVIDENCE`: a target, capability, or loop has evidence.
- `BLOCKED_BY`: a loop or gate is blocked by missing or stale evidence.
- `SUPERSEDES`: an evidence record replaces an older record.
- `OBSERVED`: telemetry evidence refers back to a target or capability.

Future questions should be expressible in the same shape:

```sh
once query 'MATCH (l:Loop {id: "release"})-[:BLOCKED_BY]->(g:Gate) RETURN g.id, g.status'
once query 'MATCH (t:Target)-[:HAS_EVIDENCE]->(e:Evidence {status: "stale"}) RETURN t.id, e.kind'
once query 'MATCH (p:Policy {name: "sdk-api"})<-[:REQUIRES]-(l:Loop) RETURN l.id'
```

This changes the product framing: Once should not only provide named query
commands. It should expose an inspectable graph of engineering state where
named commands are stable shortcuts for common graph expressions.

## Day-To-Day Examples

### New Dependency Review

A developer or agent changes a dependency lockfile. Once detects that the
`new-dependency-review` trigger matches and starts the loop.

```sh
once loop run checks/new-dependency-review
once query evidence checks/new-dependency-review --latest
```

Human output:

```text
Dependency: reqwest 0.13
Reason: needed for HTTP client support
Advisory scan: pass
License policy: pass
Native dependencies: none
Agent review: no install scripts or unexpected transitive risk
Decision: approved
```

The security scan and metadata collection are cacheable actions. The agent
review and approval are evidence. Future runs can compare against this decision
but should refresh the review when inputs, policy, model, or advisory data
change.

### CI Repair

An agent starts from a failing capability rather than raw logs.

```sh
once loop run repair-ci --target cli:test
once query failures --target cli:test --similar
```

The loop stages are:

1. Collect the latest failing action evidence.
2. Query similar failures and prior repairs.
3. Ask an agent for a structured repair plan.
4. Apply the graph or file edit through the normal edit path.
5. Run focused tests.
6. Run required checks.
7. Record a repair summary.

If the agent discovers a missing docs generation step, the loop records a
diagnostic such as:

```text
code: generated_reference_drift
target: cli:test
repair: regenerate CLI reference before rerunning this test
```

The next agent does not rediscover the same failure from raw stderr. It queries
the prior evidence.

### Pull Request Review

Once can model review as typed checks instead of one open-ended instruction.

```toml
[[target]]
name = "pr-review"
kind = "review_loop"

[target.attrs]
checks = [
  "behavioral-regression",
  "missing-tests",
  "docs-drift",
  "public-api-change",
]
required_evidence = ["diff-summary", "focused-tests"]
```

Daily use:

```sh
once loop run reviews/pr-review --base main
once query evidence reviews/pr-review --select findings
```

Example finding:

```text
Finding: public SDK changed without docs evidence
Status: should_fix
Required repair: update SDK guide or mark the API change internal
```

The review output becomes a structured finding with status, target, severity,
supporting evidence, and repair operations. It is not just a paragraph in a pull
request comment.

### Feature Implementation

A requested feature can run through a loop that enforces the repo's engineering
shape.

```text
spec -> graph edit -> example -> validation -> docs -> verification
```

For a new toolchain target kind, the loop might require:

1. Query the nearest existing target kind schema.
2. Create or edit the target kind schema.
3. Add a runnable starter example.
4. Validate examples.
5. Expose schema and examples through MCP.
6. Update reference docs.
7. Run focused tests.

Skipping the starter example should fail as a structured diagnostic:

```text
code: missing_rule_example
target: android_library
repair: add a runnable example and reference it from the target kind schema
```

### Release Readiness

A release loop combines cacheable verification, non-cacheable evidence, and a
human gate.

```toml
[[target]]
name = "release"
kind = "engineering_loop"

[target.attrs]
stages = [
  { id = "tests", kind = "capability", target = "//:workspace", capability = "test" },
  { id = "lint", kind = "action", run = "cargo clippy --workspace --all-targets -- -D warnings" },
  { id = "sdk-api", kind = "policy_check", policy = "sdk-api-compatibility" },
  { id = "release-notes", kind = "agent_check", policy = "release-notes-drift" },
  { id = "approve", kind = "human_gate" },
]
```

Status:

```text
Tests: cached pass
Lint: fresh pass
SDK API: changed Swift API
Release notes: missing public API note
Gate: blocked
```

Once does not merely run commands. It knows which evidence is required before
the release gate can pass.

### Production Feedback

Observability imports should become evidence linked to targets and tests.

```sh
once evidence import sentry --project once-site
once query incidents --since 7d
once loop run production-regression-review
```

Example evidence:

```text
Incident cluster: auth callback timeout
Linked target: auth-service
Suggested verification: add integration coverage for expired provider tokens
Decision: create follow-up issue
```

This keeps production feedback inside the engineering loop without making Once
own the monitoring system.

## Query Surface

The first named query shape should be intentionally small:

```sh
once query loops
once query loop checks/new-dependency-review
once query evidence checks/new-dependency-review
once query evidence --kind finding --status should_fix
once query gates --pending
once query decisions --target release
once query why-blocked release
```

These commands should coexist with graph expressions. Named commands serve the
common cases; graph expressions answer composition questions that would
otherwise turn into a long tail of bespoke subcommands.

MCP tools should mirror these commands:

- `once_query_loops`
- `once_query_loop`
- `once_query_evidence`
- `once_query_gates`
- `once_query_decisions`
- `once_query_why_blocked`
- `once_apply_loop_edit`

As with current graph edits, agents should edit loop targets through validated
operations instead of hand-editing TOML text.

## Runtime Surface

The current runtime session model should become the live control surface for
loop runs. A loop run should expose:

- Current stage.
- Pending gates.
- Streamed action logs.
- Agent check status.
- Produced evidence.
- Candidate repairs.
- Checkpoints.
- Cancellation and resume points.

This overlaps with Fabro's run event and checkpoint model, but Once should keep
the state tied to targets, capabilities, evidence, and action digests rather
than a standalone agent workflow product.

## Storage

Local storage can start under Once's existing state and cache roots:

- CAS remains the place for large immutable blobs.
- Action cache remains the place for deterministic action results.
- Evidence storage records structured, queryable provenance.
- Runtime sessions record live state and compact into evidence at completion.

The first storage backend can be local. A future shared backend can sync evidence
across machines and CI, but the local path must remain useful.

## Trust And Freshness

Evidence must carry freshness rules. A dependency approval from last month may
still explain why a dependency exists, but it should not automatically satisfy a
new advisory policy run.

Loop stages should be able to declare refresh triggers:

- File inputs changed.
- Graph inputs changed.
- Tool or model identity changed.
- Policy version changed.
- External data source changed.
- Evidence expired.
- Human explicitly invalidated it.

Queries should distinguish current evidence from historical evidence.

## Relationship To The Build Graph

The build graph remains the most important deterministic loop. Engineering loops
should depend on it, not replace it.

Examples:

- A release loop depends on workspace tests.
- A repair loop depends on failing target capabilities.
- A review loop depends on affected tests and docs drift checks.
- A production feedback loop suggests new tests or graph edges.

In implementation terms, loop stages can call graph capabilities. Graph targets
do not need to know about loop targets unless a policy intentionally creates
that dependency.

## Implementation Slice

The first slice should avoid a full workflow engine. It should prove the data
model and daily value.

1. Define an `EvidenceRecord` Rust model with JSON serialization.
2. Add a local evidence store with append, query, and latest-by-target lookup.
3. Record evidence for existing action executions without changing cache
   behavior.
4. Add `once query evidence` for action evidence.
5. Project evidence into the read-only graph query model.
6. Add a minimal `engineering_loop` schema with `action`, `capability`, and
   `human_gate` stages.
7. Add `once query loops` and `once query gates`.
8. Add one built-in example loop: dependency review or release readiness.
9. Expose the same surface over MCP.
10. Add stale evidence diagnostics when loop inputs changed.
11. Add agent checks only after deterministic stages and gates work.

This order keeps the first implementation grounded. Evidence records add value
even before agent orchestration exists.

## Open Questions

- Should evidence live in the same crate as cache/action records or in a
  separate runtime crate?
- Should evidence ids be pure content digests, generated ids, or a hybrid?
- How much query language is needed before a simple filter model becomes
  painful?
- Which evidence should be checked into git, if any?
- Should loop targets live in package manifests or in a separate workspace-level
  manifest section?
- How should human approvals work in terminal-only local mode?
- What is the minimal agent check interface that stays provider-neutral?
- Which external observability imports should be first-class versus generic
  evidence importers?
- How should shared evidence avoid leaking private logs, secrets, and customer
  data?

## References

- Fabro documentation: https://docs.fabro.sh/
- Swamp manual: https://swamp-club.com/manual/
- Jose Valim, "Agentic engineering is still software engineering":
  https://gist.github.com/josevalim/27e2fd147ea3765010ed5ada7162db1e
