#!/usr/bin/env bash
set -euo pipefail

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
benchmark="$root/benchmarks/cache-comparison"
result="$(mktemp "${TMPDIR:-/tmp}/struo-autoresearch.XXXXXX.json")"
started_server=0

cleanup() {
  find "$result" -maxdepth 0 -type f -delete
  if [[ "$started_server" == 1 ]]; then
    "$benchmark/server.sh" stop
  fi
}
trap cleanup EXIT INT TERM

cd "$root"
mise exec -- cargo build --quiet --release -p struo-cli
"$benchmark/verify-fixtures.sh"

if ! "$benchmark/run-struo.sh" >/dev/null 2>&1; then
    "$benchmark/server.sh" start >/dev/null
    started_server=1
    "$benchmark/run-struo.sh" >/dev/null
fi

mise exec -- hyperfine \
  --runs 40 \
  --warmup 5 \
  --export-json "$result" \
  "$benchmark/run-struo.sh" \
  >/dev/null

mise exec -- jq -r '
  .results[0] |
  "METRIC local_hit_ms=\(.median * 1000)",
  "METRIC mean_ms=\(.mean * 1000)",
  "METRIC stddev_ms=\(.stddev * 1000)"
' "$result"
