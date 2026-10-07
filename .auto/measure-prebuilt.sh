#!/usr/bin/env bash
set -euo pipefail
root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
benchmark="$root/benchmarks/cache-comparison"
result="$(mktemp "${TMPDIR:-/tmp}/once-control.XXXXXX.json")"
started_server=0
cleanup() {
  rm -f "$result"
  if [[ "$started_server" == 1 ]]; then
    "$benchmark/server.sh" stop
  fi
}
trap cleanup EXIT INT TERM
cd "$root"
"$benchmark/verify-fixtures.sh"
if ! "$benchmark/run-once.sh" >/dev/null 2>&1; then
  "$benchmark/server.sh" start >/dev/null
  started_server=1
  "$benchmark/run-once.sh" >/dev/null
fi
mise exec -- hyperfine --runs 40 --warmup 5 --export-json "$result" "$benchmark/run-once.sh" >/dev/null
mise exec -- jq -r '.results[0] | "METRIC control_local_hit_ms=\(.median * 1000)", "CONTROL_DIAGNOSTIC mean_ms=\(.mean * 1000) stddev_ms=\(.stddev * 1000)"' "$result"
