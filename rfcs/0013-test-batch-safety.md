# Request for Comments 0013: Safe And Reproducible Test Batches

## Decision

Extend the existing test selection and scheduling model rather than add a
second sharding subsystem. Semantic batches remain independent of worker
count. Target kinds own discovery, fixture boundaries, exact filtering, and
native artifacts. The scheduler owns bounded placement and duration-based
ordering. A public batch selector makes one automatic batch reproducible
without constructing runner arguments or changing a target's sources.

This change delivers local batch safety and reproduction. Distributed leases,
static CI lane export, cross-machine timing history, speculative attempts,
and coverage merging are separate extensions of the same model, not claims
of this implementation.

## Community Research

Research was conducted against issue bodies and discussion on 2026-10-09.
Closed reports are historical evidence, not assertions that current Bazel
still has every reported bug.

| Report | Observation | Once requirement |
| --- | --- | --- |
| [Bazel #18228](https://github.com/bazelbuild/bazel/issues/18228) and [#18339](https://github.com/bazelbuild/bazel/issues/18339), closed | A Python runner ignored the shard protocol and ran every test in every shard. Checking the capability acknowledgement was added later. | Require declared filtering support and verify the observed case set equals the requested set, not merely that it contains it. |
| [Bazel #22028](https://github.com/bazelbuild/bazel/issues/22028), closed | The shard acknowledgement error obscured a runner startup failure. The fix checked acknowledgement only for passing tests. | Preserve subprocess exit status and stderr. A failed process cannot become a success because a result file exists. Missing or invalid evidence is an additional reason to fail a successful exact batch. |
| [Bazel #7319](https://github.com/bazelbuild/bazel/issues/7319), closed | Filters matching no tests failed for Java while other runners behaved differently. | Resolve exact units and batches before execution. An exact nonempty request with empty observed results fails; an empty conservative selection is not an empty successful shard. |
| [Bazel #2113](https://github.com/bazelbuild/bazel/issues/2113), closed | More shards made quick tests slower because JVM startup outweighed parallelism. | Preserve file/target fixture boundaries and the existing `batching = "target"` opt-out. Capacity does not force finer batching. No automatic speedup claim. |
| [Bazel #28819](https://github.com/bazelbuild/bazel/issues/28819), open | Users could not conveniently rerun one shard for CI or debugging without modifying test sources. | Expose an exact semantic batch selector through CLI and MCP, with current-plan validation and nearby valid IDs. |
| [Bazel #15155](https://github.com/bazelbuild/bazel/issues/15155), closed | Execution-log sorting did not distinguish shards reliably. The discussion says the compact log superseded the problem. | Keep batch identity in every attempt and deterministic result ordering independent of completion order. |
| [Bazel #14446](https://github.com/bazelbuild/bazel/issues/14446), open, and [#7129](https://github.com/bazelbuild/bazel/issues/7129), closed | Heuristic shard-count behavior and documentation were confusing. | Document actual planning and fallback behavior, not unspecified auto-sharding heuristics. |
| [Bazel #19428](https://github.com/bazelbuild/bazel/issues/19428), open | Streamed-output messaging did not match execution placement. | Rendering and output format must not redefine the plan or its execution policy. |

The normative [Bazel sharding protocol](https://bazel.build/reference/test-encyclopedia#test-sharding)
passes shard index/count to a runner. Once instead passes explicit semantic
units, so it can verify actual membership. A capability declaration alone is
not execution evidence, and a runner can still lie about observations; this
contract is not a security boundary against malicious adapters.

## Tuist Comparison

The current Tuist Xcode workflow builds test products once, creates a hosted
plan, emits a CI matrix, and executes filtered tests on each runner. Its server
uses the 90th percentile of durations from a 30-day window, with fallback
estimates for unknown units. Longest-processing-time packing balances work.
Suite packing also tries module affinity to reduce artifact downloads, falling
back to plain packing if the estimated makespan worsens by more than five
percent. Gradle uses compiled test-class discovery and the same hosted planner.

Xcode suite history is not a complete current inventory. Tuist's final
catch-all shard runs everything except suites assigned to earlier shards.
Module products come from the current built inventory, and the catch-all
receives all required module products. These are correctness and artifact
locality mechanisms, not just bin packing.

Once keeps its existing conservative alternative: only a current complete
manifest can produce automatic exact batches. Missing or stale discovery
executes the whole target. Filtered runs never replace complete discovery.
History orders work; it cannot authorize omission. File-oriented batches retain
fixture affinity. Target-oriented batches are appropriate for expensive startup
or runners that cannot isolate finer units. Native artifact paths remain
batch-owned; coverage artifacts are retained but not semantically merged.

A future distributed schedule should reuse built artifacts through the action
cache, keep placement outside semantic identity, and ingest attempts with leases
and idempotency. A fixed CI matrix should assign existing batches to lanes,
never repartition test cases based on lane count. Neither needs a required
hosted planning service or a new configuration file.

## Implementation Plan

1. Tighten normalized result validation. Reject duplicate case identities and
   reject unrequested cases for nonempty exact selections. Whole-target and
   summary-only runners retain their existing shape.
2. Make batch success depend on process success and valid, passing evidence for
   exact work. Never synthesize a passing exact case when evidence is missing.
   Reject overlapping batch results instead of silently replacing one case.
3. Validate semantic plans before scheduling: canonical identities, unique
   batches, disjoint exact scopes, and no whole-target/exact overlap. For a
   complete-target selection, exact scopes must cover the entire manifest;
   explicit unit and batch requests deliberately cover only their scope.
   Preserve deterministic aggregation and resource-budgeted scheduling.
4. Add `once test <target> --test-batch ID` and
   `once query test-plan --target <target> --test-batch ID`, and the matching
   optional MCP arguments. A batch request requires exactly one explicit
   target. Resolve against the current target plan,
   preserve the batch's unit set and ID, and reject obsolete or unknown IDs
   with corrected syntax and a shortlist. Batch selection is mutually exclusive
   with unit and affected/full selection.
5. Update public testing, MCP, and Starlark module references and add regression
   coverage for ignored filters, missing/duplicate units, overlapping plans,
   startup failures, deterministic schedules, batch replay, stale IDs, and
   worker-count-independent identity.

No action identity changes are introduced for worker placement. A changed
semantic unit set naturally creates a new batch ID; replay is not permission
to run a stale inventory. Exact reproduction concerns the test scope, not a
promise that changed code or environment reproduces the same outcome.

## Design Review Decisions

Claude's adversarial review confirmed this bounded scope and requested explicit
replay scoping, readable errors, duplicate-result checks, and union completeness.
These are included above. The review suggested tolerating extra cases for
compatibility; this design deliberately rejects that alternative. The existing
`runner_args` contract already promises lossless exact filtering. Silently
dropping extras would accept the ignored-filter behavior that motivated the
change. Auxiliary setup diagnostics belong in artifacts or runner metadata;
parameterized cases need distinct discovered IDs. This is a deliberate
validation tightening, not a new opt-in capability name.

A current fingerprint proves freshness only against declared discovery inputs,
not inventory completeness. Whole-target adapters must report complete discovery;
dynamic registration requires declaring every input that changes collection.
A runner that omits cases without reporting a failure cannot be detected from
those observations alone. Periodic whole-target runs remain important. No
arbitrary batch-count warning threshold is introduced: file/target batching and
explicit worker/memory limits remain the control surface.

## Validation

Run focused Rust validator, planner, scheduler, and architecture tests; then
formatting, affected-crate Clippy, a release build, and the Shellspec test
capability suite. Record actual outcomes, including unavailable prerequisites.
Ask Claude to challenge the design before implementation and the final diff
before publishing. Address correctness findings with regressions.
