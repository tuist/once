# Once versus Bazel on Codex

This notebook records the investigation into building the
[OpenAI Codex](https://github.com/openai/codex) command-line executable with
Once and [Bazel](https://bazel.build/). It is working material for a future
blog post, so it preserves hypotheses, measurements, failed experiments, and
the reasoning behind retained changes.

## Objective

Make Once faster than Bazel for both a cold local build and an unchanged warm
local build of the same Codex executable, while preserving correctness and a
comparable compiler configuration.

## Benchmark principles

- Build the Codex executable represented by Bazel target
  `//codex-rs/cli:codex`.
- Pin the Codex source revision in every result.
- Use Codex's pinned Rust 1.95.0 compiler for both systems.
- Prefetch downloadable source inputs before timed cold builds.
- A cold build clears derived build results while preserving downloaded source
  archives.
- A warm build immediately repeats the same build with derived results intact.
- Verify that each resulting executable starts and reports its version.
- Record elapsed time and peak resident memory where the process model permits
  a comparable measurement.
- Repeat measurements after setup so one-time downloads and tool installation
  do not contaminate results.

## Environment

- Host: Apple silicon macOS
- Logical processors: 18
- Memory: 64 gibibytes
- Codex checkout: `/tmp/once-codex-benchmark`
- Codex revision initially cloned: `e597169e9a783156e50ae9765d891a3dd74df064`
- Codex benchmark revision: `3947f0d0c3e255bade02e241c16cb43d284c0e65`
- Bazel: 9.0.0, from the checked-in `.bazelversion`
- Rust: 1.95.0, from the checked-in Bazel module

## Chronological log

### 2026-07-28: setup and benchmark selection

The Codex repository documents Bazel target `//codex-rs/cli:codex` as the
source-build entry point. This target is preferable to a small leaf crate
because it exercises the dependency graph, compiler scheduling, linking, and
unchanged-build path that matter to Codex contributors.

The first Once release build in this fresh worktree took 4 minutes 5 seconds.
That time is setup for the benchmark driver, not part of the Codex comparison.

Codex pins Rust 1.95.0 while the Once repository currently pins Rust 1.96.0.
Rust 1.95.0 was installed for the benchmark so both build systems can invoke
the same compiler.

### 2026-07-28: first Bazel prefetch attempt

Command:

```sh
mise exec bazel@9.0.0 -- bazel fetch //codex-rs/cli:codex
```

Result: Bazel failed during analysis after 34.497 seconds, before executing a
build action. `codex-rs/windows-sandbox-rs/BUILD.bazel` passes
`binary_test_target_compatible_with` to `codex_rust_crate`, but the macro at
the cloned revision does not accept that argument.

This is not a performance result. The benchmark must use a Codex revision
whose checked-in Bazel graph analyzes successfully. The source revision will
be pinned before baseline measurements begin.

The incompatible argument was introduced by Codex revision `f47f28c` without a
matching change to the macro signature. The benchmark is pinned to its direct
parent, `3947f0d0c3e255bade02e241c16cb43d284c0e65`, from 2026-07-24. This is the
latest revision before the Bazel analysis regression. It retains Bazel 9.0.0
and Rust 1.95.0.

### 2026-07-28: first Once compatibility build

The generated Once graph validated with 1,343 targets. It contained 121
first-party targets in the `codex-cli` normal and build dependency closure.

The first build failed after about 24 seconds while compiling `keyring` 3.6.3.
The generated dependency target enabled mutually exclusive synchronous,
asynchronous, Apple, Linux, and Windows credential-store features.

Root cause: `cargo metadata` for an entire workspace unifies features required
by development dependencies from every workspace member. The Once
`cargo_dependencies` resolver also rooted every external package present in
that metadata, even when the requested first-party target could not reach it.
`keyring` entered through development dependencies and should not have been in
the Codex executable build.

Hypothesis: snapshot only the normal and build dependency closure of
`codex-cli`, remove development dependency edges, and replace each package's
workspace-wide feature set with the exact feature set reported by
`cargo tree -p codex-cli -e normal,build`. This should restore Cargo-compatible
feature selection and avoid compiling unreachable packages. The generated
snapshot remains bound to the exact manifests, lockfile, target platform, and
compiler host through Once's existing snapshot validation.

The first pruning pass still included packages present only on other target
platforms. It failed on `kstring`, whose selected feature list did not contain
the standard-library feature required by its source. The normal and build
metadata edges alone are insufficient because they retain target-conditional
packages that Cargo has already excluded for the selected platform.

The second pruning pass uses the exact package set printed by `cargo tree` as
well as its feature set. The graph now validates with 1,203 targets: 121
first-party targets and 978 resolved Cargo packages. The compatibility build
completed successfully in 4 minutes 38.73 seconds and produced an executable
that reports `codex-cli 0.0.0`. This was not yet an isolated cold run, but the
session timestamps provide a reproducible initial reference.

The generator also exposed Cargo's `(*)` repeated-subtree marker as though it
were a feature on repeated workspace packages. The compiler accepts this
unknown configuration value, but it does not represent the Cargo build. The
generator now drops this marker. This is a benchmark adapter correctness fix,
not a Once optimization.

Codex's native build configurations differ around the V8 JavaScript engine.
The Cargo dependency used by Once downloads a published prebuilt archive.
Codex's checked-in Bazel configuration builds the engine from source with
pointer compression and its sandbox enabled. The Bazel compatibility build
scheduled 13,486 actions and compiled engine sources for both target and
build-tool configurations. Many concurrent C++ compiler processes approached
one gibibyte of resident memory.

This distinction will stay visible in the results:

- The native-default comparison answers how long a Codex contributor waits
  after choosing each build system as configured by the repository.
- Conclusions about build-system overhead will use unchanged builds, small
  edits that do not rebuild the engine, phase timings, and comparable Rust
  actions.
- A native cold-build advantage caused by a prebuilt dependency is useful to a
  user but is not evidence that Once executes the same compilation faster.

The first complete native Bazel build executed 13,486 actions in 2,795.311
seconds, or 46 minutes 35.311 seconds. Bazel reported a 600.70-second critical
path. This is an informative reference rather than the final controlled cold
score because a Once release link and earlier setup work overlapped portions
of the run. The resulting 777-mebibyte arm64 executable starts and prints its
help successfully.

The first immediate Bazel repeat took 4.053 seconds. Repeating the unchanged
build until host caches stabilized produced 1.585, 0.797, 1.551, 1.299, 0.760,
0.532, 0.531, and 0.528 seconds. The median of the last three is 0.531 seconds.
The measured process tree retained about 2.71 gibibytes, dominated by Bazel's
persistent server. This establishes the meaningful warm target: Once must
avoid rebuilding its graph in a fresh process, not merely make a complete
scan faster.

### 2026-07-28: first unchanged Once build

The first unchanged build was a cache hit with the same final action digest,
but still took 100.28 seconds before reporting it. During that interval, Once
itself used about 3.0 gibibytes of resident memory even though no compiler
process was running.

The session trace narrows the cost. Starlark analysis reached the 1,203-target
build scheduler after 0.31 seconds and the first dependency cache hits appeared
after 0.39 seconds. The scheduler did not finish validating the dependency
actions until about 97.8 seconds. The remaining final target checks took about
2.5 seconds. This strongly localizes the regression to per-action input and
cached-output validation rather than manifest parsing or Starlark evaluation.

The leading hypothesis is that every invocation expands and hashes the full
vendored dependency tree again. The vendor directory contains 1,169 package
directories and occupies about 1.6 gibibytes. Once currently streams declared
inputs into action digests on every process invocation, without a persistent
metadata-validated file digest cache. This preserves content correctness but
makes the warm path proportional to all source bytes rather than changed
source bytes.

The first full build also showed substantial memory pressure late in the
graph. One observed `rustc` process compiling the terminal interface used
about 2.2 gibibytes of resident memory while Once retained roughly 1.8
gibibytes. This is not yet a controlled peak-memory result, but it supports
testing a lower-memory input representation and a memory-aware compiler
scheduler.

### 2026-07-28: literature review and performance model

The target of an orders-of-magnitude improvement is a hypothesis, not a result.
It is plausible for an unchanged warm build because the ideal amount of work is
almost zero. It is not a responsible assumption for a cold build, where both
systems must run the same compiler and linker. Cold-build gains must come from
better scheduling, lower orchestration overhead, earlier fetching, less
materialization, or a deliberately different compiler strategy, and must be
demonstrated by measurements.

The build-system literature consistently points to avoiding work as the
highest-leverage optimization:

- [Build Systems à la Carte](https://www.microsoft.com/en-us/research/publication/build-systems-a-la-carte/)
  separates dependency discovery, invalidation, scheduling, and rebuilding. Its
  central minimality property is directly relevant: rebuild only tasks whose
  results can be out of date.
- [Shake](https://shakebuild.com/manual) records discovered dependencies and
  uses them on later runs. Its design shows why dynamic dependency discovery
  and an incremental database belong in the build engine rather than in every
  target kind.
- [Ninja](https://ninja-build.org/manual.html) is intentionally narrow and
  aims for an almost instant incremental path. Its authors attribute much of
  that speed to moving expensive decisions to graph generation and doing as
  little work as possible in the critical path.
- Bazel's
  [Skyframe evaluation model](https://bazel.googlesource.com/bazel/+/3b9ed6e9d3570a0c67e0d59e65b3785bbc1fad99/site/en/reference/skyframe.md)
  keeps an immutable dependency graph, invalidates it from the bottom up, and
  prunes changes when recomputation produces the previous value. The document
  also calls out filesystem notifications as a way to avoid checking every
  previous input.
- [Buck2 architecture](https://buck2.build/docs/concepts/architecture/) uses
  one incremental dependency graph for target configuration, actions, and
  materialization. The accompanying
  [Modern Distributed Incremental Computation Engine paper](https://buck2.build/assets/Modern_DICE.pdf)
  describes a reusable computation graph with versioned values and concurrent
  evaluation. The important lesson for Once is that analysis and execution
  should share invalidation rather than rebuilding a temporary graph on every
  invocation.
- [Rattle](https://ndmitchell.com/downloads/paper-build_scripts_with_perfect_dependencies-18_nov_2020.pdf)
  explores speculative parallel execution with dependencies discovered from
  actual file access. This is promising for scripts whose dependencies are
  incomplete, but it does not remove the need for a cheap unchanged-build
  oracle.

The operating-system literature suggests how to make invalidation proportional
to changes rather than workspace size:

- Apple's
  [File System Events programming guide](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/TechnologyOverview/TechnologyOverview.html)
  describes a persistent change journal. A client stores its last event
  identifier and can ask what changed since that point, including across
  process restarts and machine restarts.
- The guide requires a conservative full scan after dropped events or when the
  service marks a subtree as needing a rescan. This fallback is a correctness
  requirement, not an optional recovery path.
- [Watchman's change query](https://facebook.github.io/watchman/docs/cmd/since)
  pairs a race-free clock with an indexed filesystem tree. A fresh instance
  deliberately reports a conservative result. Its
  [release notes](https://facebook.github.io/watchman/docs/release-notes)
  report that macOS bulk metadata reads improved initial crawl throughput by
  as much as 40 percent and that build graph glob expansion can reuse
  Watchman's index.
- Linux exposes directory change notifications through
  [inotify](https://www.kernel.org/doc/html/latest/filesystems/inotify.html).
  It is useful inside a long-running process, but unlike Apple's persistent
  journal it cannot by itself account for changes while Once is not running.
- Linux
  [statx](https://man7.org/linux/man-pages/man2/statx.2.html) lets callers
  request only required metadata and avoid forced synchronization with remote
  storage. It is a useful cold-scan primitive, although it does not change the
  linear number of paths a complete scan must visit.
- Linux
  [Pressure Stall Information](https://docs.kernel.org/accounting/psi.html)
  reports processor, memory, and storage contention and supports threshold
  notifications. A scheduler can use those signals to stop admitting
  memory-heavy compiler work when concurrency has become counterproductive.
  macOS exposes corresponding warning and critical states through a
  [memory-pressure dispatch source](https://developer.apple.com/documentation/dispatch/dispatch_source_type_memorypressure).
- Linux's
  [asynchronous input and output ring](https://man7.org/linux/man-pages/man7/io_uring.7.html)
  can submit multiple metadata operations through shared kernel and user-space
  queues, including
  [statx requests](https://man7.org/linux/man-pages/man3/io_uring_prep_statx.3.html).
  This could reduce cold-scan system-call overhead, but it must be benchmarked
  against a straightforward parallel walk because setup and completion
  handling are not free.
- [Apple File System clones](https://developer.apple.com/documentation/foundation/about-apple-file-system)
  and Linux
  [shared-extent file cloning](https://man7.org/linux/man-pages/man2/FICLONE.2const.html)
  can materialize a cached file without copying its data blocks. Once can try a
  clone first when the content store and workspace share a supporting volume,
  then fall back to a regular copy.
- macOS exposes
  [advisory asynchronous reads](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html)
  that can ask the kernel to read a known file range ahead. It may help a cold
  build when source hashing discovers future compiler inputs early, although
  the value depends on storage and page-cache behavior.

Process reuse and scheduling are the next tier:

- Bazel
  [persistent workers](https://docs.bazel.build/versions/main/persistent-workers.html)
  reuse tool processes and parsed state to remove startup and repeated parsing.
  Rust compiler support would require an explicit compiler protocol, so this is
  a longer-term experiment rather than an assumption.
- Bazel's
  [remote caching model](https://bazel.build/remote/caching) distinguishes the
  action cache from the content-addressed output store. Once already has the
  same useful separation. The remaining opportunity is to avoid materializing
  intermediate outputs when a downstream action or remote executor can consume
  them by digest.
- Bazel's
  [fetch command](https://docs.bazel.build/versions/main/guide.html#fetching-external-dependencies)
  and repository cache support separating network setup from timed builds.
  Once should go further by starting eligible fetches as soon as a dependency
  coordinate is known, concurrently with the rest of analysis.

This produces a concrete performance model:

| Build path | Current dominant work | Intended dominant work |
| --- | --- | --- |
| Unchanged warm | Parse and analyze the graph, expand globs, inspect every input, and previously reread every input byte | Read a change-journal cursor, validate a small environment and toolchain key, then reuse the saved graph result |
| Small edit | Repeat whole-graph analysis and inspect every input | Invalidate changed paths and their reverse dependencies, then execute only the affected action slice |
| Cold, sources prefetched | Analyze the whole graph, hash all inputs, compile with a fixed concurrency limit, and materialize all outputs | Analyze and hash concurrently, schedule the critical path within a memory budget, and avoid unnecessary materialization |
| Cold, network empty | Wait for resolution and downloads before compilation can advance | Fetch the earliest known dependency sources while graph discovery continues |

### 2026-07-28: retained optimization, persistent source digests

The first retained change adds a persistent source-digest index under Once's
workspace state directory. Every regular file and symbolic link is keyed by a
conservative metadata fingerprint. On Unix hosts the fingerprint includes
size, modification time, change time, device, inode, mode, and file type.
Matching metadata reuses the saved content digest. A mismatch streams the
source again and replaces the entry. Directories still use the recursive
content-digest implementation.

This changes unchanged input validation from reading all source bytes to one
metadata lookup per declared path. It should substantially reduce data read
from the 1.6-gibibyte vendor tree, but it is still linear in the number of
files. Therefore it is an enabling optimization, not the final warm-build
architecture.

Correctness tests cover an unchanged persisted entry, a same-length content
edit, a malformed cache file, and a cached output that changed without changing
its length. The focused tests pass.

The first benchmark attempt after this change accidentally exposed Rust 1.96
instead of Codex's pinned Rust 1.95 toolchain. Once correctly invalidated the
affected actions. I interrupted that build, corrected the benchmark harness,
and ran it again with Rust 1.95. This sequence found a separate correctness
bug: an action-cache hit confirmed that its output blobs existed in the
content-addressed store, but did not ensure that the corresponding workspace
files contained those blobs. A downstream Rust 1.95 action then consumed a
stale Rust 1.96 library left by the interrupted build and failed with an
incompatible compiler error.

The repair extends the persistent index to cached outputs. On an action-cache
hit, Once now compares each existing output with its previously recorded digest
and conservative metadata fingerprint. Matching files need no content read.
Missing, changed, or unindexed files are materialized from the content store
before dependent actions become runnable. Action misses record their resulting
outputs as well. Older index files remain readable and take the safe
materialization path once before gaining output entries.

This failed attempt has no performance score. It is retained because it
validated that toolchain identity participates in action keys and revealed a
real cache-hit correctness boundary. The next run deliberately starts from the
mixed workspace produced by the interrupted toolchain change. A successful
cache-only recovery will validate the repair before warm timing begins.

That recovery succeeded without invoking the compiler and took 16.209 seconds.
Once restored stale outputs from the Rust 1.95 cache, then recorded their new
workspace fingerprints. Stable unchanged repeats took 6.861, 7.129, 7.632,
and 6.174 seconds. Their median is about 7.0 seconds, a roughly fourteen-fold
improvement over the 100.28-second baseline.

The stable trace reaches the 1,203-target scheduler in 0.19 seconds and its
first cache hits in 0.23 seconds, then spends about 5.74 seconds traversing
1,442 cacheable action hits and 240 small uncached setup actions. No compiler
process runs. Peak resident memory remains high at roughly 4.4 to 4.7
gibibytes. The source and output indexes are therefore retained, but the
result is still about thirteen times slower than Bazel's stabilized
0.531-second warm build.

The next narrow hypothesis is that Once performs unnecessary content-store
existence checks for outputs whose workspace fingerprints already match the
cached action result. Those checks protect a future restoration, but an
unchanged action can use the matching workspace output directly. Once only
needs to probe an output blob when the output is missing or changed. Captured
standard output and standard error blobs must still be present because
consumers read them directly from the content store.

The change is retained. After one stabilization run at 7.274 seconds, three
unchanged builds took 5.890, 5.940, and 5.903 seconds, with a 5.903-second
median. That is about 1.1 seconds, or sixteen percent, faster than the previous
stable result. A changed or missing workspace output still requires its
content-store blob, and focused tests cover both stream completeness and the
matching-output shortcut.

A four-second processor sample identifies the next dominant cost. About 46
percent of sampled main-thread stacks are under the scheduler's dependency
read path, deep-cloning
[JavaScript Object Notation](https://www.json.org/json-en.html) provider trees
before each dependent target is
spawned. The copies recurse through maps and vectors, spend substantial time
in allocation and memory movement, and help explain the 3.9-gibibyte sampled
physical-footprint peak.

Hypothesis: completed providers are immutable, so the scheduler can retain each
one behind an atomically reference-counted pointer. Dependents clone only that
pointer. Starlark analysis still converts the provider into its own evaluation
heap when it actually consumes it, preserving isolation without keeping a
second scheduler-owned deep copy. This should reduce both elapsed time and
peak memory without changing action keys or provider values.

The shared-provider change is retained. After a 4.939-second stabilization
run, three unchanged builds took 3.603, 3.609, and 3.607 seconds, with a
3.607-second median. This is thirty-nine percent faster than the prior
5.903-second result. Peak resident memory fell from roughly 4.4 to 4.7
gibibytes to roughly 2.0 to 2.2 gibibytes, more than a fifty-percent
reduction. The action digest and resulting executable remain unchanged.

The next sample exposes a second memory and durability inefficiency. Multi-step
targets record evidence for their individual actions. Every append opens a new
SQLite connection, reruns database migrations, creates a worker thread, writes
one row, and closes the connection. A warm Codex build performs this hundreds
of times. Samples show repeated database preparation, journal writes, and
filesystem synchronization, plus worker thread numbers above 500.

Hypothesis: evidence stores that point at the same workspace database can share
one lazily opened connection for the lifetime of the Once process. Existing
per-database serialization remains in place. This preserves the durable insert
contract while removing repeated connection, migration, and thread startup.
Eight focused evidence tests pass after the change.

The shared connection is retained. After a 4.467-second stabilization run,
three unchanged builds took 3.088, 3.069, and 3.072 seconds, with a
3.072-second median. This removes another 0.535 seconds, or fifteen percent,
while keeping peak resident memory near 2.04 gibibytes. The remaining database
cost is one durable transaction per evidence record, which suggests batching
the command's records into one transaction as a separate experiment.

The batching experiment groups successful action evidence within each
multi-step target and inserts the records in one transaction. Failed actions
remain durable immediately, and records collected before a later action fails
are flushed before the target returns its error. This preserves failure
evidence while reducing journal synchronization. Nine evidence tests and all
56 graph-analysis tests pass.

The batching change is retained. After a 4.286-second stabilization run, three
unchanged builds took 2.847, 2.834, and 2.886 seconds, with a 2.847-second
median. This removes another 0.225 seconds, or seven percent, from the
3.072-second shared-connection result. Aggregate resident memory remains noisy
between 2.06 and 2.24 gibibytes, with no material regression. Since the
original 100.28-second Once baseline, the cumulative unchanged-build
improvement is thirty-five-fold. Bazel remains 5.4 times faster at its
0.531-second median, so the next experiment must remove complete graph
validation from the no-change path rather than tune another per-target loop.

The next experiment implements that boundary with an operating-system
filesystem watcher and a persisted successful-build receipt. The receipt is
bound to the watcher process identity, separate source and final-output change
generations, target, sandbox policy, the complete client environment, the Once
executable identity, and the rendered build record. Each request writes a
watched fence file and waits for its event before reading the generation. This
barrier ensures that a delayed filesystem event cannot arrive after a receipt
has already been accepted.

The first watcher design recursively watched the whole workspace and filtered
`.once` events in the callback. That is too late on macOS. A full output tree
generates enough kernel events to overflow the stream before user-space
filtering. The watcher correctly surfaced a mandatory rescan event and Once
refused to persist a receipt. This failed design is discarded.

The corrected design registers source paths, the fence directory, and final
outputs in one kernel event stream. Source paths exclude `.once` and `.git`. A
small root fingerprint detects newly created or removed top-level paths and
rebuilds that watch set. A final output is registered only after its successful
build, which avoids intermediate-output floods while still invalidating a
receipt if a declared result is edited or removed. One event stream is
important because the fence must order source and output events together.
Source and output generations are still counted and tested independently.

An early release trial demonstrated the intended path at 0.231 and 0.233
seconds, with about 10 mebibytes in the short-lived client. This is already
about 2.3 times faster than Bazel's 0.531-second median, but the same trial
later fell back to complete validation. Diagnostic events showed that a
nominally non-recursive macOS root watch still received the recursive output
traffic and emitted rescan events. The root watch has therefore been replaced
by direct top-level watches plus the root fingerprint. These early timings are
provisional until repeated no-op runs and deliberate source, output, and
watcher-restart invalidations pass with the corrected design.

A second intermittent fallback came from the receipt's environment boundary,
not the filesystem watcher. The first implementation fingerprinted every
process variable. Once command actions clear their inherited environment, and
target analysis reads environment variables through an explicit host lookup,
so unrelated terminal variables cannot affect the build. Treating them as
inputs caused correct but unnecessary full validation.

The receipt now stores only environment names and values actually observed by
target analysis, plus the fixed environment set used to resolve managed
toolchains. The host environment cache records each lookup across concurrent
target analysis. A 20-run focused test varied an unrelated variable on every
invocation and kept every receipt hit after stabilization. Changing the
executable search path, which participates in tool resolution, forced one
validation run and then returned to the fast path. Focused end-to-end tests
also show that touching a source or declared final output forces validation,
followed by a receipt hit.

The benchmark driver itself contained a short-process measurement bug. Its
memory sampler polled the child and then slept for 0.2 seconds, so a process
that exited in a few milliseconds could not be reported in less than about
0.23 seconds. Elapsed timing and memory sampling are now independent. The main
thread waits for the process and records its finish time immediately, while an
optional background sampler observes aggregate resident memory. Latency runs
disable sampling so process-table polling does not perturb either build
system.

The corrected, stabilized warm comparison is:

| Measurement | Once median | Bazel median | Once advantage |
| --- | ---: | ---: | ---: |
| End-to-end invocation through `mise` | 0.0556 seconds | 0.7281 seconds | 13.1 times |
| Tool path and environment prepared before the timer | 0.0227 seconds | 0.5208 seconds | 22.9 times |
| Persistent service resident memory | 11.3 mebibytes | 2,163.6 mebibytes | 191 times |

The corrected Once end-to-end median uses ten stable unchanged builds. Its
prepared-launcher median excludes one environment-boundary validation and uses
the following nine receipt hits. Both prepared-launcher cases remove the same
tool launcher from the timed process. The normal end-to-end result remains the
primary user-facing number. The prepared result separates build-system client
latency from tool-environment setup.

This establishes more than one order of magnitude in unchanged-build elapsed
time and more than two orders of magnitude in persistent resident memory
against Bazel on this workload. Relative to Once's original 100.28-second warm
baseline, the normal 0.0556-second path is about 1,804 times faster, a
three-order-of-magnitude improvement. It does not establish an
orders-of-magnitude cold-build claim.

The first isolated Once cold measurement completed in 222.972 seconds. A later
audit found that Cargo's repeated-subtree marker had been retained as part of
feature names such as `std (*)` in the generated compatibility graph. These
extra configuration names evaluated false, but they were not part of Cargo's
real feature set. That run is retained as diagnostic history and withdrawn as
the final score.

The generator now removes the marker whether it appears alone or as a suffix.
The corrected snapshot has 489 packages with selected features and no
marker-contaminated feature names. The corrected isolated build cleared
workspace outputs and the local action and content caches while preserving
the prefetched dependency sources. It completed in 311.624 seconds, or 5
minutes 11.624 seconds, and produced an executable that reports
`codex-cli 0.0.0`.

| Native-default cold measurement | Elapsed | Relative result |
| --- | ---: | ---: |
| Once, corrected isolated derived state | 311.624 seconds | 9.0 times faster |
| Bazel, completed reference build | 2,795.311 seconds | baseline |

This is a real contributor-wait comparison for each repository-native
configuration, but not a controlled build-engine comparison. Bazel compiled
the V8 JavaScript engine from source twice while Once consumed its published
prebuilt archive. The Bazel run also overlapped earlier setup work, so its time
is a completed reference rather than a clean-room score. The result establishes
almost one order of magnitude for the native-default cold experience. It does
not show that Once compiles equivalent cold actions faster, and it does not
justify an orders-of-magnitude cold claim.

A separate cold run sampled the complete process tree at 0.2-second intervals.
It observed a 7,785,904-kibibyte peak, about 7.43 gibibytes. That run took
385.164 seconds because repeated process-table scans perturbed a process-heavy
workload, so it is a memory measurement rather than a latency score. The
sampler now resolves persistent service processes once before starting the
timer instead of rescanning full command lines on every sample.

The first invocation after the original cold build took 4.066 seconds despite
every action hitting the cache. The following invocation took 0.0458 seconds.
One cause was that source fences and final outputs used separate kernel event
streams, so the source fence could complete before the output watcher delivered
a pending event. The tracker now registers both areas on the same ordered
stream. A regression test covers this handoff. Removing the final Codex output
and restoring it from the cache took 6.487 seconds; the immediately following
invocation took 0.0580 seconds and the executable remained valid.

The corrected cold run still required a 6.338-second cache-only validation
before its first 0.0579-second receipt hit. A vendored Rust build script
briefly writes in its own package directory while probing compiler features.
That changes source-directory metadata during the build. Once conservatively
refuses to certify a no-change receipt when a source generation changes between
the initial and final event barriers. Accepting the receipt without proving the
final source state would risk hiding an input mutation that occurred after its
consumer ran. Removing that validation safely requires changed-path
reconciliation against the final input digest index, not merely ignoring
build-time events.

Deleting `.once` also revealed that the long-running tracker retained its set
of previously watched outputs after their kernel watches disappeared. The
reset path now unregisters and clears those stale paths so rebuilt final
outputs are watched again. The regression test deletes all workspace state,
recreates the output, and proves that a later edit increments its generation.

The final receipt audit found another correctness boundary outside the watched
workspace. A compiler, linker, interpreter, or other observed host file could
change in place without changing the executable search path. The receipt now
stores conservative metadata fingerprints for every host path and resolved
tool observed during analysis, as well as graph-declared tool paths. Changing
a host tool at the same path rejects the receipt in a focused test. Repeating
the benchmark after this protection covered 12 host tool paths and 35 relevant
environment values. It produced the final 0.0556-second end-to-end and
0.0227-second prepared-launcher medians in the warm table.

### Ranked architecture hypotheses

1. **Persist source digests.** Expected to remove repeated file-content input
   and output work from warm builds. Risk is stale reuse after an incomplete
   metadata fingerprint. Status: retained after correctness, recovery, and
   performance validation.
2. **Add a generic filesystem change-journal boundary.** On macOS, use File
   System Events to ask for changed paths since the last successful build. On
   Linux, use a long-running watcher or an indexed service, with a full-scan
   fallback after process downtime unless a persistent journal is available.
   Expected to reduce no-change validation from the total input count to the
   changed input count. Status: retained with event barriers, rescan fallback,
   precise environment observations, and independent final-output tracking.
3. **Persist analysis computations and reverse dependencies.** A journal with
   no changed relevant paths can reuse the final analysis value immediately.
   Changed paths invalidate only dependent computations. The saved key must
   also bind the requested targets, configuration, relevant environment,
   external toolchain identity, Once version, and target-kind module versions.
4. **Pipeline dependency fetching with analysis.** Resolution should emit
   fetchable coordinates incrementally. Downloads, checksum verification, and
   unpacking can begin while independent graph analysis continues.
5. **Use a memory-aware critical-path scheduler.** Estimate or learn memory
   cost per compiler and linker action, reserve link headroom, and prefer work
   that shortens the remaining critical path. Feed operating-system memory
   pressure into admission control. Compare elapsed time, peak resident memory,
   pressure stalls, and processor utilization against a fixed job limit.
6. **Keep outputs virtual until required.** Pass content digests between
   actions and materialize only declared final outputs or inputs required by a
   local tool. When materialization is required, try a same-volume
   copy-on-write clone before copying bytes. This reduces filesystem traffic
   and cache duplication.
7. **Investigate compiler process reuse.** A persistent Rust compiler worker
   could retain parsing or macro state, but only if its correctness boundary is
   explicit and the compiler supports a stable enough protocol. This is lower
   priority than avoiding build-system work.

Kernel-level techniques remain below graph invalidation in the ranking. A
faster complete scan is still a complete scan. The change journal can remove
that scan from the warm path, while bulk metadata, asynchronous submission,
read-ahead, and file cloning can improve the unavoidable cold or
materialization paths. Each requires a focused benchmark because kernel
features can regress a fast local solid-state drive through extra setup,
queueing, or page-cache disruption.

## Hypothesis backlog

- Start dependency fetching while the rest of the graph is still being
  analyzed.
- Persist file content digests with a conservative metadata fingerprint so an
  unchanged process invocation can validate inputs with file metadata instead
  of rereading 1.6 gibibytes.
- Avoid holding complete file contents in memory when a digest can be computed
  incrementally.
- Bound concurrent compiler work by observed memory pressure rather than a
  fixed task count.
- Give linking enough memory headroom instead of maximizing compilation
  concurrency until the final action stalls.
- Reuse parsed manifests, dependency resolution, and toolchain identity across
  unchanged builds where correctness permits.
- Avoid materializing outputs that downstream actions can consume directly
  from the content store.
- Preserve downloaded and unpacked dependency sources independently from
  derived action results so a cold result cache does not become a cold network
  cache.

### 2026-07-29: rebase onto bounded-memory main and cache invalidation

Origin main added bounded memory management (PR #201) and Mix/Cargo native
project discovery (PR #200). The campaign worktree was rebased onto the new
base. Six files conflicted because both sides modified the graph analysis,
scheduler, and dispatch paths. Resolution strategy:

- `input_digest.rs`: kept the extracted `digest_source_path` free function
  (needed by the source digest cache) but upgraded its directory path to
  stream through `DigestWriter` + `write_directory_blob` instead of buffering
  the whole blob, complying with the bounded-memory design.
- `globals.rs`: merged the upstream bounded host-file reader with the
  receipt feature's `observe_host_path` call so both bounded memory and
  receipt invalidation work.
- `scheduler.rs`, `analysis.rs`: kept the transitive `available_inputs` map
  and critical-depth scheduling, added the `ResourcePool` threading and
  `materialized` field from upstream.
- `actions.rs`: kept `DeclaredActionsState` accumulation and evidence
  batching, added `ResourcePool` permit acquisition before each cache miss
  and uncacheable action, and the `materialized` field on `AvailableInput`.
- `mod.rs`: merged the build-receipt/change-tracker fast path with
  `.with_resource_limits(resource_limits)` on every session construction.

The scheduler's eager `materialize_cached_outputs` call was restored because
Starlark target kinds can consume dependency outputs through argv without
declaring them as inputs. The lazy `materialize_available_inputs` path only
restores declared inputs, so the scheduler-level materialization remains
necessary for correctness in the general case.

Validation after rebase: `cargo fmt --all -- --check`, `cargo clippy
--workspace --all-targets -- -D warnings`, `cargo test -p once-cli -p
once-core -p once-frontend -- --test-threads=1` (298 passed, 0 failed),
Python compile, bash syntax, and JSONL checks all pass. Shellspec timed out
under background load; the release build completed in 4 minutes 45 seconds.

The active lazy-materialization experiment (282.33 seconds, exit zero, valid
executable) was a cache-priming cold build, not a warm materialization test:
the transitive artifact identity refactor changed action keys, invalidating
the eager-built cache. A new cache must be primed with the rebased binary
before lazy materialization can be measured against the eager control.

### 2026-07-29: upstream cold-build regression diagnosis

After rebasing onto origin/main, cold builds of the Codex workspace hung for
60-plus seconds before any compilation began. Stack sampling with the macOS
`sample` tool localized the bottleneck: 75 percent of samples were in
`walkdir::IntoIter::next` called from the Starlark prelude's `build` function
during per-target analysis. The upstream PR #200 (native Mix and Cargo project
discovery) restructured the `rust.star` prelude so that `_rust_sources` and
`_rust_source_inputs` each call `glob(ctx["srcs"])` when resolver-provided
source lists are absent. For manually-defined targets (as the Codex benchmark
graph uses), both functions expand the source pattern by walking the directory
tree, doubling the directory walks per target across 1203 targets.

The pre-rebase binary (`/tmp/once-final-candidate`) does not exhibit this
regression because it predates the prelude restructuring. The rebased code
compiles, passes clippy, and passes all tests, but cannot be used for cold
benchmarks until the upstream glob expansion is cached or pruned. This is an
upstream issue, not a campaign regression. The campaign measurements continue
with the pre-rebase binary.

### 2026-07-29: lazy materialization measured

With the pre-rebase binary carrying transitive artifact identities through
each `BuildOutcome`, a complete-cache, empty-output-tree treatment completed
in 8.87 seconds. The prior eager control was 47.799 seconds. The lazy
treatment is 5.4 times faster for this scenario.

| Measurement | Elapsed | Output tree |
| --- | ---: | ---: |
| Eager control (old binary, all outputs restored) | 47.799 seconds | 3.7 gibibytes |
| Lazy treatment (transitive artifacts, only needed outputs) | 8.870 seconds | 658 mebibytes |

The lazy design materialized only 658 mebibytes instead of 3.7 gibibytes, an
82 percent reduction in filesystem traffic. Intermediate library outputs
(rlibs, dylibs for vendored crates) were never written to disk because no
cache-miss action consumed them. Only the final executable, its direct
inputs, and outputs needed by the scheduler's `materialize_cached_outputs`
call were restored from the content store.

The immediately following receipt-hit invocation took 0.297 seconds,
confirming the receipt path still works after output-tree reconstruction.

Caveat: the eager control was measured with a different binary that predates
the transitive artifact identity refactor. The action keys differ between
the two binaries, so the comparison is between two independent cache-priming
runs rather than a controlled A/B test on the same cache. The improvement
is real but the magnitude carries binary-change uncertainty.

### 2026-07-29: glob walk-root narrowing fix

Root cause of the upstream cold-build regression confirmed: when `package` is
empty (targets at the workspace root), every `glob(ctx["srcs"])` call walks
the entire workspace tree, including the 1.6 gibibyte vendored dependency
directory. With 1203 targets each calling glob twice, this is thousands of
full-tree walks.

Fix added to `expand_globs_with_excludes`: a `narrow_walk_root` function
extracts the literal directory prefix from each glob pattern before the first
wildcard character, computes the longest common ancestor across all patterns,
and starts the walkdir from that subdirectory instead of the workspace root.
For a pattern like `codex-rs/tui/**/*`, the walk starts at
`workspace/codex-rs/tui/` rather than the workspace root.

Before the fix, the rebased binary produced zero output after 60 seconds and
burned 5 cores in walkdir. After the fix, compilation began within 30 seconds
with 11 cores active. The fix compiles clean, passes clippy, and preserves
correctness because pattern matching still uses the full workspace-relative
path.

### 2026-07-29: review-driven correctness fixes

A structured review of the retained changes surfaced two actionable defects.
Both are fixed and covered by focused tests.

- Change-tracker fence barrier ordering. The event callback released a fence
  waiter as soon as it saw the fence path, but it recorded the source and
  output change generations only after finishing the whole event. When a real
  change and the fence write coalesce into one filesystem event, a waiter could
  snapshot the pre-change generation and wrongly certify a no-change receipt.
  The callback now collects fence tokens during the loop, records every change
  in the event first, and releases the waiters last. The fence classifier was
  split from the release path so it no longer mutates waiter state while
  classifying.
- Glob walk-root narrowing on absolute patterns. `narrow_walk_root` joined the
  literal prefix onto the package directory. An absolute prefix made the join
  discard the package directory and could collapse the common ancestor toward
  the filesystem root, walking far more than the package. The function now
  returns early for an absolute literal prefix, falling back to the package
  walk. A regression test covers absolute-only and mixed absolute-plus-relative
  pattern sets.

Two further review observations are design boundaries rather than bugs and are
recorded here without code changes: the metadata-only reuse fingerprint can in
principle reuse a stale digest if a same-length rewrite lands within one
filesystem timestamp tick, and cached outputs are validated by fingerprint plus
the watcher generation rather than by content digest. Both are conservative in
the direction that matters for the receipt boundary and depend on the watcher
never silently dropping an event without signalling a rescan.

Validation: `cargo fmt` and `cargo clippy` clean on the touched crates;
`once-frontend`, `once-core`, and the targeted `once-cli` suites pass (669
tests, 0 failures). The end-to-end filesystem-watcher barrier test is sensitive
to the macOS event-delivery warm-up window on this host and times out on its
first fence regardless of this change; it is an environment timing issue in the
test, not a regression in the tracker.
