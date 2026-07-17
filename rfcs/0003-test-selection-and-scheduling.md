# Request for Comments 0003: Test Selection And Scheduling

## Summary

Once should represent test selection, planning, scheduling, execution, and
result aggregation as separate decisions. A selected test is semantic work. A
batch is a stable execution envelope. A local worker, remote worker, or fixed
job is only a placement decision.

The implementation creates an immutable `once.test_plan.v1` record and a
separate `once.test_schedule.v1` execution record. The plan contains the
selection policy, changed paths, unmatched paths, selected test targets,
reasons, and stable batches. It deliberately contains no worker or
remote-provider assignment. The schedule records local worker placement and
attempt timing without changing the plan.

This design applies equally to typed targets and scripted graph adapters. It
also gives coding agents an auditable answer to three different questions:

1. Why was this test selected?
2. What exact work was planned?
3. Where and how was that work executed?

## Decision

Once will use an assured hierarchical model:

```text
conservative graph envelope
  -> optional narrowing backed by complete dependency evidence
  -> history-based prioritization
  -> stable execution batches
  -> dynamic local or remote scheduling
  -> attempt and result ledger
  -> periodic fresh full-suite validation
```

Graph structure defines the default correctness boundary. Observed file access
may narrow that boundary only when Once can establish that observation was
complete. Historical and learned signals may order work, but they do not remove
tests unless the caller explicitly chooses an approximate, budgeted policy.

## Goals

- Never make remote placement or worker count part of test selection.
- Explain every selected test and every conservative fallback.
- Keep plan and batch identities stable when runner capacity changes.
- Make retries safe through idempotent result ingestion and logical
  deduplication.
- Support build-once, test-many execution without rebuilding per worker.
- Apply the same dependency model to target kinds and annotated scripts.
- Give coding agents evidence that their tests validate behavior, not merely
  that the tests pass after the change.
- Allow later prioritization and approximate selection without weakening the
  default policy.

## Non-Goals

- Predictive exclusion in the default policy.
- Encoding a specific hosted continuous-integration provider in a test plan.
- Treating a shard number as test identity.
- Reusing an old passing test result during an explicitly fresh audit.
- Requiring a central service to create a plan.

## Research Basis

The model combines the strongest properties of existing systems and published
research:

- Bazel and Buck2 provide conservative graph reachability, hermetic actions,
  and remote execution. Buck2 also separates test discovery from execution.
- Nx makes project-level affectedness approachable, but a general Once surface
  must not assume one package or language ecosystem.
