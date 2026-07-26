#!/usr/bin/env bash
set -euo pipefail

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
benchmark="$root/benchmarks/cache-comparison"
result="$(mktemp "${TMPDIR:-/tmp}/once-autoresearch.XXXXXX.json")"
started_server=0

cleanup() {
  find "$result" -maxdepth 0 -type f -delete
  if [[ "$started_server" == 1 ]]; then
    "$benchmark/server.sh" stop
  fi
}
trap cleanup EXIT INT TERM

cd "$root"
mise exec -- cargo build --quiet --release -p once-cli
"$benchmark/verify-fixtures.sh"

if ! curl -fsS http://127.0.0.1:18080/status >/dev/null 2>&1; then
    "$benchmark/server.sh" start >/dev/null
    started_server=1
fi
"$benchmark/reset-clients.sh" once
"$benchmark/run-once.sh" >/dev/null

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
