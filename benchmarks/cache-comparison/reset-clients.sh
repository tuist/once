#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
state="$root/.state"

reset_struo() {
  mkdir -p "$root/struo/.struo" "$state/struo-client/cache/struo/cas"
  find "$root/struo/.struo" -mindepth 1 -delete
  find "$state/struo-client/cache/struo/cas" -mindepth 1 -delete
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
  struo) reset_struo ;;
  bazel) reset_bazel ;;
  buck2) reset_buck2 ;;
  all)
    reset_struo
    reset_bazel
    reset_buck2
    ;;
  *)
    echo "usage: $0 struo|bazel|buck2|all" >&2
    exit 2
    ;;
esac
