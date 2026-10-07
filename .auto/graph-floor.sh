#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
result="$(mktemp "${TMPDIR:-/tmp}/once-graph-hit.XXXXXX.json")"
trap 'rm -f "$result"' EXIT
mise exec -- hyperfine --runs 40 --warmup 5 --export-json "$result" "env ONCE_CHANGE_TRACKER=0 $root/benchmarks/cache-comparison/run-once.sh" >/dev/null
mise exec -- jq -r '.results[0] | "METRIC graph_hit_ms=\(.median * 1000)", "GRAPH_DIAGNOSTIC mean_ms=\(.mean * 1000) stddev_ms=\(.stddev * 1000)"' "$result"
