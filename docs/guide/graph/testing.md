# Testing and Scheduling

Once separates which tests should run from where and when they run. That
separation keeps affected-test selection conservative, makes exact requests
auditable, and lets scheduling evolve without changing test identity.

Start here whether you are declaring a test target for the first time or
planning work for an existing target. [Full ecosystem guides](/guide/graph/ecosystems)
cover build and test targets together. Test-runner reference pages provide
focused, copyable declarations for runner-only integrations.

## Start With Your Runner

Keep the native runner as the source of truth for test collection and
execution. Choose the matching target kind, make its prerequisite available,
and let Once record stable test identities around it:

| Runner | Target declaration | Prerequisite | Default automatic batch |
| --- | --- | --- | --- |
| pytest | [`pytest_test`](/reference/prelude/pytest_test) | The selected Python interpreter can import pytest | One test file |
| [Ruby Specification](https://rspec.info/) | [`rspec_test`](/reference/prelude/rspec_test) | The selected Ruby interpreter can load the runner | One specification file |
| Minitest | [`minitest_test`](/reference/prelude/minitest_test) | The selected Ruby interpreter can load Minitest | One test file |
| Vitest | [`vitest_test`](/reference/prelude/vitest_test) | The package-local Vitest entry point is installed | One test file |
| Jest | [`jest_test`](/reference/prelude/jest_test) | The package-local Jest entry point is installed | One test file |

Each target kind ships a runnable starter. Retrieve the one that matches your
runner when you want a complete manifest and source layout:

```sh
# Python with pytest
once query example pytest_test pytest-test-minimal --format json

# Ruby Specification
once query example rspec_test rspec-test-minimal --format json

# Ruby with Minitest
once query example minitest_test minitest-test-minimal --format json

# JavaScript or TypeScript with Vitest
once query example vitest_test vitest-test-minimal --format json

# JavaScript or TypeScript with Jest
once query example jest_test jest-test-minimal --format json
```

After copying or adapting the declaration, use the same first-run workflow for
every runner. The bundled starters name their target `tests`:

```sh
once query schema pytest_test
once test tests --format json
once query test-manifest tests --format json
once test tests --jobs 4 --format json
```

Replace `pytest_test` with the chosen kind when querying its schema. The first
complete run discovers stable units and writes the manifest. The next run can
schedule those units as independent file batches across the requested workers.
Set `batching = "case"` in a pytest, Ruby Specification, Vitest, or Jest target
only when its fixtures and startup costs make case-level batches worthwhile.

## Start With the Safety Model

| Layer | Decides | Stable output |
| --- | --- | --- |
| Selection | Which test targets or explicit units belong in the run | Selection reasons and safety level |
| Plan | How selected work is divided into semantic batches | Plan and batch identifiers |
| Schedule | Which worker takes each batch and attempt | Placement, timing, and status |
| Results | What the runner observed | Normalized suites, cases, attempts, and artifacts |

Affected selection is conservative. Once starts from changed graph inputs,
walks reverse dependencies, and selects complete test targets. Missing
ownership also selects tests instead of silently skipping them.

Exact unit selection is different. It is allowed only when the target kind
declares a lossless translation from a stable Once unit identifier to native
runner arguments. Unsupported or stale unit requests fail during planning.

The boundary is deliberately ecosystem-neutral. Rust validates generic test
information and result records, builds graph-based plans, and schedules opaque
batches. A Starlark target kind owns runner discovery, command arguments,
filter translation, result normalization, and platform requirements. Adding a
test ecosystem should therefore require a target kind, not a branch in the
planner or executor.

## Discover the Test Surface

List test targets before choosing one:

```sh
once query tests --format json
```

Run a complete target once to establish current discovery data:

```sh
once test tests/unit --format json
once query test-manifest tests/unit --format json
```

The manifest reports:

- `source`, which distinguishes normalized discovery from a whole-target
  fallback;
- `listing_supported`, which states whether the target kind can expose stable
  units;
- `case_filtering`, which is `runner_args` only when exact execution is safe;
- `sharding`, which states whether Once may create automatic batches and
  whether their granularity is `file`, `case`, or `target`;
- `discovery_fingerprint`, which ties the unit list to the declared inputs
  that can change discovery; and
- `units`, the stable identifiers observed in the latest complete run.

A filtered run does not replace the complete manifest. Agents can run one case
for quick feedback without forgetting the other cases discovered by the last
whole-target run.

## Plan Affected Tests Before Running

Inspect the immutable plan for changed workspace paths:

```sh
once query test-plan \
  --changed-path src/library.rs \
  --format json
```

Each selected test includes a reason. The current affected policy emits one
complete test scope per selected target. When that target has a current
manifest and supports exact filtering, the plan divides the complete scope
into stable file or case batches. This changes scheduling granularity without
using test impact as permission to omit a discovered case.

The first run, a changed test input, or an incomplete manifest produces one
whole-target batch. That run refreshes discovery. Later plans may then split
the same target automatically. A stale manifest never filters against an old
unit list.

Run the same selection through the dynamic scheduler:

```sh
once test \
  --changed-path src/library.rs \
  --jobs 4 \
  --format json
```

`--jobs` caps concurrent local workers. It does not change selection, plan
identity, or batch identity.

Inspect recent attempts and measured durations afterward:

```sh
once query test-attempts --limit 20 --format json
```

## Run One Exact Unit

Choose an identifier returned by the current complete manifest:

```sh
once query test-plan \
  --target tests/unit \
  --test-unit 'tests/unit::returns_greeting' \
  --format json

once test tests/unit \
  --test-unit 'tests/unit::returns_greeting' \
  --format json
```

Planning verifies both conditions before it creates an exact batch:

1. the target declares `case_filtering = "runner_args"`; and
2. the requested identifier is present in the persisted complete manifest.

Run the whole target again when discovery is missing or stale. Do not construct
runner-specific filters from names that Once did not return.

## Understand Current Ecosystem Coverage

| Target kind | Stable unit discovery | Exact unit execution | Automatic granularity |
| --- | --- | --- | --- |
| [`pytest_test`](/reference/prelude/pytest_test) | Yes | Yes | File by default, optional case |
| [`vitest_test`](/reference/prelude/vitest_test) | Yes | Yes | File by default, optional case |
| [`jest_test`](/reference/prelude/jest_test) | Yes | Yes | File by default, optional case |
| [`rspec_test`](/reference/prelude/rspec_test) for [Ruby Specification](https://rspec.info/) | Yes | Yes | File by default, optional case |
| [`minitest_test`](/reference/prelude/minitest_test) | One unit per file | Yes, by file | File |
| [`rust_test`](/reference/prelude/rust_test) | Yes | Yes | Target |
| [`android_local_test`](/reference/prelude/android_local_test) | Yes | Yes | Target |
| [`android_instrumentation_test`](/reference/prelude/android_instrumentation_test) | Yes, from a completed device run | Yes, through the instrumentation `class` argument | Target |
| [`apple_test_bundle`](/reference/prelude/apple_test_bundle) with Swift Testing | Yes | Not yet | Target |
| `shellspec_test` | Yes | Not yet | Target |
| [`zig_test`](/reference/prelude/zig_test) | Suite-level fallback | Not yet | Target |
| [`elixir_test`](/reference/prelude/elixir_test) | Summary counts only | Not yet | Target |

Unsupported ecosystems still participate in conservative affected selection
and target-level scheduling. Once reports the limitation instead of guessing a
native filter.

## Treat Sharding As Scheduling

The scheduler creates stable batches, orders them using recent uncached
durations, and lets idle workers pull the next batch. This avoids the duration
skew of a fixed partition while keeping the plan independent from the
requested worker count.

Dynamic-language runners default to file batches because files commonly share
imports, fixtures, database setup, and interpreter startup. Every discovered
case from one file stays in the same batch. Historical batch durations let
Once balance those stable files without coupling the plan to four, eight, or
any other fixed number of workers. A target can opt into case granularity when
its runner and setup make smaller batches worthwhile.

Concurrent batches write results, logs, and native output below isolated batch
directories. After all batches finish, Once validates and merges them into one
canonical target result. The plan contains no provider-specific or
worker-specific assignment, so local or future remote placement does not
redefine test identity.

## Add Selection to a Project Module

Project-local Starlark test kinds use the same discovery and planning surfaces
as built-in kinds. Query the live contract instead of copying a provider shape:

```sh
once query module-contract --format json
```

The returned `test_starter` includes the complete `once_test_info` provider,
normalized result paths, action declaration, and filter plumbing.
`test_target_starter` shows the matching manifest table, and
`normalized_test_result_example` is the machine-readable output contract. Its
adapter receives:

- `--once-results <path>`;
- `--once-log <path>`;
- `--once-target <target>`; and
- one repeated `--once-test-unit <identifier>` for each exact filter.

The adapter writes `once.test_results.v1`, uses stable target-qualified case
identifiers, honors every requested filter, and exits unsuccessfully for a
failed or unknown selection. Once rejects incomplete normalized results before
using them for discovery or accepting a scheduled batch. Validate the module
and workspace before the first run:

```sh
once query validate-module modules/testing.star --format json
once query validate-workspace --format json
```

After a complete run, the normal manifest, exact-plan, affected-plan, and
scheduler commands work without a parallel project-specific interface. See the
[modules reference](/reference/modules/) for the full authoring contract.

## Agent Checklist

For a coding agent, the safe loop is short:

1. Query test targets and capabilities.
2. Inspect an affected plan before running change-scoped tests.
3. Run a complete target before relying on its manifest.
4. Request an exact unit only when the manifest declares runner argument
   filtering.
5. Treat prioritization and worker count as ordering, never as permission to
   remove tests.
6. Read normalized results and schedule attempts instead of scraping native
   runner output.

With `--format json`, command failures return an `once.error.v1` record on
standard output and keep diagnostic logs on standard error. Agents can use the
process status for control flow and the structured `error.message` field for
repairs.

The same operations are available through the
[Model Context Protocol](https://modelcontextprotocol.io/) tools. The
[tool reference](/reference/mcp/tools) maps each tool to its matching terminal
command.
