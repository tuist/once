#shellcheck shell=bash

setup_nested_workspace() {
  NESTED_WORKSPACE="$(mktemp -d -t once-nested-spec.XXXXXX)"
  export NESTED_WORKSPACE
}

cleanup_nested_workspace() {
  if [ -n "${NESTED_WORKSPACE:-}" ] && [ -d "$NESTED_WORKSPACE" ]; then
    rm -rf "$NESTED_WORKSPACE"
    unset NESTED_WORKSPACE
  fi
}

once_under_test() {
  "$ONCE_BIN" -C "$NESTED_WORKSPACE" "$@"
}
