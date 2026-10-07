#!/usr/bin/env bash
set -euo pipefail
root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
control="$(mktemp -d "${TMPDIR:-/tmp}/once-runtime-control.XXXXXX")"
rmdir "$control"
cd "$root"
cleanup() {
  git worktree remove --force "$control" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM
git worktree add --quiet --detach "$control" "${2:-80c1709}"
if [[ "${1:-control-first}" == "treatment-first" ]]; then
  "$root/autoresearch.sh"
  "$root/.auto/process-floor.sh"
  mise exec -- cargo build --quiet --release -p once-cli --manifest-path "$control/Cargo.toml" --target-dir "$root/target"
  "$root/.auto/measure-prebuilt.sh"
  mise exec -- cargo build --quiet --release -p once-cli
else
  mise exec -- cargo build --quiet --release -p once-cli --manifest-path "$control/Cargo.toml" --target-dir "$root/target"
  "$root/.auto/measure-prebuilt.sh"
  "$root/autoresearch.sh"
  "$root/.auto/process-floor.sh"
fi
