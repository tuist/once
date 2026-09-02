# Warm-loop bench

Measures Once's per-build overhead when every action already has a local
cache hit. Catches regressions like a change tracker leak that adds
seconds per build without changing any output. Runs in CI on `pull_request`
against `ubuntu-latest` and `macos-latest`; macOS coverage matters
because the filesystem-watching backend differs per platform and some
overheads (FSEvents, in particular) show up only there.

## Fixture

Reuses `benchmarks/cache-comparison/once`, a deterministic 15-action
graph. Each action reads a tiny input, sleeps 40 milliseconds, and
writes a controlled artifact between 1 and 16 mebibytes. The bench
forces `ONCE_CACHE_PROVIDER=local` so no network or server is required.

## Run

Build the release binary first, then run:

```sh
mise exec -- cargo build --release --package once-cli
benchmarks/warm-loop/run.sh
```

`RUNS` overrides the default five measured warm runs. `ONCE_BINARY`
points at an alternative binary. `CEILING_SECONDS` (read by `check.sh`)
overrides the catastrophic ceiling.

## Signal

The first build populates the local cache and is not timed. The
measured runs are all warm no-ops; a healthy Once completes each of
them in well under a second. CI publishes median, min, and max on
every PR, and fails only when the median exceeds a loose ceiling
(default 10 seconds) that catches "something is very wrong" without
gating on normal variance. Tighten the ceiling once trend data on
main gives an honest baseline.
