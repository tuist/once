# Cache comparison report

Measured on 26 July 2026 on an Apple silicon macOS host. The benchmark used
Bazel 9.2.0, Buck2 2026-07-15, Once built in release mode, bazel-remote 2.6.2,
and hyperfine 1.20.0.

## Scope

The benchmark represents every major project surface as one shared
deterministic graph:

- five Rust crate groups
- the native, JavaScript, and Ruby software development kits
- reference generation and the documentation website
- web dependencies, compilation, assets, and release packaging
- the Helm chart and final distribution

The three workspaces have identical fixture inputs and action behavior. The
fixture check compares every copied source before a run. Each of the 15 actions
waits 40 milliseconds and creates a deterministic artifact. The graph produces
84 mebibytes in total, with individual outputs from 1 to 16 mebibytes.

This is a cache client comparison, not a compiler throughput comparison. Tool
installation and dependency fetching happen before measurement. The remote
server runs on the same host, so production network latency would amplify
differences in request count and scheduling.

## Results

Each remote-hit run starts after deleting the client's action cache and
materialized outputs while preserving the populated remote cache. Each local
run preserves the client state from the preceding run. Hyperfine recorded ten
runs per client.

### Remote hit with a clean client

| Client | Mean | Median | Range | Remote action hits |
| --- | ---: | ---: | ---: | ---: |
| Bazel | 2.016 s | 2.071 s | 1.761 to 2.311 s | 15 of 15 |
| Buck2 | 0.697 s | 0.509 s | 0.452 to 1.854 s | 15 of 15 |
| Once | 0.061 s | 0.042 s | 0.037 to 0.241 s | 15 of 15 |

The Bazel and Buck2 results include a fresh daemon after each clean. Their
ranges therefore include daemon startup. Once starts a fresh process on every
invocation and is 11.4 times faster than Buck2 and 33.0 times faster than Bazel
by mean in this clean-client scenario. Once's first sample was a 241-millisecond
outlier; its other nine samples were between 37 and 45 milliseconds. A separate
30-run stable repeat measured a 36.27-millisecond median.

### Local hit

| Client | Mean | Median | Range |
| --- | ---: | ---: | ---: |
| Bazel | 0.868 s | 0.786 s | 0.570 to 1.491 s |
| Buck2 | 0.059 s | 0.058 s | 0.054 to 0.068 s |
| Once | 0.021 s | 0.020 s | 0.019 to 0.026 s |

Once is 41.4 times faster than Bazel and 2.83 times faster than Buck2 by mean
on a local hit. Buck2 uses a long-lived process, while Once starts a fresh
process for every build. Once's 21-millisecond mean covers process startup,
graph loading, analysis, 15 local action-cache probes, validation of the
requested output, and no output rewrite when its digest and permissions still
match.

The raw hyperfine data is in
[`results/remote-hit-clean-client.json`](results/remote-hit-clean-client.json)
and [`results/local-hit.json`](results/local-hit.json).

### Once before and after

| Scenario | Before | After | Improvement |
| --- | ---: | ---: | ---: |
| Clean-client remote hit | 2.475 s | 0.061 s | 40.5 times faster |
| Local hit | 2.398 s | 0.021 s | 114.2 times faster |
| Clean-client output materialization | 84 mebibytes | 1 mebibyte | 84 times less |

The before measurements used the same graph, host, remote server, and release
build settings. The after measurements use ten runs instead of five.

## Transfer and materialization

An isolated clean-client probe against the populated server produced:

| Client | Action lookups | Output blob reads | Logical output bytes materialized |
| --- | ---: | ---: | ---: |
| Bazel | 15 | 1 streamed read | 1 mebibyte |
| Buck2 | 15 | 1 batched read | 1 mebibyte |
| Once | 15 | 1 batched read | 1 mebibyte |

Bazel's top-level download policy and Buck2's deferred materialization fetch
only the requested distribution artifact. Once now does the same. Its remote
action metadata carries the native output digests needed to construct
downstream action keys, so a hit does not require the output body. Intermediate
outputs are downloaded only if a downstream action misses and must execute.

Once also negotiates Zstandard compression for blobs that do need to move.
Real compiler outputs will usually compress less than this deterministic
fixture, so avoiding the transfer remains more valuable than compressing it.

## Cache effectiveness

Before the transfer changes in this work, a clean Once client recovered only 5
of the 15 actions. Outputs above the unary message limit failed to upload or
download. Remote write failures were ignored, so the build still appeared
successful and the loss in future hit rate was silent.

