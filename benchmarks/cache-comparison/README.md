# Cache comparison

This benchmark reconstructs the repository as the same deterministic 15-action
graph in Bazel, Buck2, and Once. It isolates cache lookup, transfer,
materialization, and invalidation behavior from compiler-specific work.

The graph represents the repository surfaces and their operational dependency
shape:

| Surface | Representative actions |
| --- | --- |
| Rust workspace | content-addressed storage, core, frontend, command line |
| Software development kits | native bridge, JavaScript, Ruby |
| Documentation | generated reference, website |
| Web application | dependencies, compilation, assets, release |
| Distribution | Helm package and final distribution |

Every action reads deterministic inputs, waits 40 milliseconds, and emits a
controlled artifact between 1 and 16 mebibytes. The complete graph produces 84
mebibytes across 15 artifacts. All three implementations invoke Node directly
with an argument list.

The remote cache is
[bazel-remote](https://github.com/buchgr/bazel-remote), served locally over the
[Remote Execution application programming interface](https://github.com/bazelbuild/remote-apis).
The timing harness is [hyperfine](https://github.com/sharkdp/hyperfine).

## Run

Build the release Once binary first, then run:

```sh
mise exec -- cargo build --release --package once-cli
benchmarks/cache-comparison/benchmark.sh
```

Set `RUNS` to change the default five measured runs:

```sh
RUNS=10 benchmarks/cache-comparison/benchmark.sh
```

The harness first fills one isolated remote namespace per build system. Every
timed run starts with no client action cache and no materialized output, while
preserving the shared remote server. Results include hyperfine data, final
server status, and request counters.

The controlled graph is intentionally not a compiler throughput benchmark.
Dependency fetching and tool installation happen before measurement. Its
purpose is to answer how much work and bandwidth each client spends when the
same graph is already available remotely.

See [REPORT.md](REPORT.md) for the measured comparison, the Once cache changes
that followed from it, and the remaining opportunities.
