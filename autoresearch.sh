#!/usr/bin/env bash
set -euo pipefail

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
benchmark="$root/benchmarks/codex-build-comparison/benchmark.py"
checkout="${ONCE_CODEX_CHECKOUT:-/tmp/once-codex-benchmark}"
state_root="${ONCE_CODEX_STATE_ROOT:-/tmp/once-codex-autoresearch}"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/once-autoresearch.XXXXXX")"
result="$temporary/results.jsonl"
logs="$temporary/logs"
mkdir -p "$logs"

cleanup() {
  find "$temporary" -depth -delete
}
trap cleanup EXIT INT TERM

cd "$root"
mise exec -- cargo build --quiet --release -p once-cli

python3 "$benchmark" once warm \
  --checkout "$checkout" \
  --once "$root/target/release/once" \
  --state-root "$state_root" \
  --results "$result" \
  --log-directory "$logs" \
  >/dev/null

for _ in 1 2 3; do
  python3 "$benchmark" once warm \
    --checkout "$checkout" \
    --once "$root/target/release/once" \
    --state-root "$state_root" \
    --results "$result" \
    --log-directory "$logs" \
    >/dev/null
done

mise exec -- jq -r '
  [inputs] | .[1:] |
  "METRIC warm_elapsed_seconds=\([.[].elapsed_seconds] | sort | .[1])"
' /dev/null "$result"
