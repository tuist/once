#!/usr/bin/env bash
set -euo pipefail

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
benchmark="$root/benchmarks/cache-comparison"
result="$(mktemp "${TMPDIR:-/tmp}/once-autoresearch.XXXXXX.json")"

cleanup() {
  find "$result" -maxdepth 0 -type f -delete
  "$benchmark/server.sh" stop
}
trap cleanup EXIT INT TERM

cd "$root"
mise exec -- cargo build --quiet --release -p once-cli
"$benchmark/verify-fixtures.sh"
"$benchmark/server.sh" start >/dev/null
"$benchmark/reset-clients.sh" once
"$benchmark/run-once.sh" >/dev/null
"$benchmark/reset-clients.sh" once

mise exec -- hyperfine \
  --runs 20 \
  --warmup 0 \
  --prepare "$benchmark/reset-clients.sh once" \
  --export-json "$result" \
  "$benchmark/run-once.sh" \
  >/dev/null

mise exec -- jq -r '
  .results[0] |
  "METRIC remote_hit_ms=\(.median * 1000)",
  "METRIC mean_ms=\(.mean * 1000)",
  "METRIC stddev_ms=\(.stddev * 1000)"
' "$result"
