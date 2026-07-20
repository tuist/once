# shellcheck shell=bash
# Shared shellspec helpers.

REPO_ROOT="$(cd "$SHELLSPEC_PROJECT_ROOT" && pwd)"
ONCE_BIN="${REPO_ROOT}/target/release/once"
export REPO_ROOT ONCE_BIN

spec_helper_precheck() {
  if [ ! -x "$ONCE_BIN" ]; then
    abort "once binary missing at $ONCE_BIN - run 'cargo build --release' first"
  fi
}

spec_helper_loaded() { :; }

setup_workspace() {
  WORKSPACE="$(mktemp -d -t once-spec.XXXXXX)"
  # Per-test XDG roots so once's CAS, runtime sockets, and
  # materialized data land under the test's tempdir instead of the
  # user's real home. Keeping them as siblings under $WORKSPACE means
  # the existing cleanup (`rm -rf $WORKSPACE`) wipes them automatically.
  XDG_CACHE_HOME="$WORKSPACE/.xdg/cache"
  XDG_STATE_HOME="$WORKSPACE/.xdg/state"
  XDG_DATA_HOME="$WORKSPACE/.xdg/data"
  XDG_CONFIG_HOME="$WORKSPACE/.xdg/config"
  XDG_RUNTIME_DIR="$WORKSPACE/.xdg/runtime"
  SPEC_ORIGINAL_HOME="${HOME:-}"
  SPEC_ORIGINAL_MSB_PATH="${MSB_PATH:-}"
  HOME="$WORKSPACE/.home"
  mkdir -p "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR" "$HOME"
  export WORKSPACE XDG_CACHE_HOME XDG_STATE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_RUNTIME_DIR HOME SPEC_ORIGINAL_HOME
  if [ "${ONCE_RUN_MICROSANDBOX_SPECS:-}" = "1" ]; then
    MSB_HOME="$(mktemp -d /tmp/once-msb.XXXXXX)"
    if [ -z "${MSB_PATH:-}" ] && [ -x "$SPEC_ORIGINAL_HOME/.microsandbox/bin/msb" ]; then
      MSB_PATH="$SPEC_ORIGINAL_HOME/.microsandbox/bin/msb"
    fi
    export MSB_HOME MSB_PATH
  fi
}

cleanup_workspace() {
  case "${MSB_HOME:-}" in
    /tmp/once-msb.*)
      rm -rf -- "$MSB_HOME"
      ;;
  esac
  unset MSB_HOME
  if [ -n "${SPEC_ORIGINAL_MSB_PATH:-}" ]; then
    MSB_PATH="$SPEC_ORIGINAL_MSB_PATH"
    export MSB_PATH
  else
    unset MSB_PATH
  fi
  if [ -n "${WORKSPACE:-}" ] && [ -d "$WORKSPACE" ]; then
    rm -rf "$WORKSPACE"
    unset WORKSPACE
  fi
  if [ -n "${SPEC_ORIGINAL_HOME:-}" ]; then
    HOME="$SPEC_ORIGINAL_HOME"
    export HOME
  else
    unset HOME
  fi
  unset XDG_CACHE_HOME XDG_STATE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_RUNTIME_DIR SPEC_ORIGINAL_HOME SPEC_ORIGINAL_MSB_PATH
}

once() {
  "$ONCE_BIN" -C "$WORKSPACE" "$@"
}

once_log_dir() {
  case "$(uname -s)" in
    Darwin)
      printf '%s\n' "$HOME/Library/Logs/Once"
      ;;
    *)
      printf '%s\n' "$XDG_STATE_HOME/once/logs"
      ;;
  esac
}

microsandbox_specs_disabled() {
  [ "${ONCE_RUN_MICROSANDBOX_SPECS:-}" != "1" ]
}
