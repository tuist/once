#shellcheck shell=bash

setup_nested_workspace() {
  NESTED_WORKSPACE="$(mktemp -d -t struo-nested-spec.XXXXXX)"
  export NESTED_WORKSPACE
}

cleanup_nested_workspace() {
  if [ -n "${NESTED_WORKSPACE:-}" ] && [ -d "$NESTED_WORKSPACE" ]; then
    rm -rf "$NESTED_WORKSPACE"
    unset NESTED_WORKSPACE
  fi
}

struo_under_test() {
  "$STRUO_BIN" -C "$NESTED_WORKSPACE" "$@"
}
