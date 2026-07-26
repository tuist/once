#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
runs="${RUNS:-5}"
results="$root/results"

mkdir -p "$results"
"$root/verify-fixtures.sh"
"$root/server.sh" start
trap '"$root/server.sh" stop' EXIT INT TERM

"$root/reset-clients.sh" all
"$root/run-bazel.sh" >/dev/null
"$root/reset-clients.sh" bazel
"$root/run-buck2.sh" >/dev/null
"$root/reset-clients.sh" buck2
"$root/run-once.sh" >/dev/null
"$root/reset-clients.sh" all

mise exec -- hyperfine \
  --runs "$runs" \
  --warmup 0 \
  --export-json "$results/remote-hit-clean-client.json" \
  --prepare "$root/reset-clients.sh all" \
  --command-name Bazel "$root/run-bazel.sh" \
  --command-name Buck2 "$root/run-buck2.sh" \
  --command-name Once "$root/run-once.sh"

mise exec -- hyperfine \
  --runs "$runs" \
  --warmup 1 \
  --export-json "$results/local-hit.json" \
  --command-name Bazel "$root/run-bazel.sh" \
  --command-name Buck2 "$root/run-buck2.sh" \
  --command-name Once "$root/run-once.sh"

curl -fsS http://127.0.0.1:18080/status >"$results/cache-status.json"
curl -fsS http://127.0.0.1:18080/metrics >"$results/cache-metrics.txt"