After the changes, the same probe returns all 15 action results. Once uses the
Remote Execution protocol's byte stream for large blobs, validates every
response and digest, reports remote write failures through structured logging,
and can consume native action metadata without fetching intermediate blobs.

The graph's content-derived keys provide the expected invalidation boundaries:

| Changed input | Rebuilt actions | Reused actions |
| --- | ---: | ---: |
| Final distribution | 1 | 14 |
| Web dependencies | 5 | 10 |
| Rust content store | 9 | 6 |

The action keys are strong enough to preserve these boundaries in all three
workspaces.

## Changes made to Once

The remote provider now:

- discovers server capabilities once per process
- checks for missing blobs before uploading
- uses streamed reads and writes for blobs above 2 mebibytes
- transfers stream data in 2-mebibyte chunks
- negotiates Zstandard compression for streams and batches
- keeps at most eight blob transfers active at once
- validates upload status, decompressed size, and the
  [Secure Hash Algorithm 256-bit](https://csrc.nist.gov/pubs/fips/180-4/upd1/final)
  digest
- remembers the native-to-remote digest mapping within the process
- defers remote network client construction until a request misses the local
  tier
- mirrors remotely sourced action results with an atomic recoverable write,
  while locally produced results retain crash-survivable durability
- validates remotely fetched blob content before writing it, synchronizes the
  file contents, and treats the directory entry as a recoverable mirror
- borrows incompressible blob input during local storage instead of copying it
- reserves bounded capacity for streamed remote reads to avoid repeated buffer
  growth copies
- avoids fetching the canonical empty blob
- logs remote action and blob upload failures instead of discarding them

The graph executor now:

- stores native Once action metadata in remote action results
- reconstructs downstream action keys from metadata without downloading output
  bodies
- materializes only requested top-level outputs on cache hits
- materializes a cached dependency only when a downstream action must execute
- validates an existing requested file and skips rewriting it when its digest
  and permissions still match
- resolves the workspace toolchain once instead of spawning toolchain-manager
  processes for every target
- persists validated graph tool paths and only installs tools when resolution
  proves that one is missing
- persists safe version probes for pinned graph tools with configuration,
  executable, and relevant environment invalidation
- compiles and freezes the Starlark module once per command, then invokes every
  target implementation from that shared immutable program
- constructs the graph schema from the same compiled Starlark module instead
  of evaluating the built-in module twice
- compiles only the built-in Starlark modules needed by target kinds present in
  the workspace, with module dependencies declared by the prelude index
- loads workspace manifests once for targeted analysis and graph construction
- analyzes independent ready targets concurrently

These changes fix the correctness gap and minimize transferred bytes without
making the generic cache interface toolchain-specific.

The command runtime now uses a current-thread asynchronous event loop while
keeping analysis and file work on the parallel blocking pool. This removes
unneeded kernel thread startup and shutdown without serializing independent
work. Internal logs write directly to their per-invocation file, avoiding a
dedicated logging thread.

## Next opportunities

Once is now nearly three times as fast as Buck2 on this local-hit workload
without keeping a process alive. The benchmark shell wrapper alone measures
about 6 milliseconds, while the full local hit measures about 18 to 20
milliseconds on a quiet repeat. A persistent Once service could remove more
startup work, but it now has a small payoff and would add lifecycle and
invalidation complexity.

The scheduler can also move downstream cache lookups earlier when their
dependency metadata is already known, reducing round trips on deeper graphs
and higher-latency remote servers.

## Independent review

A headless Claude review reached the same priorities independently: repair
large-blob transfers first, make write failures observable, add missing-blob
checks and compression, then move action lookups earlier and make output
materialization lazy. The review also highlighted that production round-trip
latency makes the current dependency-by-dependency lookup order more expensive
than this loopback benchmark shows. Follow-up reviews identified the eager
remote-client construction, raw local-storage copy, streamed-read growth
copies, and recoverable directory synchronization that are now removed.

## Validation

The following checks passed after the cache changes and benchmark run:

```sh
mise exec -- cargo test --workspace
mise exec -- cargo clippy --workspace --all-targets -- -D warnings
mise exec -- cargo fmt --all -- --check
mise exec -- shellspec
benchmarks/cache-comparison/verify-fixtures.sh
```

The Once benchmark module and its 15-target workspace also pass the structured
module and workspace validators.
