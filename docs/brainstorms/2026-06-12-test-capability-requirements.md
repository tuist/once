---
date: 2026-06-12
topic: test-capability
---

# Test Capability

## Problem Frame

Once needs a standard test interface that coding agents can use to discover,
run, observe, and query test results across native runners and ecosystems.
Rules should own runner-specific knowledge, such as ShellSpec, Swift Testing,
XCTest, Ruby test frameworks, Python test frameworks, Node test frameworks, and
Android test frameworks. The Rust foundation should only understand generic
graph capabilities, declared actions, outputs, cache outcomes, diagnostics, and
normalized observations.

The first dogfood target is a ShellSpec fixture that mirrors Once's own e2e
setup. ShellSpec specs are script-shaped and already validate the CLI's external
contract, so modeling them through Once should prove that rules can bridge
existing test tools while Once selects and caches the tests affected by a
change. After this ships in a released Once version, the next migration step is
to replace the repo's direct e2e ShellSpec invocation with Once-modeled tests.
The ShellSpec slice must prove the generic contract without making ShellSpec the
implicit shape every other ecosystem has to imitate.

## Requirements

**Agent Test Interface**

- R1. Agents can discover testable targets through the same graph and schema
  surfaces used for other rule capabilities.
- R2. Agents can run a test target through a stable `test` capability without
  knowing the target's native runner.
- R3. A test run returns a machine-readable record containing target id, rule
  kind, capability, status, cache outcome, declared output groups, output paths,
  and a pointer to normalized test results.
- R4. Agents can observe in-flight test execution through generic Once status,
  logs, diagnostics, and action metadata without native runner-specific parsing.
- R5. Agents can query normalized test results after execution without parsing
  native runner stdout or stderr.
- R6. Normalized test results include test-case-level entries with suite or file
  context, test case name, status, duration when available, and failure details
  when a case fails.

**Rule-Owned Runner Bridges**

- R7. Runner-specific execution and output normalization live in Starlark rule
  definitions or rule-owned metadata, not in Rust command logic.
- R8. The Rust execution path remains generic: resolve graph target, validate
  capability, execute declared actions through the cache substrate, surface
  structured records and diagnostics.
- R9. Rules can declare test-related output groups, including at minimum
  `test_results`; coverage remains optional per rule.
- R10. The test capability contract supports ecosystem-specific runner bridges
  for Ruby, Python, Node, Android, Apple, shell-based tests, and third-party
  toolchains without adding ecosystem-specific Rust branches.
- R11. Ecosystem-specific data can be attached as structured runner metadata,
  but the common result envelope remains stable enough for agents to compare,
  summarize, retry, and query tests across ecosystems.

**ShellSpec Dogfood Slice**

- R12. A near-real ShellSpec fixture that mirrors Once's current e2e setup can be
  represented as graph test targets and run from the existing e2e suite.
- R13. ShellSpec test targets can model dependencies on the pieces they exercise
  so Once can select and re-run the relevant tests for changed inputs.
- R14. Running the ShellSpec targets through Once produces normalized test
  results while the existing direct ShellSpec suite remains the release
  confidence path.
- R15. After the ShellSpec bridge ships in a released Once version, the repo can
  migrate its e2e suite to use Once-modeled ShellSpec targets as the primary
  path.
- R16. The ShellSpec fixture preserves the important production-like mechanics
  of the current e2e suite, including multiple spec files, shared helpers,
  per-test workspace setup, and invocation of the Once binary under test.

**Affected-Test Selection**

- R17. Once exposes an agent-readable query for likely affected tests from a
  change set or graph diff.
- R18. Affected-test selection is based on graph relationships and declared
  inputs rather than hardcoded knowledge of ShellSpec files or native runners.

## Success Criteria

- A coding agent can discover ShellSpec fixture test targets, run the relevant
  tests, and inspect structured pass/fail results without reading
  ShellSpec-specific docs or parsing ShellSpec output.
- A failed ShellSpec example can be identified from normalized results at test
  case level, including enough failure context for an agent to explain or retry
  the failure.
- Adding another native runner bridge, such as Swift Testing, does not require
  adding runner-specific branches to Rust command orchestration.
- The same agent-facing result and query concepts can describe tests from at
  least shell, Apple, Ruby, Python, Node, and Android ecosystems, even when each
  runner has different native output and sharding models.
- A changed CLI or frontend component can lead to a smaller relevant ShellSpec
  subset instead of always requiring the full suite.
- Existing direct `mise exec -- shellspec` usage remains the authoritative e2e
  check until the Once-modeled path is available in a released Once version.
- The fixture is close enough to the real e2e suite that the later migration is
  mostly mechanical rather than a second design exercise.

## Scope Boundaries

- This does not require replacing all existing ShellSpec invocation in one step.
- This does not replace Once's own direct e2e ShellSpec path before the feature
  is available in a released Once binary.
- This does not make Rust understand ShellSpec, Swift Testing, XCTest, or any
  runner-specific result format.
- This does not make Rust understand Ruby, Python, Node, Android, Apple, or any
  other ecosystem's testing conventions.
- This does not require implementing every ecosystem bridge in the first slice.
- This does not require a fully general remote test UI before the ShellSpec
  dogfood path works.
- This does not require coverage support for every runner in the first slice.

## Key Decisions

- ShellSpec fixture-first dogfood: Starts with a fixture that mirrors Once's e2e
  setup because it is script-shaped, keeps the first release path safe, and
  exercises the agent-facing run and query loop directly.
- Staged adoption: Keep direct ShellSpec as the release-confidence path until a
  released Once can run the modeled ShellSpec tests, then migrate the repo e2e
  suite to Once itself.
- Rule-owned bridges: Keeps the build-system knowledge in Starlark, preserving
  the Rust foundation as a generic execution and observation substrate.
- Normalized observations over stdout scraping: Gives agents a stable interface
  and lets native runner output remain a debugging artifact instead of the API.
- Ecosystem-neutral foundation: ShellSpec validates the contract first, but the
  shared envelope must be general enough for runners with different concepts of
  files, suites, cases, devices, simulators, shards, retries, and coverage.

## Dependencies / Assumptions

- The existing `test` capability, `test_results` output group, and RFC mention
  of affected test queries are compatible with this direction.
- The fixture should mirror the current ShellSpec setup closely enough to expose
  real integration issues before the repo switches its own e2e path to Once.

## Outstanding Questions

### Deferred to Planning

- [Affects R3, R5, R6][Technical] What exact JSON shape should normalized
  test-case-level results use so ShellSpec works now and native runner bridges
  fit later?
- [Affects R6, R10, R11][Technical] Which fields belong in the common result
  envelope, and which belong in ecosystem-specific runner metadata?
- [Affects R12, R13][Technical] What target granularity should ShellSpec use:
  one target per spec file, per describe block, or per logical subsystem?
- [Affects R17, R18][Technical] What input should the affected-test query accept
  first: git diff, changed target ids, changed paths, or an edit transaction
  result?

## Next Steps

→ /ce:plan for structured implementation planning
