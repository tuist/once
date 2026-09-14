#shellcheck shell=bash

Describe 'explicit Apple modules'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  explicit_module_toolchain_unavailable() {
    [ "$(uname -s)" != Darwin ] || ! xcrun --find swiftc >/dev/null 2>&1
  }

  build_explicit() {
    # Exercise content-based reuse without waiting for an unrelated filesystem watcher.
    ONCE_CHANGE_TRACKER=0 once build "$1" --format json > "$WORKSPACE/build.json" 2> "$WORKSPACE/build.err" || {
      cat "$WORKSPACE/build.err"
      return 1
    }
  }

  exercise_explicit_cache() {
    cp "$REPO_ROOT/fixtures/apple_explicit_modules/once.toml" "$WORKSPACE/"
    cp "$REPO_ROOT/fixtures/apple_explicit_modules/Package.swift" "$WORKSPACE/"
    cp -R "$REPO_ROOT/fixtures/apple_explicit_modules/Sources" "$WORKSPACE/"
    build_explicit Main || return
    [ "$("$WORKSPACE/.once/out/Main/Main")" = 14 ] || return 1
    once query evidence Left:build --format json > "$WORKSPACE/left.json" || return
    once query evidence Right:build --format json > "$WORKSPACE/right.json" || return
    jq -e --slurpfile left "$WORKSPACE/left.json" '
      [.[], $left[0][] | select(.outputs | keys | any(endswith(".pcm")))] |
      group_by(.action_digest) | any(length > 1 and any(.cache == "hit"))
    ' "$WORKSPACE/right.json" >/dev/null || return

    build_explicit Main || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    mv "$WORKSPACE/.once/out" "$WORKSPACE/saved-outputs"
    build_explicit Main || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/Main/Main")" = 14 ] || return 1

    perl -pi -e 's/NATIVE_VALUE 7/NATIVE_VALUE 9/' "$WORKSPACE/Sources/NativeValue/include/NativeValue.h"
    build_explicit Main || return
    jq -e '.cache == "miss"' "$WORKSPACE/build.json" >/dev/null || return
    [ "$("$WORKSPACE/.once/out/Main/Main")" = 18 ] || return 1

    build_explicit SwiftPackage_ExplicitModules_Main || return
    jq -e '.outputs | any(contains("ModuleScans"))' "$WORKSPACE/build.json" >/dev/null || return
    build_explicit Bridged || return
    build_explicit AppAccess || return
    build_explicit FrameAccess || return

    printf '\nimport NativeValue\n' >> "$WORKSPACE/Sources/Main/main.swift"
    if ONCE_CHANGE_TRACKER=0 RUST_LOG=off once build Main --format json > "$WORKSPACE/strict.json" 2> "$WORKSPACE/strict.err"; then
      return 1
    fi
    jq -e '.. | objects | select(.code? == "undeclared_module_dependency") | .target == "Main" and .attribute == "deps" and (.repairs | length > 0)' "$WORKSPACE/strict.err" >/dev/null
  }

  exercise_native_xcode() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/xcode-workspace-native-project/." "$WORKSPACE/"
    perl -pi -e 's/SDKROOT = iphoneos/SDKROOT = macosx/; s/IPHONEOS_DEPLOYMENT_TARGET = 16.0/MACOSX_DEPLOYMENT_TARGET = 13.0; SWIFT_ENABLE_EXPLICIT_MODULES = YES/' "$WORKSPACE/Hello.xcodeproj/project.pbxproj"
    build_explicit Hello || return
    jq -e '.outputs | any(contains("ModuleScans"))' "$WORKSPACE/build.json" >/dev/null || return
    build_explicit Hello || return
    jq -e '.cache == "hit"' "$WORKSPACE/build.json" >/dev/null
  }

  It 'shares module actions, restores clean builds, invalidates headers, and rejects undeclared native imports'
    Skip if 'requires an Apple Swift compiler' explicit_module_toolchain_unavailable
    When call exercise_explicit_cache
    The status should be success
    The output should equal ''
  End

  It 'honors native Xcode explicit-module settings and restores a framework from cache'
    Skip if 'requires Xcode' explicit_module_toolchain_unavailable
    When call exercise_native_xcode
    The status should be success
    The output should equal ''
  End
End
