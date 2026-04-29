# shellcheck shell=bash
# Shared shellspec helpers.

REPO_ROOT="$(cd "$SHELLSPEC_PROJECT_ROOT" && pwd)"
FABRIK_BIN="${REPO_ROOT}/target/release/fabrik"
export REPO_ROOT FABRIK_BIN

spec_helper_precheck() {
  if [ ! -x "$FABRIK_BIN" ]; then
    abort "fabrik binary missing at $FABRIK_BIN — run 'cargo build --release' first"
  fi
}

spec_helper_loaded() { :; }

setup_workspace() {
  WORKSPACE="$(mktemp -d -t fabrik-spec.XXXXXX)"
  export WORKSPACE
}

cleanup_workspace() {
  if [ -n "${WORKSPACE:-}" ] && [ -d "$WORKSPACE" ]; then
    rm -rf "$WORKSPACE"
    unset WORKSPACE
  fi
}

fabrik() {
  "$FABRIK_BIN" -C "$WORKSPACE" "$@"
}
