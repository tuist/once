#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
state="$root/.state"

reset_once() {
  mkdir -p "$root/once/.once" "$state/once-client/cache/once/cas"
  find "$root/once/.once" -mindepth 1 -delete
  find "$state/once-client/cache/once/cas" -mindepth 1 -delete
}

reset_bazel() {
  (
    cd "$root/bazel"
    mise exec -- bazelisk clean --expunge >/dev/null 2>&1
  )
}

reset_buck2() {
  (
    cd "$root/buck2"
    mise exec -- buck2 clean >/dev/null 2>&1
  )
}

case "${1:-all}" in
  once) reset_once ;;
  bazel) reset_bazel ;;
  buck2) reset_buck2 ;;
  all)
    reset_once
    reset_bazel
    reset_buck2
    ;;
  *)
    echo "usage: $0 once|bazel|buck2|all" >&2
    exit 2
    ;;
esac
