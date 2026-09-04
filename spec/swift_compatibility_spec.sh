#shellcheck shell=bash

Describe 'swift compatibility'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  install_system_swift() {
    mkdir -p "$WORKSPACE/tools"
    cat > "$WORKSPACE/tools/swift" <<'SH'
#!/bin/sh
printf '%s\n' "$*"
exit "${SWIFT_EXIT_CODE:-0}"
SH
    chmod +x "$WORKSPACE/tools/swift"
  }

  swift_package_tools_unavailable() {
    [ "$(uname -s)" != "Darwin" ] || ! command -v swift >/dev/null 2>&1 || ! command -v xcrun >/dev/null 2>&1
  }

  copy_native_package() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/swift-package-workspace-native-project/." "$WORKSPACE/"
  }

  clear_swift_build_outputs() {
    find "$WORKSPACE/.once/out" -type f -delete
    find "$WORKSPACE/.once/out" -depth -type d -exec rmdir {} \;
  }

  swift_library_evidence() {
    "$ONCE_BIN" -C "$WORKSPACE" --format json query evidence SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackage:build
  }

  swift_consumer_evidence() {
    "$ONCE_BIN" -C "$WORKSPACE" --format json query evidence SwiftPackage_OnceNativeSwiftPackage_OnceNativeConsumer:build
  }

  swift_dependency_evidence() {
    "$ONCE_BIN" -C "$WORKSPACE" --format json query evidence SwiftPackage_OnceNativeSwiftPackage_OnceNativeDependency:build
  }

  verify_swift_cache_evidence() {
    cmp "$WORKSPACE/first-artifacts" "$WORKSPACE/second-artifacts" || return
    jq -e --slurpfile first "$WORKSPACE/first-evidence.json" '
      ([.[] | select(.cache == "hit") | .action_digest] | sort) ==
      ([$first[0][] | .action_digest] | sort)
    ' "$WORKSPACE/second-evidence.json" >/dev/null || return
    jq -e '
      sort_by(.created_at_unix_ms) | .[-4:] |
      ([.[] | select(.cache == "hit")] | length == 2) and
      ([.[] | select(.cache == "miss")] | length == 2) and
      any(.[]; .cache == "miss" and (.outputs | keys[] | endswith(".a"))) and
      any(.[]; .cache == "miss" and (.outputs | keys[] | endswith(".swiftmodule")))
    ' "$WORKSPACE/third-evidence.json" >/dev/null
  }

  verify_consumer_reuses_unchanged_module() {
    jq -e \
      --slurpfile before "$WORKSPACE/dependency-before-evidence.json" \
      --slurpfile after "$WORKSPACE/dependency-after-evidence.json" '
      def latest_module(records):
        [records[] | select(.outputs | has(".once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeDependency/OnceNativeDependency.swiftmodule"))]
        | sort_by(.created_at_unix_ms)
        | last
        | .outputs[".once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeDependency/OnceNativeDependency.swiftmodule"];
      (latest_module($before[0]) == latest_module($after[0])) and
      ($after[0] | sort_by(.created_at_unix_ms) | .[-4:] | ([.[] | select(.cache == "miss")] | length == 2)) and
      (sort_by(.created_at_unix_ms) | .[-2:] | length == 2 and all(.[]; .cache == "hit"))
    ' "$WORKSPACE/consumer-evidence.json" >/dev/null
  }

  restore_swift_artifacts_from_cache() {
    copy_native_package

    once swift -- build >/dev/null
    find "$WORKSPACE/.once/out" -type f -print | sort > "$WORKSPACE/first-artifacts"
    swift_library_evidence > "$WORKSPACE/first-evidence.json"
    clear_swift_build_outputs

    once swift -- build >/dev/null
    find "$WORKSPACE/.once/out" -type f -print | sort > "$WORKSPACE/second-artifacts"
    swift_library_evidence > "$WORKSPACE/second-evidence.json"

    printf '\npublic func cacheProbe() {}\n' >> "$WORKSPACE/Sources/OnceNativeSwiftPackage/Greeting.swift"
    clear_swift_build_outputs
    once swift -- build >/dev/null
    swift_library_evidence > "$WORKSPACE/third-evidence.json"

    verify_swift_cache_evidence
  }

  reuse_consumer_when_module_is_unchanged() {
    copy_native_package

    once swift -- build >/dev/null
    swift_dependency_evidence > "$WORKSPACE/dependency-before-evidence.json"
    perl -0pi -e 's/"one"/"two"/' "$WORKSPACE/Sources/OnceNativeDependency/Dependency.swift"
    clear_swift_build_outputs
    once swift -- build >/dev/null
    swift_dependency_evidence > "$WORKSPACE/dependency-after-evidence.json"
    swift_consumer_evidence > "$WORKSPACE/consumer-evidence.json"

    verify_consumer_reuses_unchanged_module
  }

  It 'passes unsupported invocations through without a Once trailer'
    install_system_swift

    When call env PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" swift -- package resolve
    The status should be success
    The stdout should equal 'package resolve'
    The stderr should not include 'cache '
  End

  It 'preserves the system swift exit status for unsupported invocations'
    install_system_swift

    When call env SWIFT_EXIT_CODE=17 PATH="$WORKSPACE/tools:$PATH" "$ONCE_BIN" -C "$WORKSPACE" swift -- build -c release
    The status should be failure
    The status should equal 17
    The stdout should equal 'build -c release'
  End

  It 'routes a compatible debug build through Once'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable
    copy_native_package

    When call once swift -- build
    The status should be success
    The stdout should include 'once: build swift_package (swift_package_workspace)'
  End

  It 'routes a compatible debug test through Once'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable
    copy_native_package

    When call once swift -- test
    The status should be success
    The stdout should include 'once: test SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackageTests (apple_test_bundle)'
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackageTests/test/test_results.json" should be file
  End

  It 'builds and tests a discovered Swift package without a Once manifest'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable
    copy_native_package
    rm "$WORKSPACE/once.toml"

    When call /bin/sh -c '"$1" -C "$2" build --quiet && "$1" -C "$2" test --quiet' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include 'once: build swift_package (swift_package_workspace)'
    The stdout should include 'test batches'
    The path "$WORKSPACE/once.toml" should not be exist
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackageTests/test/test_results.json" should be file
  End

  It 'restores every declared artifact and invalidates only changed Swift actions'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable

    When call restore_swift_artifacts_from_cache
    The status should be success
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackage/OnceNativeSwiftPackage.a" should be file
    The path "$WORKSPACE/.once/out/SwiftPackage_OnceNativeSwiftPackage_OnceNativeSwiftPackage/OnceNativeSwiftPackage.abi.json" should be file
  End

  It 'reuses a consumer when a dependency implementation preserves its module'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_package_tools_unavailable

    When call reuse_consumer_when_module_is_unchanged
    The status should be success
  End
End
