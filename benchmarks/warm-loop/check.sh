#!/bin/sh
#
# Read a hyperfine JSON result and enforce a catastrophic-regression
# ceiling on the median warm-loop time. The ceiling is intentionally
# loose (a warm build of the 15-action fixture should finish in well
# under a second on a healthy system); tighten once baseline data on
# main is available.

set -eu

file="${1:?path to hyperfine json required}"
ceiling_seconds="${CEILING_SECONDS:-10}"

# Guard the gate against silent no-ops: a missing key or empty array
# would make jq return "null" and the comparison below would pass with
# nothing actually measured. Bail before we lie about a green bench.
case "$ceiling_seconds" in
  ''|*[!0-9]*)
    printf 'FAIL: CEILING_SECONDS must be a positive integer (got %s).\n' \
      "$ceiling_seconds" >&2
    exit 2
    ;;
esac

median="$(mise exec -- jq -r '.results[0].median // "null"' "$file")"
min="$(mise exec -- jq -r '.results[0].min // "null"' "$file")"
max="$(mise exec -- jq -r '.results[0].max // "null"' "$file")"

for label in median min max; do
  eval "value=\$$label"
  case "$value" in
    ''|null)
      printf 'FAIL: hyperfine JSON is missing .results[0].%s (%s).\n' \
        "$label" "$file" >&2
      exit 2
      ;;
  esac
done

printf 'warm-loop median: %ss  min: %ss  max: %ss  ceiling: %ss\n' \
  "$median" "$min" "$max" "$ceiling_seconds"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    printf '### Warm-loop bench\n\n'
    printf '| median | min | max | ceiling |\n'
    printf '| --- | --- | --- | --- |\n'
    printf '| %ss | %ss | %ss | %ss |\n\n' "$median" "$min" "$max" "$ceiling_seconds"
  } >> "$GITHUB_STEP_SUMMARY"
fi

if mise exec -- jq -e --argjson ceiling "$ceiling_seconds" \
    '.results[0].median > $ceiling' "$file" >/dev/null; then
  printf 'FAIL: warm-loop median (%ss) exceeded catastrophic ceiling (%ss).\n' \
    "$median" "$ceiling_seconds" >&2
  exit 1
fi
