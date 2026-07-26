# Autoresearch: reduce Once local-hit latency

## Objective

Reduce end-to-end local cache-hit latency for the 15-target cache-comparison
workspace. Retain only changes that improve repeated release-build measurements
without weakening correctness, portability, structured logging, or graph
semantics.

## Metrics

- Primary: median local-hit latency (`local_hit_ms`, milliseconds, lower is better)
- Secondary: mean latency (`mean_ms`) and standard deviation (`stddev_ms`)

## How to Run

`./autoresearch.sh`

The script builds the release executable before measurement, warms the existing
local action cache, and measures 40 fresh command invocations after five warmup
runs. It starts the local benchmark server only when needed to populate an empty
client cache.

## Files in Scope

- `crates/once-cli/src/main.rs`: process runtime and command dispatch
- `crates/once-cli/src/logging.rs`: per-invocation logging setup and writes
- `crates/once-cli/src/commands/graph/`: graph analysis and scheduling
- `crates/once-frontend/src/`: manifest, module, and graph loading
- workspace manifests: allocator and release profile experiments

## Off Limits

- A persistent background process unless measurements show process startup is
  still the dominant controllable cost
- Platform-specific behavior in generic graph or cache interfaces
- Skipping required session logs
- Benchmark-only shortcuts that do not improve normal Once workspaces

## Constraints

- Use `mise exec --` for every Rust command.
- Keep the public command and graph behavior compatible.
- Keep expensive filesystem and analysis work eligible for parallel execution.
- Run focused checks after every measured experiment and the full suite before
  handoff.
- Do not use em dashes in user-facing text.

## What's Been Tried

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
