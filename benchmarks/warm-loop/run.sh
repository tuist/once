#!/bin/sh
#
# Measure Once's warm-loop overhead: how long a no-op rebuild takes
# when every action already has a local cache hit. Guards against
# regressions like a change-tracker leak that adds seconds per build
# without changing any output.
#
# Reuses the deterministic 15-action fixture from
# benchmarks/cache-comparison/once and forces the cache provider to
# local so no network or server is required.

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
repository="$(CDPATH= cd -- "$root/../.." && pwd)"
fixture="$repository/benchmarks/cache-comparison/once"
raw_binary="${ONCE_BINARY:-$repository/target/release/once}"
runs="${RUNS:-5}"
results="$root/results"
state="$root/.state"

# Resolve the binary before the timed command changes directory into
# the fixture. A relative ONCE_BINARY like `target/release/once` would
# otherwise pass the existence check here (interpreted against the
# caller's cwd) and then fail during the run.
if ! binary="$(CDPATH= cd "$(dirname "$raw_binary")" 2>/dev/null && pwd)/$(basename "$raw_binary")"; then
  printf 'once binary path %s could not be resolved\n' "$raw_binary" >&2
  exit 2
fi

if [ ! -x "$binary" ]; then
  printf 'once binary not found at %s\n' "$binary" >&2
  printf 'build it with: mise exec -- cargo build --release --package once-cli\n' >&2
  exit 2
fi

rm -rf "$state" "$results"
mkdir -p "$state/cache" "$state/config" "$results"

build_cmd="cd '$fixture' && \
  ONCE_CACHE_PROVIDER=local \
  XDG_CACHE_HOME='$state/cache' \
  XDG_CONFIG_HOME='$state/config' \
  '$binary' build distribution --format json --quiet"

sh -c "$build_cmd" >/dev/null

mise exec -- hyperfine \
  --runs "$runs" \
  --warmup 0 \
  --export-json "$results/warm-loop.json" \
  --command-name once-warm-loop \
  "$build_cmd"

"$root/check.sh" "$results/warm-loop.json"
