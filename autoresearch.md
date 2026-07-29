# Autoresearch: outperform Bazel on the Codex build

## Objective

Reduce end-to-end cold and unchanged warm build time for the pinned OpenAI
Codex command-line executable. First make Once's unchanged warm build faster
than Bazel's, then optimize cold scheduling, memory, fetching, and
materialization. Retain only changes that improve repeated release-build
measurements without weakening correctness, portability, structured logging,
or graph semantics.

## Metrics

- Primary: median Once unchanged warm build time (`warm_elapsed_seconds`,
  seconds, lower is better)
- Secondary, measured in dedicated runs: aggregate peak resident memory
  (`peak_resident_kibibytes`, kibibytes, lower is better)
- Comparison gates: Bazel unchanged warm build time, Once cold build time, and
  Bazel cold build time for the same pinned Codex revision

## How to Run

`./autoresearch.sh`

The script builds the release executable, stabilizes the Codex action and
source-digest caches, then measures three fresh Once command invocations. The
Codex checkout defaults to `/tmp/once-codex-benchmark` and can be overridden
with `ONCE_CODEX_CHECKOUT`. Isolated benchmark state defaults to
`/tmp/once-codex-autoresearch` and can be overridden with
`ONCE_CODEX_STATE_ROOT`.

## Files in Scope

- `crates/once-cli/src/commands/graph/`: graph analysis and scheduling
- `crates/once-frontend/src/`: manifest, module, and graph loading
- `crates/once-core/src/`: input hashing, execution, and materialization
- workspace manifests: dependencies and release profile experiments
- `benchmarks/codex-build-comparison/`: reproducible benchmark adapters and
  measurement

## Off Limits

- Platform-specific behavior in generic graph or cache interfaces
- Benchmark-only target-kind behavior or Codex-specific branches in Rust
- Reusing a cached result without a conservative invalidation boundary
- Skipping required session logs
- Benchmark-only shortcuts that do not improve normal Once workspaces

## Constraints

- Use `mise exec --` for every Rust command.
- Keep the public command and graph behavior compatible.
- Keep expensive filesystem and analysis work eligible for parallel execution.
- Pin Codex revision `3947f0d0c3e255bade02e241c16cb43d284c0e65`.
- Use Rust 1.95.0 for both build systems.
- Separate downloaded source inputs from derived action results.
- Run focused checks after every measured experiment and the full suite before
  handoff.
- Do not use em dashes in user-facing text.

## What's Been Tried

### Current Codex campaign

- The generated Once graph now follows the exact normal and build dependency
  closure and feature set reported for `codex-cli`. This fixed incompatible
  development and cross-platform feature unification.
- The first unchanged Once result was a cache hit but took 100.28 seconds and
  retained about 3.0 gibibytes, with no compiler process running.
- A persistent metadata-validated source and output digest index reduced
  repeated content reads and repairs missing or stale workspace outputs from
  the content store.
- Immutable provider sharing cut the stable warm path from 5.903 to 3.607
  seconds and reduced Once's resident memory from about 4.4 to 4.7 gibibytes
  to about 2.0 to 2.2 gibibytes.
- One shared evidence database connection and batched target evidence reduced
  the stable warm path to 2.847 seconds.
- A kernel file-event tracker and correctness-bound build receipts reduced the
  final end-to-end median to 0.0556 seconds. Bazel's median is 0.7281 seconds,
  so Once is 13.1 times faster. With both launchers prepared, Once is 22.9
  times faster.
- The Once tracker retained 11.3 mebibytes versus 2,163.6 mebibytes for the
  Bazel service. A sampled Once cold build peaked at about 7.43 gibibytes.
- The corrected isolated Once cold build took 311.624 seconds versus a
  2,795.311-second completed Bazel reference. The native dependency strategies
  differ, so this is a contributor-wait comparison rather than proof that Once
  executes equivalent cold actions faster.
- The research, rejected hypotheses, correctness fixes, and chronological
  measurements are recorded in `codex.md`.

### Earlier cache-hit campaign

- Reusing one compiled Starlark program per invocation was a large improvement.
- Loading only built-in modules required by the workspace reduced the rebased
  local-hit mean from 61.6 milliseconds to about 30 milliseconds.
- A current-thread Tokio asynchronous runtime removed unnecessary kernel worker
  creation while the existing blocking pool retained parallel file and analysis
  work.
- Direct synchronized log writes removed the dedicated logging thread with a
  small stable improvement.
- Stripping the release executable did not improve latency.
- Research on Arachne and scheduler activations supports avoiding short-lived
  kernel threads. Once now creates no asynchronous worker pool for this command.
- Research on mimalloc, snmalloc, and Hoard motivates testing a sharded allocator,
  but their largest gains target concurrent allocation. Mimalloc measured 25.75
  milliseconds in its stable repeat while the system allocator measured 25.13
  milliseconds, so the system allocator remains.
- The remote provider loads the authentication token through a blocking task for
  every request. The first wave can also establish duplicate data-plane channels
  because connection initialization is not single-flight.
- Resolving authentication once, single-flight channel setup, and skipping a
  throwaway single-endpoint probe reduced the stable clean-client remote median
  from 176.89 to 173.55 milliseconds.
- Retaining validated host tool identity outside disposable action state reduced
  the stable clean-client remote median further to 148.71 milliseconds.
- Mirroring remotely sourced action results through atomic, recoverable writes
  instead of forcing each cache file and directory to stable storage reduced the
  stable clean-client remote median to 41.57 milliseconds.
