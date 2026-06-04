#shellcheck shell=bash
# Opt-in end-to-end specs for the production Tuist cache provider.

Describe 'Tuist Kura cache provider'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'persists and restores annotated script artifacts through the tuist account cache'
    Skip if 'Tuist Kura specs are opt-in' tuist_kura_specs_disabled
    Skip if 'TUIST_TOKEN or ONCE_TUIST_E2E_TOKEN is required for tuist.dev' tuist_kura_token_missing

    token="${ONCE_TUIST_E2E_TOKEN:-${TUIST_TOKEN:-}}"
    run_id="$(date +%s)-$$"

    cat > "$WORKSPACE/Tuist.swift" <<'EOF'
import ProjectDescription

let tuist = Tuist(
    fullHandle: "tuist/once-kura-e2e",
    url: "https://tuist.dev",
    project: .xcode()
)
EOF

    mkdir -p "$WORKSPACE/scripts" "$WORKSPACE/src"
    printf 'input-%s' "$run_id" > "$WORKSPACE/src/input.txt"
    cat > "$WORKSPACE/scripts/build.sh" <<EOF
#!/usr/bin/env -S $ONCE_BIN exec -- bash
# once input "../src/input.txt"
# once output "../dist/artifact.txt"
# once cwd ".."
set -euo pipefail
payload="\$(cat src/input.txt)"
printf 'executed' >> executions.log
mkdir -p dist
printf 'artifact:%s' "\$payload" > dist/artifact.txt
printf 'stdout:%s' "\$payload"
EOF
    chmod +x "$WORKSPACE/scripts/build.sh"

    env TUIST_TOKEN="$token" PATH=/usr/bin:/bin "$WORKSPACE/scripts/build.sh" \
      > "$WORKSPACE/first.out" 2> "$WORKSPACE/first.err"
    first_status=$?
    if [ "$first_status" -ne 0 ]; then
      printf '%s\n' "first run failed with status $first_status" >&2
      cat "$WORKSPACE/first.err" >&2
      return "$first_status"
    fi

    first_artifact="$(cat "$WORKSPACE/dist/artifact.txt")"
    rm -rf "$WORKSPACE/dist"

    XDG_CACHE_HOME="$WORKSPACE/.xdg-cold/cache"
    XDG_STATE_HOME="$WORKSPACE/.xdg-cold/state"
    XDG_DATA_HOME="$WORKSPACE/.xdg-cold/data"
    XDG_CONFIG_HOME="$WORKSPACE/.xdg-cold/config"
    XDG_RUNTIME_DIR="$WORKSPACE/.xdg-cold/runtime"
    mkdir -p "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"
    export XDG_CACHE_HOME XDG_STATE_HOME XDG_DATA_HOME XDG_CONFIG_HOME XDG_RUNTIME_DIR

    When call env TUIST_TOKEN="$token" PATH=/usr/bin:/bin "$WORKSPACE/scripts/build.sh"
    The status should be success
    The stdout should equal "$(cat "$WORKSPACE/first.out")"
    The stderr should include 'cache hit'
    The contents of file "$WORKSPACE/executions.log" should equal 'executed'
    The contents of file "$WORKSPACE/dist/artifact.txt" should equal "$first_artifact"
  End
End
