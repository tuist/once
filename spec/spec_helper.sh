# shellcheck shell=bash
# Shared shellspec helpers.

REPO_ROOT="$(cd "$SHELLSPEC_PROJECT_ROOT" && pwd)"
FABRIK_BIN="${REPO_ROOT}/target/release/fabrik"
export REPO_ROOT FABRIK_BIN

spec_helper_precheck() {
  if [ ! -x "$FABRIK_BIN" ]; then
    abort "fabrik binary missing at $FABRIK_BIN - run 'cargo build --release' first"
  fi
}

spec_helper_loaded() { :; }

setup_workspace() {
  WORKSPACE="$(mktemp -d -t fabrik-spec.XXXXXX)"
  # Per-test XDG roots so fabrik's CAS, runtime sockets, and
  # materialized data land under the test's tempdir instead of the
  # user's real home. Keeping them as siblings under $WORKSPACE means
  # the existing cleanup (`rm -rf $WORKSPACE`) wipes them automatically.
  XDG_CACHE_HOME="$WORKSPACE/.xdg/cache"
  XDG_STATE_HOME="$WORKSPACE/.xdg/state"
  XDG_DATA_HOME="$WORKSPACE/.xdg/data"
  XDG_CONFIG_HOME="$WORKSPACE/.xdg/config"
  XDG_RUNTIME_DIR="$WORKSPACE/.xdg/runtime"
  mkdir -p "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"
  export WORKSPACE XDG_CACHE_HOME XDG_STATE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_RUNTIME_DIR
}

cleanup_workspace() {
  if [ -n "${WORKSPACE:-}" ] && [ -d "$WORKSPACE" ]; then
    rm -rf "$WORKSPACE"
    unset WORKSPACE
  fi
  unset XDG_CACHE_HOME XDG_STATE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_RUNTIME_DIR
}

fabrik() {
  "$FABRIK_BIN" -C "$WORKSPACE" "$@"
}

copy_examples() {
  mkdir -p "$WORKSPACE/examples"
  cp -R "$REPO_ROOT/examples/." "$WORKSPACE/examples/"
}

stage_elixir_workspace_bin() {
  # Elixir build actions resolve `fabrik` via `workspace_tool`, which
  # falls back to PATH when the workspace has no mise.toml. Drop a
  # symlink to the release binary into a bin dir the test caller
  # prepends to PATH; the real `elixirc` and `elixir` come from the
  # outer mise environment.
  mkdir -p "$WORKSPACE/bin"
  ln -sf "$FABRIK_BIN" "$WORKSPACE/bin/fabrik"
}

# Predicates used by `Skip if` in the apple spec. Each returns 0 when
# the surrounding case should be skipped: non-Darwin host, missing
# xcrun, or no booted iOS simulator. shellspec's `Skip` directive is a
# DSL keyword that doesn't survive being called from a regular shell
# function, hence the predicate-via-exit-code pattern.
apple_toolchain_unavailable() {
  [ "$(uname -s)" != "Darwin" ] || ! command -v xcrun >/dev/null 2>&1
}

no_booted_ios_simulator() {
  apple_toolchain_unavailable && return 0
  ! xcrun simctl list devices booted 2>/dev/null | grep -q "(Booted)"
}
