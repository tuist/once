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
| Bazel | 2.284 s | 2.080 s | 1.828 to 2.921 s | 15 of 15 |
| Buck2 | 0.474 s | 0.445 s | 0.423 to 0.630 s | 15 of 15 |
| Once | 0.192 s | 0.192 s | 0.169 to 0.208 s | 15 of 15 |

The Bazel and Buck2 results include a fresh daemon after each clean. Their
ranges therefore include daemon startup. Once starts a fresh process on every
invocation and is 2.5 times faster than Buck2 and 11.9 times faster than Bazel
by mean in this clean-client scenario.

### Local hit

| Client | Mean | Median | Range |
| --- | ---: | ---: | ---: |
| Bazel | 0.863 s | 0.833 s | 0.611 to 1.391 s |
| Buck2 | 0.065 s | 0.062 s | 0.057 to 0.090 s |
| Once | 0.030 s | 0.029 s | 0.027 to 0.036 s |

Once is 28.4 times faster than Bazel and 2.12 times faster than Buck2 by mean
on a local hit. Buck2 uses a long-lived process, while Once starts a fresh
process for every build. Once's 30-millisecond mean covers process startup,
graph loading, analysis, 15 local action-cache probes, validation of the
requested output, and no output rewrite when its digest and permissions still
match.

The raw hyperfine data is in
[`results/remote-hit-clean-client.json`](results/remote-hit-clean-client.json)
and [`results/local-hit.json`](results/local-hit.json).

### Once before and after

| Scenario | Before | After | Improvement |
| --- | ---: | ---: | ---: |
| Clean-client remote hit | 2.475 s | 0.192 s | 12.9 times faster |
| Local hit | 2.398 s | 0.030 s | 78.9 times faster |
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

Once is now more than twice as fast as Buck2 on this local-hit workload without
keeping a process alive. The remaining fixed work is dominated by process and
benchmark-wrapper startup. Once itself runs from session start to finish in
about 11 milliseconds on a stable local hit. A persistent Once service could
remove more startup work, but it now has a small payoff and would add lifecycle
and invalidation complexity.

The scheduler can also move downstream cache lookups earlier when their
dependency metadata is already known, reducing round trips on deeper graphs
and higher-latency remote servers.

## Independent review

A headless Claude review reached the same priorities independently: repair
large-blob transfers first, make write failures observable, add missing-blob
checks and compression, then move action lookups earlier and make output
materialization lazy. The review also highlighted that production round-trip
latency makes the current dependency-by-dependency lookup order more expensive
than this loopback benchmark shows.

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