- The [Tuist test sharding proposal](https://community.tuist.dev/t/rfc-test-sharding/929)
  correctly centers built test artifacts, build-once execution, timing-aware
  balancing, and result merging. Once keeps those ideas while avoiding session
  identity tied to a hosted workflow and avoiding a shard count as semantic
  identity.
- Rothermel and Harrold's
  [safe regression test selection](https://doi.org/10.1145/248233.248262)
  establishes the key rule: a safe selector must include every test whose
  behavior may be affected by the change.
- [Ekstazi](https://users.ece.utexas.edu/~gligoric/papers/GligoricETAL15Ekstazi.pdf)
  shows that observed file dependencies can reduce end-to-end time, but its
  assumptions about deterministic behavior and complete dependencies matter.
- [Hybrid dynamic file and static method selection](https://zbchen.github.io/files/ase2024.pdf)
  shows why Once should accept dependency evidence at more than one
  granularity instead of hardcoding target-level selection forever.
- Meta's
  [predictive test selection](https://arxiv.org/abs/1810.05286) and Microsoft's
  [data-driven test selection](https://www.microsoft.com/en-us/research/publication/data-driven-test-selection-at-scale/)
  show substantial infrastructure savings, while Google's
  [assessment of transition-based selectors](https://research.google/pubs/assessing-transition-based-test-selection-algorithms-at-google/)
  shows that predictive exclusion is not a universal correctness mechanism.
- Graham's
  [list scheduling bound](https://doi.org/10.1137/0117039), the work-stealing
  analysis by Blumofe and Leiserson
  [for multithreaded computation](https://doi.org/10.1145/324133.324234), and
  Google's [tail-at-scale analysis](https://research.google/pubs/the-tail-at-scale/)
  support dynamic scheduling, small retryable units, and duplicate attempts
  for genuine stragglers.

## Records

### Selection report

The selection report records:

- schema identifier
- selection mode, safety policy, and evidence class
- normalized changed paths
- paths without a declared graph owner
- selected tests and reasons

An empty changed-path set means a full test scope. A changed manifest or
configured graph module selects every test because graph topology or target
behavior may have changed. A changed path without an owner also selects every
test until Once has complete dependency evidence that proves it irrelevant.

Declared source patterns are matched directly. Selection must not require the
changed file to exist in the current checkout, because deleted and renamed
files still affect tests.

### Test manifest

A test target may expose a manifest of independently runnable test units. Each
unit needs a stable identifier, execution requirements, labels, and optional
dependency evidence. Discovery belongs to target-kind analysis or to a
cacheable discovery action over a built test artifact.

If a target cannot enumerate stable units, the entire target remains one test
unit. This is the current fallback and preserves compatibility with every
target kind.

The current discovery slice projects normalized case results into an immutable
`once.test_manifest.v1` record after a whole-target execution. The record is
persisted separately from the latest result, so an explicit filtered run does
not erase the complete discovered unit set. Before a manifest exists, Once
reports a whole-target fallback instead of guessing test names.

Explicit unit execution is allowed only when the target kind declares case
filtering and translates the stable semantic unit identifier into its native
runner arguments. Affected-test selection remains a complete target scope.
When a current manifest declares sharding support, planning may divide that
scope into file or case batches without omitting any discovered unit. A
missing, stale, or incomplete manifest falls back to one whole-target batch
and refreshes discovery.

### Dependency evidence

Dependency evidence is generic and may come from:

- declared target sources and dependencies
- target-kind metadata
- complete sandbox file-access observation
- static analysis supplied by a target kind
- annotated script inputs and dependency scripts

Every evidence item states its source, granularity, configuration identity,
and whether completeness is enforceable. Incomplete evidence may add tests or
prioritize them. It may not remove tests from the conservative graph envelope.

### Test plan

The test plan combines one selection report with stable execution batches. A
batch contains test-unit identities and execution requirements, but no worker
assignment.

The current implementation emits one batch per selected test target unless a
current manifest declares exact filtering and automatic sharding. File
granularity groups semantic units by their stable file field. Case granularity
creates one batch per semantic unit. Batch identities are derived from their
semantic content, not their ordinal position or the requested worker count.

### Schedule

A schedule assigns plan batches and attempts to execution slots. It may change
as capacity, failures, or durations change. Scheduling records should include:

- plan and batch identity
- attempt identity
- local or remote placement
- execution platform requirements
- queue, start, and finish times
- cancellation, retry, and duplicate-attempt relationships

Fixed hosted jobs are a schedule adapter. They may group stable batches into a
requested number of lanes, but lane membership is not written back into the
test plan.

### Result ledger

Remote execution provides at-least-once physical execution. Once therefore
requires idempotent result ingestion and logical deduplication by plan, batch,
test unit, configuration, and attempt. A late duplicate may contribute
diagnostics, but it cannot double-count the logical test result.

Aggregation must preserve failures when a scheduler or worker terminates early.
Coverage and runner-native result bundles are artifacts of the logical test
run, assembled only from accepted attempts.

## Selection Policy

Once first finds changed graph owners, then traverses reverse dependencies to
test targets. The traversal is iterative and indexed, so cost grows with the
graph and changed paths rather than repeating a full dependency walk for every
test.

Target-level selection is the safe default. Within an affected target, Once
runs every discovered test unit unless it has complete finer-grained dependency
evidence. Coverage alone does not establish that completeness.

Selection modes are:

- `full`: run every test in scope.
- `affected`: use the conservative graph envelope and complete dependency
  evidence.
- `explicit`: run exactly the requested targets or test units.
- `budgeted`: permit approximate exclusion under an explicit time or compute
  budget, report the expected miss risk, and require a later full audit.

The initial implementation exposes `full`, `affected`, and `explicit` plans.

## Prioritization

Prioritization changes order, not inclusion. Useful signals include:

- tests that cover edited behavior through complete dependency evidence
- tests authored or modified in the current change
- recent failures
- historical failure correlation
- predicted duration
- runner startup and fixture affinity

Fail-fast feedback should stream early failures to a person or coding agent.
The final result still represents every planned unit unless the run is recorded
as cancelled or superseded.

## Scheduling And Remote Execution

Native Once scheduling pulls ready batches from a shared queue. Workers request
more work as they finish, which handles duration skew better than a static
partition. The median of recent successful uncached durations seeds
longest-first ordering; it does not fix work to a worker. Cache hits are
recorded as attempts but do not train future physical-execution estimates.

Remote placement is an execution concern. A batch may run locally or remotely
without changing its identity. Platform constraints that change observable
semantics belong to the test action identity. A provider name chosen only for
placement does not.

Speculative duplicate attempts are reserved for measured stragglers. The first
accepted complete result wins, and other attempts are cancelled when possible.
Retries create new attempt identities under the same logical batch.

## Caching And Fresh Audits

Build outputs, discovery manifests, and successful test results may be reused
when their semantic action identity is unchanged. Changing worker count or
placement alone must not invalidate them.

A fresh audit reuses build and discovery outputs but bypasses test-result reuse.
This is required to detect environment drift, nondeterminism, order dependence,
and missing dependency declarations. Periodic full audits measure the recall of
affected selection and produce the evidence needed before introducing any
budgeted policy.

## Coding Agents

Agent-written tests need stronger evidence than a green post-change run. The
recommended validation record is:

```text
agent change and agent-authored test
  -> test fails against the behavior before the change
  -> test passes with the change
  -> existing affected tests pass
  -> optional mutation or differential check challenges the assertion
```

The plan should record which tests were authored or modified by the agent and
prioritize them first. The result ledger should preserve the fail-before and
pass-after relationship. When reverting the complete change is not possible,
selective mutation of the edited behavior is the preferred substitute. Higher
line coverage alone is not sufficient evidence that a generated test detects a
fault.

Long-running plans should be cancellable and supersedable. When an agent edits
the workspace again, Once may keep reusable completed batches whose semantic
identity is unchanged, cancel obsolete work, and schedule the new plan.

## Scripted Graphs

Annotated scripts participate through the same dependency evidence model:

- script files are inputs
- declared input patterns are inputs
- dependency scripts form graph edges
- fingerprints and selected environment values contribute execution identity
- declared outputs remain visible to downstream actions

A script adapted into a typed graph target can therefore select only dependent
tests. A standalone changed script without a graph owner conservatively selects
the full test scope. This creates a safe migration path: adding declarations and
graph edges improves selectivity without changing the test planning model.

## Delivery

The first delivered foundation includes:

- one shared selector for affected-test preview, plan preview, and agent test
  execution
- direct declared-pattern matching for deleted and renamed files
- conservative full selection for graph-definition and unowned-path changes
- reverse-dependency traversal from changed owners to tests
- stable plan and target-batch identities
- explicit unmatched-path reporting
- command-line and agent-tool plan queries

The delivered scheduling slice includes:

- bounded local workers pulling batches from one dynamic queue
- longest-estimated-duration-first ordering from persisted timing history
- attempt records connecting plan, batch, worker, timing, cache state, and
  result status
- command-line and agent controls for worker capacity
- command-line and agent queries for recent batch attempts

The delivered test-unit slice includes:

- stable manifests projected from normalized test cases and retained apart
  from the latest filtered result
- whole-target discovery fallback when no manifest exists
- exact unit plans whose identities include the semantic unit identifier
- generic test filters in the Starlark implementation context
- target-kind-owned translation for Rust libtest, Android local tests, and
  Android instrumentation tests
- command-line and agent manifest queries and explicit unit execution

The next compatible additions are isolated per-batch result paths, automatic
case batching for fresh manifests, remote placement, fresh-audit policy, and
complete sandbox-backed dependency observation. Predictive ordering follows
only after the result ledger can measure it. Approximate exclusion follows only
after full audits can quantify missed failures.

## Rejected Alternatives

### Make shard number part of the test action

Changing worker count would reshape action identities, destroy reusable work,
and couple correctness to scheduling.

### Let prediction define the default selected set

The available evidence shows valuable cost reduction, but also system-specific
results and missed failures. Prediction is appropriate for ordering and an
explicit budgeted mode.

### Require a central planning service

A service is useful for cross-machine timing history and coordination, but the
plan must be reproducible locally from workspace state. Remote coordination is
an optional schedule and evidence provider.

### Infer a fixed shard count from total duration alone

Duration estimates cannot determine the best count without startup overhead,
available capacity, platform restrictions, retry cost, and a target completion
time. Native dynamic scheduling avoids choosing a fixed count. Fixed-job
adapters accept an explicit lane count and balance stable batches within it.
