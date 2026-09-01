#shellcheck shell=bash

Describe 'xcodebuild compatibility'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  install_system_xcodebuild() {
    mkdir -p "$WORKSPACE/tools"
    cat > "$WORKSPACE/tools/xcodebuild" <<'SH'
#!/bin/sh
printf '%s\n' "$*"
exit "${XCODEBUILD_EXIT_CODE:-0}"
SH
    chmod +x "$WORKSPACE/tools/xcodebuild"
  }

  It 'passes unsupported invocations through without a Once trailer'
    install_system_xcodebuild

    When call env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" xcodebuild -- -showBuildSettings
    The status should be success
    The stdout should equal '-showBuildSettings'
    The stderr should not include 'cache '
  End

  It 'preserves the system xcodebuild exit status for unsupported invocations'
    install_system_xcodebuild

    When call env XCODEBUILD_EXIT_CODE=17 PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" xcodebuild -- -showBuildSettings
    The status should be failure
    The status should equal 17
    The stdout should equal '-showBuildSettings'
  End
End
