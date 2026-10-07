#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
result="$(mktemp "${TMPDIR:-/tmp}/once-process-floor.XXXXXX.json")"
trap 'rm -f "$result"' EXIT
mise exec -- hyperfine --runs 40 --warmup 5 --export-json "$result" './target/release/once --version' >/dev/null
mise exec -- jq -r '.results[0] | "METRIC process_floor_ms=\(.median * 1000)"' "$result"