- Local blob encoding now borrows incompressible input bytes instead of copying
  them before the write. Large streamed remote reads reserve bounded capacity
  from their declared size to avoid repeated buffer-growth copies.
- Deferring construction of the remote network client until the first remote
  request reduced the stable local-hit median from 22.02 to 18.37 milliseconds.
- Remotely fetched blob contents stay synchronized before atomic rename, while
  their recoverable directory entry skips a parent-directory durability
  barrier. Two clean-client repeats measured 37.77 and 36.27 milliseconds
  median versus the 39.99-millisecond control.
- Cached file validation now hashes the file-blob header and file contents as a
  stream. Nine warmed samples measured 21.61 megabytes median maximum resident
  memory versus 23.17 megabytes before the change, a 6.7 percent reduction.
- Staged file restoration now streams into the declared output. Maximum
  resident memory stayed flat because allocator pages from the preceding cache
  fetch were reused, but the second full output allocation and copy are gone.
- Workspace include patterns with a literal first component now prune unrelated
  top-level trees. With 5,000 temporary unrelated branches, 40 runs measured
  11.36 milliseconds median with pruning versus 127.61 milliseconds when an
  equivalent wildcard forced full traversal.
- Module source composition uses clone-on-write. It borrows the built-in source
  when no custom files exist and reserves the exact combined capacity when
  appending custom files.
- Sharing immutable analysis metadata through atomic reference counting removed
  only about 75 allocations in the 15-target fixture and did not improve its
  timing, so the existing ownership layout remains.
- Reducing the workspace database synchronization level regressed the local
  median to 19.65 milliseconds. The immediate reverted control returned to
  18.98 milliseconds, so full synchronization remains.

## Primary Research

- [Mimalloc: Free List Sharding in Action](https://www.microsoft.com/en-us/research/publication/mimalloc-free-list-sharding-in-action/)
  uses page-local free-list sharding to improve locality and reduce contention.
- [snmalloc: A Message Passing Allocator](https://www.microsoft.com/en-us/research/publication/issm-2019-proceedings-of-the-2019-acm-sigplan-international-symposium-on-memory-management/)
  batches cross-thread deallocation without locks.
- [Hoard: A Scalable Memory Allocator](https://people.cs.umass.edu/~emery/pubs/berger-asplos2000.pdf)
  uses per-processor heaps to reduce false sharing and bounds memory blowup.
- [Arachne: Core-Aware Thread Management](https://www.usenix.org/conference/osdi18/presentation/qin)
  demonstrates the latency cost of kernel-managed short-lived threads and uses
  one long-lived kernel thread per assigned core.
- [An Implementation of Scheduler Activations on NetBSD](https://www.usenix.org/conference/2002-usenix-annual-technical-conference/implementation-scheduler-activations-netbsd)
  distinguishes cheap user-level scheduling from more expensive kernel thread
  creation, synchronization, and disposal.
- [Rust `Vec` documentation](https://doc.rust-lang.org/stable/std/vec/struct.Vec.html)
  describes contiguous allocation, capacity reservation, and the reallocation
  required when length exceeds capacity.
- [Rust clone-on-write documentation](https://doc.rust-lang.org/std/borrow/enum.Cow.html)
  provides the borrowed-or-owned primitive used for unchanged module source.
- [Rust atomic reference counting documentation](https://doc.rust-lang.org/std/sync/struct.Arc.html)
  documents shared ownership and its atomic reference-count cost.
- [The Slab Allocator](https://www.usenix.org/conference/usenix-summer-1994-technical-conference/slab-allocator-object-caching-kernel)
  motivates object reuse for stable kernel allocation workloads. Once's
  short-lived, size-varying command allocations did not justify a custom slab.
- [Magazines and vmem](https://www.usenix.org/legacy/event/usenix01/full_papers/bonwick/bonwick_html/)
  shows how per-processor object caches reduce allocator contention. Once's
  current-hit path is primarily single-threaded, so removing allocations had
  more value than adding another allocator layer.
- [Linux zero-copy receive documentation](https://www.kernel.org/doc/html/next/networking/iou-zcrx.html)
  describes a specialized network receive path with kernel and hardware
  requirements. It is not portable enough for Once's generic cache client.

### Rebase onto bounded-memory main

The campaign worktree rebased onto origin/main which added bounded memory
management (`ResourcePool`, `--memory-limit`, streaming I/O, 250 MiB default
action estimate, scheduler concurrency derived from memory budget). All six
conflicting files were resolved by layering the campaign features (transitive
artifact identities, critical-depth scheduling, build receipts, source digest
cache, change tracker) on top of the upstream resource-bounding infrastructure.
The release build and focused tests pass. A new cache must be primed with the
rebased binary before lazy materialization and cold/warm comparisons resume.

### Lazy materialization measured

The corrected transitive lazy materialization design (carrying typed artifact
identities through BuildOutcome, materializing only declared inputs and final
outputs) was measured against the eager control on a complete-cache,
empty-output-tree scenario:

- Eager control: 47.799 seconds, 2740 files, 4.5 GiB output tree
- Lazy treatment: 8.870 seconds, 121 files, 658 MiB output tree
- Improvement: 5.4 times faster, 82 percent less filesystem traffic
- Receipt follow-up: 0.297 seconds

The treatment is retained. The rebased code restored the scheduler-level
`materialize_cached_outputs` call for Starlark correctness, so the rebased
binary cannot achieve this without further work to make the materialization
demand-driven rather than eager.
